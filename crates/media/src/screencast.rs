// SPDX-License-Identifier: GPL-3.0-or-later
//! `org.gnome.Mutter.ScreenCast` from the app's own connection (`docs/spikes/05`): a
//! session, one `RecordMonitor` stream, its PipeWire node, Stop. No portal and no prompt,
//! because the app is a session peer of the shell (`spec/01` §2 row 21).
//!
//! The session lives exactly as long as the connection that made it, so the recorder keeps
//! one connection for the whole recording, and the compositor's `Closed` signal -- the
//! monitor went away, the shell restarted -- is the end of the recording, not an error to
//! retry.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use futures_channel::oneshot;
use futures_util::future::{Either, select};
use gio::prelude::*;

const DEST: &str = "org.gnome.Mutter.ScreenCast";
const ROOT: &str = "/org/gnome/Mutter/ScreenCast";
const SESSION_IFACE: &str = "org.gnome.Mutter.ScreenCast.Session";
const STREAM_IFACE: &str = "org.gnome.Mutter.ScreenCast.Stream";
const CALL_TIMEOUT_MS: i32 = 5000;
/// How long `Start` may take to announce the PipeWire node before the recorder gives up.
const NODE_TIMEOUT: Duration = Duration::from_secs(5);

/// What can go wrong between `CreateSession` and a PipeWire node.
#[derive(Debug, thiserror::Error)]
pub enum ScreenCastError {
    #[error("{method} failed: {source}")]
    Call {
        method: &'static str,
        #[source]
        source: glib::Error,
    },
    #[error("{method} answered with an unexpected shape")]
    Malformed { method: &'static str },
    #[error("the stream announced no PipeWire node within {0:?}")]
    NoNode(Duration),
    #[error("the compositor closed the session")]
    Closed,
    #[error("the PipeWire node {node} has no object.serial that pw-cli can report")]
    NoSerial { node: u32 },
}

/// What runs when the compositor closes the session.
type ClosedCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// One ScreenCast session on one connection.
pub struct Session {
    connection: gio::DBusConnection,
    path: String,
    closed: Rc<Cell<bool>>,
    on_closed: ClosedCallback,
    /// Dropped with the session, which unsubscribes.
    subscriptions: Vec<gio::SignalSubscription>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("path", &self.path).field("closed", &self.closed.get()).finish()
    }
}

/// One stream of a session: the object path `RecordMonitor` answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub path: String,
}

/// How `pipewiresrc` is pointed at the stream's node (`docs/spikes/11`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `path=<node id>`: the id `PipeWireStreamAdded` carries, deprecated by pipewiresrc
    /// but resolved by the session manager.
    NodeId(u32),
    /// `target-object=<object.serial>`: unique for the life of the daemon where node ids
    /// are reused.
    Serial(u64),
}

impl Session {
    /// `CreateSession`, and a watch on the session's `Closed` signal.
    pub async fn create(connection: &gio::DBusConnection) -> Result<Self, ScreenCastError> {
        let options = glib::VariantDict::new(None);
        let reply = call(
            connection,
            ROOT,
            DEST,
            "CreateSession",
            Some(glib::Variant::tuple_from_iter([options.end()])),
            "(o)",
        )
        .await?;
        let path = object_path(&reply, "CreateSession")?;

        let closed = Rc::new(Cell::new(false));
        let on_closed: ClosedCallback = Rc::new(RefCell::new(None));
        let watch = {
            let closed = Rc::clone(&closed);
            let on_closed = Rc::clone(&on_closed);
            connection.subscribe_to_signal(
                Some(DEST),
                Some(SESSION_IFACE),
                Some("Closed"),
                Some(&path),
                None,
                gio::DBusSignalFlags::NONE,
                move |_| {
                    closed.set(true);
                    tracing::info!("the compositor closed the screencast session");
                    if let Some(callback) = on_closed.borrow().as_ref() {
                        callback();
                    }
                },
            )
        };
        tracing::debug!(path, "screencast session created");
        Ok(Self {
            connection: connection.clone(),
            path,
            closed,
            on_closed,
            subscriptions: vec![watch],
        })
    }

    /// The session's object path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// True once the compositor has said `Closed`.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }

    /// Runs `callback` when the compositor closes the session under the recorder.
    pub fn connect_closed(&self, callback: impl Fn() + 'static) {
        *self.on_closed.borrow_mut() = Some(Box::new(callback));
    }

    /// `RecordMonitor` of `connector`, in the monitor's physical pixels (`docs/spikes/05`
    /// caveat 1). `cursor_mode` is Mutter's: 0 hidden, 1 embedded, 2 metadata.
    /// `is_recording` tells the compositor to show its own recording indicator.
    pub async fn record_monitor(
        &self,
        connector: &str,
        cursor_mode: u32,
        is_recording: bool,
    ) -> Result<Stream, ScreenCastError> {
        let options = glib::VariantDict::new(None);
        options.insert_value("cursor-mode", &cursor_mode.to_variant());
        options.insert_value("is-recording", &is_recording.to_variant());
        let reply = call(
            &self.connection,
            &self.path,
            SESSION_IFACE,
            "RecordMonitor",
            Some(glib::Variant::tuple_from_iter([connector.to_variant(), options.end()])),
            "(o)",
        )
        .await?;
        let path = object_path(&reply, "RecordMonitor")?;
        tracing::debug!(connector, cursor_mode, is_recording, path, "stream created");
        Ok(Stream { path })
    }

    /// `Start`, and the PipeWire node id the stream announces. The watch is in place
    /// before the call, because Mutter announces the node while `Start` is running.
    pub async fn start(&mut self, stream: &Stream) -> Result<u32, ScreenCastError> {
        let (tx, rx) = oneshot::channel::<u32>();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let watch = self.connection.subscribe_to_signal(
            Some(DEST),
            Some(STREAM_IFACE),
            Some("PipeWireStreamAdded"),
            Some(&stream.path),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                if let Some(node) = signal.parameters.child_value(0).get::<u32>()
                    && let Some(tx) = tx.borrow_mut().take()
                {
                    let _ = tx.send(node);
                }
            },
        );
        self.subscriptions.push(watch);

        call(&self.connection, &self.path, SESSION_IFACE, "Start", None, "()").await?;

        match select(rx, glib::timeout_future(NODE_TIMEOUT)).await {
            Either::Left((Ok(node), _)) => {
                tracing::debug!(node, "pipewire node announced");
                Ok(node)
            }
            Either::Left((Err(_), _)) => Err(ScreenCastError::Closed),
            Either::Right(((), _)) => Err(ScreenCastError::NoNode(NODE_TIMEOUT)),
        }
    }

    /// `Stop`. A session the compositor has already closed is not an error to stop.
    pub async fn stop(&self) -> Result<(), ScreenCastError> {
        if self.closed.get() {
            return Ok(());
        }
        match call(&self.connection, &self.path, SESSION_IFACE, "Stop", None, "()").await {
            Ok(_) => {
                tracing::debug!(path = self.path, "screencast session stopped");
                Ok(())
            }
            Err(ScreenCastError::Call { source, .. }) if is_gone(&source) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// The node's `object.serial`, which `pipewiresrc target-object` wants and which nothing
/// on the bus reports (`docs/spikes/05`): asked of `pw-cli`, the way the spike did.
pub fn resolve_serial(node: u32) -> Result<u64, ScreenCastError> {
    let output = std::process::Command::new("pw-cli")
        .args(["info", &node.to_string()])
        .output()
        .map_err(|e| {
            tracing::warn!("pw-cli could not be run: {e}");
            ScreenCastError::NoSerial { node }
        })?;
    let text = String::from_utf8_lossy(&output.stdout);
    // A property line reads `*\t\tobject.serial = "202"`; the star marks a property the
    // object set itself.
    text.lines()
        .find_map(|line| {
            let line = line.trim_start_matches(['*', ' ', '\t']);
            let value = line.strip_prefix("object.serial = \"")?;
            value.strip_suffix('"')?.parse::<u64>().ok()
        })
        .ok_or(ScreenCastError::NoSerial { node })
}

async fn call(
    connection: &gio::DBusConnection,
    path: &str,
    interface: &str,
    method: &'static str,
    params: Option<glib::Variant>,
    reply_type: &str,
) -> Result<glib::Variant, ScreenCastError> {
    let ty = glib::VariantTy::new(reply_type)
        .map_err(|_| ScreenCastError::Malformed { method })?;
    connection
        .call_future(
            Some(DEST),
            path,
            interface,
            method,
            params.as_ref(),
            Some(ty),
            gio::DBusCallFlags::NONE,
            CALL_TIMEOUT_MS,
        )
        .await
        .map_err(|source| ScreenCastError::Call { method, source })
}

fn object_path(reply: &glib::Variant, method: &'static str) -> Result<String, ScreenCastError> {
    reply
        .child_value(0)
        .str()
        .map(str::to_owned)
        .ok_or(ScreenCastError::Malformed { method })
}

/// "Object does not exist": the session is already gone, which is what Stop wanted.
fn is_gone(e: &glib::Error) -> bool {
    matches!(
        e.kind::<gio::DBusError>(),
        Some(gio::DBusError::UnknownObject | gio::DBusError::UnknownMethod)
    ) || e.message().contains("does not exist")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_is_one_of_two_spellings() {
        assert_ne!(Target::NodeId(93), Target::Serial(93));
        assert_eq!(Target::Serial(202), Target::Serial(202));
    }
}

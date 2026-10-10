// SPDX-License-Identifier: GPL-3.0-or-later

//! `SYS-13` (D181): GNOME's Activities search finds captures in the history, by the text
//! read in them (`HIS-05`), the app they came from or their window's title.
//!
//! `org.gnome.Shell.SearchProvider2`, exported from [`Application`]'s `dbus_register`,
//! which runs before GApplication takes the bus name. The Shell's first query is often
//! what starts the app, and a call to an object exported in `startup`, after the name, is
//! answered "no such object" before the main loop has run (`docs/spikes/13`). The `.ini`
//! in `data/` tells the Shell the provider is here.
//!
//! A result is a capture's id. Its name is the window's title, or else the app's name, or
//! else what kind of capture it is; its description says when, and the line of text the
//! words were found on; its icon is the capture's thumbnail. Activating one restores it as
//! a card, as Return does in the history strip, and the Shell's "show all" opens the strip
//! searching for the same words.

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};
use octosnap_core::history::{Entry, THUMB_FILE, relative_time};
use octosnap_core::search;
use tracing::{debug, info, warn};

/// Where the provider is, as `data/io.github.odrakirmusic.OctoSnap.search-provider.ini`
/// says.
pub const OBJECT_PATH: &str = "/io/github/odrakirmusic/OctoSnap/SearchProvider";

const INTERFACE: &str = "org.gnome.Shell.SearchProvider2";

const XML: &str = r#"<node>
  <interface name="org.gnome.Shell.SearchProvider2">
    <method name="GetInitialResultSet">
      <arg type="as" name="terms" direction="in"/>
      <arg type="as" name="results" direction="out"/>
    </method>
    <method name="GetSubsearchResultSet">
      <arg type="as" name="previous_results" direction="in"/>
      <arg type="as" name="terms" direction="in"/>
      <arg type="as" name="results" direction="out"/>
    </method>
    <method name="GetResultMetas">
      <arg type="as" name="identifiers" direction="in"/>
      <arg type="aa{sv}" name="metas" direction="out"/>
    </method>
    <method name="ActivateResult">
      <arg type="s" name="identifier" direction="in"/>
      <arg type="as" name="terms" direction="in"/>
      <arg type="u" name="timestamp" direction="in"/>
    </method>
    <method name="LaunchSearch">
      <arg type="as" name="terms" direction="in"/>
      <arg type="u" name="timestamp" direction="in"/>
    </method>
  </interface>
</node>"#;

/// The most results one query answers. The Shell shows a few, and asks for the metas of
/// those it shows.
const MOST: usize = 50;
/// A description is one line under the result's name.
const SNIPPET_CHARS: usize = 60;

thread_local! {
    /// The last query's words: `GetResultMetas` is asked for ids alone, and a description
    /// shows where the words were found.
    static LAST_TERMS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Application {
        pub provider: RefCell<Option<gio::RegistrationId>>,
    }

    impl std::fmt::Debug for Application {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Application")
                .field("provider", &self.provider.borrow().is_some())
                .finish()
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "OctosnapApplication";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {}

    impl ApplicationImpl for Application {
        fn dbus_register(
            &self,
            connection: &gio::DBusConnection,
            object_path: &str,
        ) -> Result<(), glib::Error> {
            self.parent_dbus_register(connection, object_path)?;
            let app = self.obj().upcast_ref::<adw::Application>().downgrade();
            match super::register(connection, app) {
                Ok(id) => {
                    *self.provider.borrow_mut() = Some(id);
                    debug!(path = OBJECT_PATH, "search provider exported");
                }
                // A convenience: the app is whole without it.
                Err(e) => warn!("could not export the search provider: {e}"),
            }
            Ok(())
        }

        fn dbus_unregister(&self, connection: &gio::DBusConnection, object_path: &str) {
            if let Some(id) = self.provider.borrow_mut().take() {
                let _ = connection.unregister_object(id);
            }
            self.parent_dbus_unregister(connection, object_path);
        }
    }

    impl GtkApplicationImpl for Application {}
    impl AdwApplicationImpl for Application {}
}

glib::wrapper! {
    /// The app: an `AdwApplication` that exports the search provider with its name.
    pub struct Application(ObjectSubclass<imp::Application>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Application {
    #[must_use]
    pub fn new(id: &str, flags: gio::ApplicationFlags) -> Self {
        glib::Object::builder().property("application-id", id).property("flags", flags).build()
    }
}

fn register(
    connection: &gio::DBusConnection,
    app: glib::WeakRef<adw::Application>,
) -> Result<gio::RegistrationId, glib::Error> {
    let node = gio::DBusNodeInfo::for_xml(XML)?;
    let interface = node.lookup_interface(INTERFACE).ok_or_else(|| {
        glib::Error::new(gio::IOErrorEnum::Failed, &format!("{INTERFACE} is missing from its XML"))
    })?;
    connection
        .register_object(OBJECT_PATH, &interface)
        .method_call(move |_connection, _sender, _path, _interface, method, params, invocation| {
            let strings =
                |at: usize| -> Vec<String> { params.child_value(at).get().unwrap_or_default() };
            match method {
                "GetInitialResultSet" => {
                    invocation.return_value(Some(&(found(&strings(0)),).to_variant()));
                }
                "GetSubsearchResultSet" => {
                    let previous = strings(0);
                    let narrowed: Vec<String> =
                        found(&strings(1)).into_iter().filter(|id| previous.contains(id)).collect();
                    invocation.return_value(Some(&(narrowed,).to_variant()));
                }
                "GetResultMetas" => {
                    let metas = metas(&strings(0));
                    invocation.return_value(Some(&glib::Variant::tuple_from_iter([metas])));
                }
                "ActivateResult" => {
                    let id: String = params.child_value(0).get().unwrap_or_default();
                    activate(&id);
                    invocation.return_value(None);
                }
                "LaunchSearch" => {
                    let words = strings(0);
                    if let Some(app) = app.upgrade() {
                        info!(terms = words.len(), "the Activities search opens the history");
                        crate::history::strip::search(&app, &words.join(" "));
                    }
                    invocation.return_value(None);
                }
                other => invocation.return_dbus_error(
                    "org.freedesktop.DBus.Error.UnknownMethod",
                    &format!("{INTERFACE} has no method '{other}'"),
                ),
            }
        })
        .build()
}

/// The captures the words find, newest first.
fn found(words: &[String]) -> Vec<String> {
    let terms = search::terms_of(words);
    LAST_TERMS.with(|last| last.borrow_mut().clone_from(&terms));
    let Some(history) = crate::history() else { return Vec::new() };
    let found: Vec<String> =
        history.search(&terms).into_iter().take(MOST).map(|entry| entry.id).collect();
    // How many words and how many found, never which.
    debug!(terms = terms.len(), found = found.len(), "activities search");
    found
}

/// Each result's name, description and icon, as `aa{sv}`.
fn metas(ids: &[String]) -> glib::Variant {
    let now = glib::real_time().unsigned_abs();
    let terms = LAST_TERMS.with(|last| last.borrow().clone());
    let history = crate::history();
    let metas: Vec<glib::Variant> = ids
        .iter()
        .filter_map(|id| {
            let history = history.as_ref()?;
            let entry = history.get(id)?;
            let text = history.text_of(&entry);
            let dict = glib::VariantDict::new(None);
            dict.insert_value("id", &id.to_variant());
            dict.insert_value("name", &name_of(&entry).to_variant());
            let description =
                description_of(&entry, text.as_deref().unwrap_or_default(), &terms, now);
            dict.insert_value("description", &description.to_variant());
            if let Some(icon) = icon_of(&entry).serialize() {
                dict.insert_value("icon", &icon);
            }
            Some(dict.end())
        })
        .collect();
    glib::Variant::array_from_iter_with_type(glib::VariantTy::VARDICT, metas)
}

/// The window's title, or else the app's name, or else the kind of capture.
fn name_of(entry: &Entry) -> String {
    [&entry.source.title, &entry.source.app_name]
        .into_iter()
        .find(|name| !name.trim().is_empty())
        .map_or_else(|| entry.kind.label().to_owned(), |name| name.trim().to_owned())
}

/// When, and the line the words were found on -- or the app, when the name is the title.
fn description_of(entry: &Entry, text: &str, terms: &[String], now: u64) -> String {
    let age = relative_time(entry.created_at, now);
    if let Some(line) = search::snippet(text, terms, SNIPPET_CHARS) {
        return format!("{age} \u{b7} {line}");
    }
    let app = entry.source.app_name.trim();
    if !entry.source.title.trim().is_empty() && !app.is_empty() {
        return format!("{app} \u{b7} {age}");
    }
    age
}

/// The capture's thumbnail, or the app's icon while there is none.
fn icon_of(entry: &Entry) -> gio::Icon {
    let thumb = entry.thumb_path.clone().or_else(|| entry.dir().map(|dir| dir.join(THUMB_FILE)));
    match thumb {
        Some(thumb) if thumb.is_file() => gio::FileIcon::new(&gio::File::for_path(thumb)).upcast(),
        _ => gio::ThemedIcon::new(octosnap_core::protocol::APP_BUS_NAME).upcast(),
    }
}

/// A result chosen: the capture leaves the history and is a card again, as Return makes it
/// in the strip.
fn activate(id: &str) {
    let (Some(history), Some(overlay)) = (crate::history(), crate::overlay()) else { return };
    match history.take(id) {
        Some((capture, saved_to)) => {
            let shown = overlay.show(&capture, saved_to.as_deref());
            info!(shown, "the Activities search restored a capture");
        }
        None => warn!(id, "the Activities search chose a capture the history no longer has"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octosnap_core::capture::SourceWindow;
    use octosnap_core::history::Kind;

    fn entry(app_name: &str, title: &str) -> Entry {
        Entry {
            id: "01J".to_owned(),
            kind: Kind::Screenshot,
            created_at: 0,
            filed_at: 0,
            path: "/tmp/01J/01J.png".into(),
            meta_path: None,
            saved_path: None,
            project_path: None,
            thumb_path: None,
            width: 10,
            height: 10,
            scale: 1.0,
            source: SourceWindow {
                app_id: String::new(),
                app_name: app_name.into(),
                title: title.into(),
            },
            display: String::new(),
            duration_ms: None,
        }
    }

    #[test]
    fn a_result_is_named_by_its_title_then_its_app_then_its_kind() {
        assert_eq!(name_of(&entry("Firefox", "Pull request 56")), "Pull request 56");
        assert_eq!(name_of(&entry("Firefox", "  ")), "Firefox");
        assert_eq!(name_of(&entry("", "")), Kind::Screenshot.label());
    }

    #[test]
    fn a_description_says_when_and_where_the_words_are() {
        let now = 3 * 60 * 1_000_000;
        let terms = search::terms("invoice");
        let with_text =
            description_of(&entry("Firefox", "Mail"), "Hello\nInvoice 42 attached", &terms, now);
        assert!(with_text.ends_with("\u{b7} Invoice 42 attached"), "{with_text}");
        // Found by the title: the app, then when.
        let by_title = description_of(&entry("Firefox", "Invoice"), "", &terms, now);
        assert!(by_title.starts_with("Firefox \u{b7} "), "{by_title}");
    }
}

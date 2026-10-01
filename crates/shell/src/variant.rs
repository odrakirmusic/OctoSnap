// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding of the `a{sv}` dictionaries the extension sends.
//!
//! Every getter is fallible and total: a missing or wrongly typed key yields `None`
//! rather than panicking, because the extension and the app are versioned separately and
//! a newer extension is allowed to send keys this build has never heard of.
//!
//! These take a [`glib::VariantDict`], which is glib's index over an `a{sv}` variant.
//! Build one once per dictionary with `VariantDict::new(Some(&variant))` rather than
//! per key, or every lookup rebuilds the index.

use glib::VariantTy;
use octosnap_core::Rect;

pub fn string(dict: &glib::VariantDict, key: &str) -> Option<String> {
    dict.lookup_value(key, Some(VariantTy::STRING))?.get::<String>()
}

pub fn i32_(dict: &glib::VariantDict, key: &str) -> Option<i32> {
    dict.lookup_value(key, Some(VariantTy::INT32))?.get::<i32>()
}

pub fn u32_(dict: &glib::VariantDict, key: &str) -> Option<u32> {
    dict.lookup_value(key, Some(VariantTy::UINT32))?.get::<u32>()
}

pub fn u64_(dict: &glib::VariantDict, key: &str) -> Option<u64> {
    dict.lookup_value(key, Some(VariantTy::UINT64))?.get::<u64>()
}

pub fn f64_(dict: &glib::VariantDict, key: &str) -> Option<f64> {
    dict.lookup_value(key, Some(VariantTy::DOUBLE))?.get::<f64>()
}

pub fn bool_(dict: &glib::VariantDict, key: &str) -> Option<bool> {
    dict.lookup_value(key, Some(VariantTy::BOOLEAN))?.get::<bool>()
}

/// A `(iiii)` tuple as a logical rect.
pub fn rect(dict: &glib::VariantDict, key: &str) -> Option<Rect> {
    let ty = VariantTy::new("(iiii)").ok()?;
    let (x, y, width, height) = dict.lookup_value(key, Some(ty))?.get::<(i32, i32, i32, i32)>()?;
    Some(Rect { x, y, width, height })
}

/// Four separate integer keys as a rect, which is how `GetMonitors` reports geometry.
pub fn rect_from_parts(
    dict: &glib::VariantDict,
    x: &str,
    y: &str,
    width: &str,
    height: &str,
) -> Option<Rect> {
    Some(Rect {
        x: i32_(dict, x)?,
        y: i32_(dict, y)?,
        width: i32_(dict, width)?,
        height: i32_(dict, height)?,
    })
}

#[cfg(test)]
// A failing assertion should panic, so expect() is the right tool here. The
// workspace-wide ban exists to keep panics out of D-Bus handlers, not out of tests.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn dict() -> glib::VariantDict {
        let d = glib::VariantDict::new(None);
        d.insert("connector", "eDP-1");
        d.insert("x", 0i32);
        d.insert("y", 0i32);
        d.insert("width", 1536i32);
        d.insert("height", 960i32);
        d.insert("scale", 1.25f64);
        d.insert("primary", true);
        d.insert_value("work-area", &glib::Variant::from((0i32, 0i32, 1536i32, 923i32)));
        d
    }

    #[test]
    fn reads_each_scalar_type() {
        let d = dict();
        assert_eq!(string(&d, "connector").as_deref(), Some("eDP-1"));
        assert_eq!(i32_(&d, "width"), Some(1536));
        assert_eq!(f64_(&d, "scale"), Some(1.25));
        assert_eq!(bool_(&d, "primary"), Some(true));
    }

    #[test]
    fn reads_rects_both_ways() {
        let d = dict();
        assert_eq!(rect_from_parts(&d, "x", "y", "width", "height"), Some(Rect::new(0, 0, 1536, 960)));
        assert_eq!(rect(&d, "work-area"), Some(Rect::new(0, 0, 1536, 923)));
    }

    /// A key this build does not know about, or one sent with the wrong type, must not
    /// take the whole capture down.
    #[test]
    fn missing_and_mistyped_keys_are_none() {
        let d = dict();
        assert_eq!(string(&d, "no-such-key"), None);
        assert_eq!(rect(&d, "no-such-key"), None);
        // "width" is an i32, so asking for a double must fail rather than coerce.
        assert_eq!(f64_(&d, "width"), None);
    }
}

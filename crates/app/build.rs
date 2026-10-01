// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiles the application's GResource bundle.
//!
//! Icons, since `spec/05` §2's tool strip needs shapes the Adwaita theme does not ship, and
//! the desktop pets' sheets. See `resources/octosnap.gresource.xml` for which and why.

fn main() {
    glib_build_tools::compile_resources(
        &["resources", "../../data"],
        "resources/octosnap.gresource.xml",
        "octosnap.gresource",
    );
}

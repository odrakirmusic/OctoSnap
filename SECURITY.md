# Security

## Reporting

Report a vulnerability privately, from this repository's **Security** tab, with **Report a vulnerability**. It reaches the maintainer only. Please do not open a public issue for it.

Say what an attacker could do, and how. Steps or a proof of concept help. Expect an answer within a week. A fix goes out in a release, which credits you unless you would rather it did not.

Only the latest release is fixed.

## What is in scope

- **The app**, which reads every capture and recording, and the files it saves and keeps in its history.
- **The GNOME Shell extension.** It runs inside GNOME Shell, with all of the user's privileges, so it is the most sensitive part. Its D-Bus interface is on the session bus, where any of the user's processes can call it, and what it hands out there is in scope too.
- **The Flatpak's sandbox.** It may write one folder outside itself, the extension's own in `~/.local/share/gnome-shell/extensions`, which is how the Flatpak installs and updates the extension. A way to write elsewhere through it, or to put code there that the app did not carry, is a vulnerability.
- **The update repository** at <https://odrakirmusic.github.io/OctoSnap>, which every install updates from. Each release there is signed, and Flatpak installs nothing that its key, `build-aux/flatpak/octosnap.gpg`, did not sign. The site lists the key's fingerprint. A release that installs without that signature is a vulnerability, and so is a signature on something the maintainer did not publish.

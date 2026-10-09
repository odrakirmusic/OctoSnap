# Contributing to OctoSnap

Issues and pull requests are welcome. OctoSnap has one maintainer, who reads every one of them and is the only one who merges.

## Reporting a problem

Open an [issue](https://github.com/odrakirmusic/OctoSnap/issues/new/choose) and pick **Something does not work**. What helps most:

- OctoSnap's version, from Settings → About, and how it is installed: the Flatpak from OctoSnap's repository, a release's `.flatpak` file, or built natively.
- GNOME Shell's version (`gnome-shell --version`), and your monitors with their scales. Much of what goes wrong on one scale does not on another.
- What you did, what happened, and what you expected.
- A report: Settings → About → Troubleshooting → **Save a report** writes one file with the versions, the extension's state, the monitors and both halves' recent logs. Nothing is sent anywhere. Read it before you attach it.

A security problem is not an issue: [SECURITY.md](SECURITY.md) says where it goes.

## Asking for something

Open an issue and pick **Something it could do**. Say what you would do with it. If another tool does it well, say which and how.

## Pull requests

For a fix, open the pull request. For anything larger, a new feature or a change to how something behaves, open an issue first, so the two of you agree on what it should do before you write it.

### How a pull request lands

This repository receives one commit a release, written from the maintainer's development repository, where the design notes, the test harnesses and the history are kept. A pull request is therefore not merged here: the next release's commit would replace it. Instead:

1. The maintainer reviews it here, as on any project.
2. Once it is ready, your commits are carried into the development repository with you as their author, and tested there with the rest.
3. The next release publishes them, and its commit names you as a co-author, which GitHub counts as yours.
4. The pull request is closed with a link to that release.

### Before you open one

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cd extension && npm ci && npx tsc --noEmit && npx vitest run
```

[Building and testing](README.md#building-and-testing) has the rest, and [How it is built](README.md#how-it-is-built) explains the two halves and what each owns.

- Every new file starts with the SPDX header its neighbours have: `SPDX-License-Identifier: GPL-3.0-or-later`.
- Comments say why, not what. Many cite design notes by number (D154) or by section (`spec/14` §3). Those notes are not public, and you do not need them. If a comment's reasoning is not clear without them, ask in the pull request.
- On GNOME Wayland, test what you change at more than one display scale if you can. 100 %, 125 %, 150 % and 200 % behave differently.
- Most of OctoSnap is written by Claude, Anthropic's AI coding agent, under the maintainer's direction (see the [README](README.md#how-it-is-built)). Your pull request is read by the maintainer all the same.

## Licence

What you contribute is under the licence of what it changes: GPL-3.0-or-later for the code, CC-BY-SA-4.0 for the icons and sounds ([Licence](README.md#licence)). There is no agreement to sign. Opening the pull request is agreeing to that, as GitHub's terms say.

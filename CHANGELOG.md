# Changelog

Notable changes to SpareLine, newest first. Versions follow [semantic versioning](https://semver.org): while SpareLine is at 0.x, the middle number goes up when an update needs the server and every app to be updated together, and the last number goes up for everything else.

Add notes under **Unreleased** as you make changes. `cargo xtask version bump` moves them under the new version's heading.

## Unreleased

## 0.1.22 (2026-11-15)

This is the first version recorded in this changelog, so these notes describe what SpareLine does as of this version.

### Added

- **SpareLine Server** with one-screen setup, and a control panel for people, groups, invites, announcements, diagnostics and health checks. The panel has no way to read messages.
- **The SpareLine app** for staff. It joins with a one-time invite, finds the server on the network by itself, and checks it's the real server before sending anything.
- **End-to-end encrypted chats** with MLS (RFC 9420): group chats, direct messages, replies, edits, deletions and reactions. History is stored encrypted on each computer.
- **Standby copies.** The server saves an encrypted copy of itself to a folder you choose, so another computer can take over with the same identity if it fails. Staff apps reconnect and catch up by themselves.
- **Command-line options** for running the server without the control panel, including standby copies and restoring. Run `spareline-server --help` to see them, or `--version` to see the version.
- **One version number for the whole project**, with `cargo xtask version` to show, check and change it.

### Changed

- A quieter, compact design for both apps, in light and dark, with a command menu on <kbd>⌘K</kbd> (<kbd>Ctrl</kbd>+<kbd>K</kbd> on Windows).

### Fixed

- Crashes after joining a server, and when showing pop-up notices from background tasks.

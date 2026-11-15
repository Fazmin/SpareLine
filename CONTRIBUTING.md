# Contributing to SpareLine

Thanks for helping. SpareLine is a backup chat that organisations rely on when everything else is down, so we care most about three things: **privacy, reliability, and being easy to use under stress.**

Please read the [code of conduct](CODE_OF_CONDUCT.md) first.

## Ways to help

- **Report a bug:** open an issue with what happened, what you expected, your OS, and the app version (Settings → About). For connection problems, attach the report from **Settings → Connection → Copy report for your admin**. It contains no message content.
- **Suggest an improvement:** open an issue describing the situation it helps with, for example "during an outage, admins need to…".
- **Send a pull request:** for anything bigger than a small fix, open an issue first so we can agree on the approach.

**Security issues:** please don't open a public issue. Use GitHub's private vulnerability reporting (the **Security** tab → **Report a vulnerability**).

## Set up

You need:

- Rust 1.91 or newer, plus the wasm target: `rustup target add wasm32-unknown-unknown`
- Trunk and the Tauri CLI: `cargo install trunk` and `cargo install tauri-cli --version "^2"`
- On Windows, WebView2 (built into Windows 11). On macOS, the Xcode command-line tools.

```sh
cargo xtask dev-server     # server + control panel (the first run shows setup)
cargo xtask dev-client     # chat client
cargo xtask ci             # what CI runs: version check, fmt, clippy (native + wasm), tests
cargo xtask version        # show the version (see "Versions and releases" below)
cargo xtask discover       # list SpareLine servers visible on your network
```

Tips:

- **Two clients on one machine:** start the second with its own data folder, for example `SPARELINE_CLIENT_DIR=/tmp/sl2`.
- **Headless server:** `SPARELINE_ADMIN_PASSWORD="…" spareline-server --headless --data-dir ./dev-data` (`--help` lists the standby and restore options)
- **UI preview with sample data, no backend:** run `trunk serve` in `apps/client` and open `localhost:1420/?demo`, or in `apps/server` and open `localhost:1430/?demo`. Add `=onboarding`, `=signin`, `=setup` or `=locked` to see other screens.

## Project layout

```
crates/sl-proto        wire protocol, IPC types, invite-code crypto (wasm-safe)
crates/sl-net          pinned TLS verifier, WebSocket dialer, LAN helpers
crates/sl-server       server engine: accounts, invites, groups, MLS delivery, health, logs
crates/sl-client-core  client engine: discovery, transport, OpenMLS, encrypted store, outbox
crates/sl-ui           shared Leptos widgets, design tokens (styles/sl.css), a11y prefs
apps/server            control panel UI + Tauri tray app hosting the engine
apps/client            chat UI + Tauri app hosting the client engine
docs/                  privacy statement, admin guide, staff quick start, protocol notes
```

[docs/PROTOCOL.md](docs/PROTOCOL.md) explains the trust model, ordering and MLS membership rules. Read it before changing anything in `sl-server/src/delivery.rs` or `sl-client-core/src/core.rs`.

## Rules that keep SpareLine trustworthy

**Privacy**

- The server must never see or log message content. Opaque payloads use `sl_proto::Bytes`, whose `Debug` output is only a length. Keep it that way.
- The admin panel (`sl-server/src/admin.rs`) must not gain access to envelope storage or per-conversation data.
- Nothing may contact a service outside the organisation's own network.

**Reliability**

- Every change to the protocol or delivery logic needs a test. The end-to-end test in `crates/sl-client-core/tests/e2e.rs` is the place for flows that cross client and server.
- Keep `cargo xtask ci` green: `cargo fmt`, clippy with `-D warnings`, and all tests.

**Accessibility and UI**

- Every control works by keyboard and has a visible focus ring and an accessible name.
- Status never relies on colour alone.
- Text sizes use `rem`, so the text-size setting (up to 200%) keeps working.
- Check new screens in light, dark and high contrast, and with reduced motion.
- Use the tokens in `crates/sl-ui/styles/sl.css`; don't add one-off colours.
- Write copy in plain, active language, and name tools generically ("your usual chat"), not specific vendors.
- In Leptos views, a signal `set` always notifies. Only write when the value changes, or use a `Memo`, to avoid render loops.

## Pull requests

- Keep each PR focused, and describe what changed and why.
- Include screenshots for UI changes, ideally light and dark.
- Note any change to the wire protocol and whether it's backwards compatible. Bump `PROTOCOL_VERSION` if it isn't.
- Add a line under **Unreleased** in [CHANGELOG.md](CHANGELOG.md) for anything people would notice.

## Versions and releases

SpareLine has one version number for both apps, the server and every crate. It lives in one place, `version` under `[workspace.package]` in the root `Cargo.toml`, and everything else follows it:

- The Rust code reports it with `env!("CARGO_PKG_VERSION")`: in Settings → About, in problem reports, and in `spareline-server --version`.
- Both Tauri apps take it from `Cargo.toml` when they're built. Their `tauri.conf.json` files deliberately have no `version`.
- `CITATION.cff`, the README footer and `CHANGELOG.md` repeat it for readers, and `cargo xtask version` keeps them in step.

```sh
cargo xtask version                  # show the version and check every file agrees
cargo xtask version bump patch       # 0.1.22 → 0.1.23
cargo xtask version bump minor       # 0.1.22 → 0.2.0
cargo xtask version set 0.2.0        # set an exact version
```

`bump` and `set` update `Cargo.toml`, `Cargo.lock`, `CITATION.cff` (version and release date), the README footer, and `CHANGELOG.md`, where the notes under **Unreleased** move under the new version. `cargo xtask ci` fails if any of these disagree.

**Which number to change.** While SpareLine is at 0.x, bump **minor** when the server and every app have to be updated together: a change to the wire protocol that isn't backwards compatible (which also bumps `PROTOCOL_VERSION`), or a database change older versions can't read. Bump **patch** for everything else, including new features that work with older apps. Windows installers limit the first two numbers to 255 and the last to 65535.

**Cutting a release:**

1. Check that the notes under **Unreleased** in `CHANGELOG.md` cover the release.
2. Run `cargo xtask version bump patch` (or `minor`), then `cargo xtask ci`.
3. Commit and tag: `git commit -am "Release 0.1.23"` and `git tag -a v0.1.23 -m "SpareLine 0.1.23"`.
4. Push with `git push --follow-tags`, then build the installers with `cargo xtask bundle`.

## License

By contributing, you agree that your contributions are dual-licensed under the [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE) licenses, without any additional terms.

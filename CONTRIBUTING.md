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
cargo xtask ci             # what CI runs: fmt, clippy (native + wasm), tests
cargo xtask discover       # list SpareLine servers visible on your network
```

Tips:

- **Two clients on one machine:** start the second with its own data folder, for example `SPARELINE_CLIENT_DIR=/tmp/sl2`.
- **Headless server:** `SPARELINE_ADMIN_PASSWORD="…" spareline-server --headless --data-dir ./dev-data`
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
- When cutting a release, update `version` in `Cargo.toml` and both `version` and `date-released` in `CITATION.cff`.

## License

By contributing, you agree that your contributions are dual-licensed under the [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE) licenses, without any additional terms.

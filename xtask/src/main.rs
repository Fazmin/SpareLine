//! Developer tasks.
//!
//! ```text
//! cargo xtask ci          fmt check, clippy (deny warnings), tests
//! cargo xtask discover    list SpareLine servers visible on this network (mDNS)
//! cargo xtask dev-server  run the server control panel (cargo tauri dev)
//! cargo xtask dev-client  run the chat client (cargo tauri dev)
//! cargo xtask bundle      release installers for both apps
//! ```

use std::process::{Command, ExitCode};

fn run(dir: &str, prog: &str, args: &[&str]) -> bool {
    println!("$ ({dir}) {prog} {}", args.join(" "));
    Command::new(prog)
        .args(args)
        .current_dir(dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ok = match args.first().map(String::as_str) {
        Some("ci") => {
            run(".", "cargo", &["fmt", "--all", "--check"])
                && run(
                    ".",
                    "cargo",
                    &[
                        "clippy",
                        "--workspace",
                        "--exclude",
                        "spareline-client-ui",
                        "--exclude",
                        "spareline-server-ui",
                        "--exclude",
                        "sl-ui",
                        "--all-targets",
                        "--",
                        "-D",
                        "warnings",
                    ],
                )
                && run(
                    ".",
                    "cargo",
                    &[
                        "clippy",
                        "-p",
                        "spareline-client-ui",
                        "-p",
                        "spareline-server-ui",
                        "-p",
                        "sl-ui",
                        "--target",
                        "wasm32-unknown-unknown",
                        "--",
                        "-D",
                        "warnings",
                    ],
                )
                && run(
                    ".",
                    "cargo",
                    &[
                        "test",
                        "--workspace",
                        "--exclude",
                        "spareline-client-ui",
                        "--exclude",
                        "spareline-server-ui",
                        "--exclude",
                        "sl-ui",
                    ],
                )
        }
        Some("discover") => {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            };
            let found = rt.block_on(sl_client_core::discovery::browse(
                std::time::Duration::from_secs(4),
            ));
            if found.is_empty() {
                println!("No SpareLine servers found on this network (multicast may be blocked).");
            }
            for s in found {
                println!(
                    "{}  {}  fingerprint starts {}",
                    s.name,
                    s.addresses.join(", "),
                    s.fingerprint_hint.unwrap_or_default()
                );
            }
            true
        }
        Some("dev-server") => run("apps/server", "cargo", &["tauri", "dev"]),
        Some("dev-client") => run("apps/client", "cargo", &["tauri", "dev"]),
        Some("bundle") => {
            run("apps/server", "cargo", &["tauri", "build"])
                && run("apps/client", "cargo", &["tauri", "build"])
        }
        _ => {
            eprintln!("usage: cargo xtask <ci|discover|dev-server|dev-client|bundle>");
            false
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

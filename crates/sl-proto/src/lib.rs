//! SpareLine wire protocol and shared types.
//!
//! This crate is the contract between the server engine, the client engine and the two
//! Leptos UIs. It is wasm-safe by default; the `crypto` feature adds invite-code
//! derivations used only by the native engines.
//!
//! Privacy rule: message content never appears here in plaintext. Everything a
//! conversation says travels as MLS ciphertext inside [`Bytes`], whose `Debug` output
//! is only a length.

pub mod b64;
pub mod client_ipc;
pub mod code;
#[cfg(feature = "crypto")]
pub mod crypto;
pub mod error;
pub mod model;
pub mod panel;
pub mod wire;

pub use error::{ApiError, ErrorCode};
pub use model::*;

/// Wire protocol version. Bump on breaking changes.
pub const PROTOCOL_VERSION: u16 = 1;
/// Oldest client protocol the server still accepts.
pub const MIN_PROTOCOL_VERSION: u16 = 1;
/// Default TCP port for the server (TLS WebSocket).
pub const DEFAULT_PORT: u16 = 7443;
/// mDNS / DNS-SD service type advertised on the LAN.
pub const MDNS_SERVICE_TYPE: &str = "_spareline._tcp.local.";
/// WebSocket endpoint.
pub const WS_PATH: &str = "/v1/ws";
/// Plain health endpoint (no auth, no data).
pub const HEALTH_PATH: &str = "/v1/health";
/// Product name.
pub const APP_NAME: &str = "SpareLine";

/// Unix time in milliseconds.
pub type Timestamp = i64;

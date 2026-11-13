//! Shared Leptos kit for the SpareLine apps: Tauri IPC, accessibility preferences,
//! icons, formatting and small widgets. Styles live in `styles/sl.css`.

pub mod format;
pub mod icons;
pub mod ipc;
pub mod prefs;
pub mod widgets;

pub use icons::Icon;
pub use prefs::{PrefsCtx, provide_prefs};
pub use widgets::*;

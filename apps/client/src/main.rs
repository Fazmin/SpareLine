//! SpareLine chat UI (Leptos, compiled to wasm and hosted by the Tauri shell).

mod app;
mod demo;
mod dialogs;
mod markdown;
mod onboarding;
mod people;
mod settings;
mod shell;
mod state;

fn main() {
    if !sl_ui::ipc::in_tauri()
        && let Some(mode) = sl_ui::ipc::demo_requested()
    {
        demo::install(&mode);
    }
    leptos::mount::mount_to_body(app::App);
}

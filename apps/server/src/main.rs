//! SpareLine Server control panel UI (Leptos → wasm, hosted by the Tauri shell).

mod activity;
mod announcements;
mod chart;
mod ctx;
mod demo;
mod diagnostics;
mod gate;
mod groups;
mod invites;
mod overview;
mod people;
mod settings;
mod shell;

use leptos::prelude::*;
use sl_proto::panel::PanelState;
use sl_ui::{ToastRegion, provide_prefs, provide_toasts};

#[component]
fn App() -> impl IntoView {
    provide_prefs();
    provide_toasts();
    let c = ctx::Ctx::new();
    provide_context(c);
    leptos::task::spawn_local(async move {
        match sl_ui::ipc::invoke0::<PanelState>("panel_state").await {
            Ok(s) => c.state.set(Some(s)),
            Err(e) => sl_ui::toasts().error(e.message),
        }
    });
    view! {
        {move || match c.state.get() {
            None => view! { <sl_ui::SignalLine state="idle" label=""/> <div class="empty" style="height:100vh"><sl_ui::Spinner label="Starting"/></div> }.into_any(),
            Some(s) if s.needs_setup => view! { <sl_ui::SignalLine state="idle" label=""/> <gate::Setup/> }.into_any(),
            Some(s) if !s.unlocked => view! { <sl_ui::SignalLine state="idle" label="Control panel locked"/> <gate::Unlock/> }.into_any(),
            Some(_) => view! { <shell::Shell/> }.into_any(),
        }}
        <ToastRegion/>
    }
}

fn main() {
    if !sl_ui::ipc::in_tauri()
        && let Some(mode) = sl_ui::ipc::demo_requested()
    {
        demo::install(&mode);
    }
    leptos::mount::mount_to_body(App);
}

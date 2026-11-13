//! Root component: routing by phase, global shortcuts, event bridge.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::client_ipc::*;
use sl_ui::{SignalLine, ToastRegion, ipc, provide_prefs, provide_toasts};

use crate::onboarding::{Onboarding, SignIn};
use crate::shell::Shell;
use crate::state::{Store, conn_label, conn_state_key};

/// A link waiting for confirmation before it opens in the browser.
#[derive(Clone, Copy)]
pub struct LinkCtx(pub RwSignal<Option<String>>);

#[component]
pub fn App() -> impl IntoView {
    provide_prefs();
    provide_toasts();
    let store = Store::new();
    provide_context(store);
    let link = LinkCtx(RwSignal::new(None));
    provide_context(link);

    // Initial state, then live events.
    spawn_local(async move {
        if let Ok(s) = ipc::invoke0::<AppState>("app_state").await {
            let ready = s.phase == Phase::Ready;
            store.app.set(s);
            if ready {
                store.load();
            }
        }
    });
    ipc::listen::<ClientEvent>("sl://event", move |e| store.apply(e));
    // Browser preview only: `?demo=chat` opens the first conversation.
    if !ipc::in_tauri() && ipc::demo_requested().as_deref() == Some("chat") {
        set_timeout(
            move || {
                if let Some(first) = store.convs.with_untracked(|v| v.first().map(|c| c.id)) {
                    store.open(first);
                }
            },
            std::time::Duration::from_millis(400),
        );
    }
    ipc::listen::<()>("sl://resync", move |_| store.load());

    // Window focus drives notifications and read receipts.
    let h1 = window_event_listener(leptos::ev::focus, move |_| store.focused.set(true));
    let h2 = window_event_listener(leptos::ev::blur, move |_| store.focused.set(false));
    std::mem::forget((h1, h2));

    // Global shortcuts.
    let h3 = window_event_listener(leptos::ev::keydown, move |e| {
        if store.app.get_untracked().phase != Phase::Ready {
            return;
        }
        let ctrl = e.ctrl_key() || e.meta_key();
        match e.key().as_str() {
            "k" | "K" if ctrl => {
                e.prevent_default();
                store.switcher_open.set(true);
            }
            "n" | "N" if ctrl => {
                e.prevent_default();
                store.new_chat_open.set(true);
            }
            "/" if ctrl => {
                e.prevent_default();
                store.shortcuts_open.set(true);
            }
            "ArrowUp" | "ArrowDown" if e.alt_key() => {
                e.prevent_default();
                let up = e.key() == "ArrowUp";
                let ids: Vec<_> = store
                    .convs
                    .with_untracked(|v| v.iter().map(|c| c.id).collect());
                if ids.is_empty() {
                    return;
                }
                let cur = store
                    .selected
                    .get_untracked()
                    .and_then(|s| ids.iter().position(|x| *x == s));
                let next = match (cur, up) {
                    (None, _) => 0,
                    (Some(i), true) => i.saturating_sub(1),
                    (Some(i), false) => (i + 1).min(ids.len() - 1),
                };
                store.open(ids[next]);
            }
            "F6" => {
                e.prevent_default();
                cycle_regions(e.shift_key());
            }
            _ => {}
        }
    });
    std::mem::forget(h3);

    // Keep the tray tooltip / dock badge in step with unread messages.
    Effect::new(move |_| {
        let n = store.unread_total();
        #[derive(Serialize)]
        struct A {
            count: u32,
        }
        spawn_local(async move {
            let _ = ipc::invoke::<()>("set_unread", A { count: n }).await;
        });
    });

    // Away after 10 minutes without focus.
    Effect::new(move |prev: Option<Option<TimeoutHandle>>| {
        let focused = store.focused.get();
        if let Some(Some(h)) = prev {
            h.clear();
        }
        if focused {
            if store.my_presence.get_untracked() == sl_proto::Presence::Away {
                set_presence(store, sl_proto::Presence::Online);
            }
            None
        } else {
            set_timeout_with_handle(
                move || {
                    if !store.focused.get_untracked()
                        && store.my_presence.get_untracked() == sl_proto::Presence::Online
                    {
                        set_presence(store, sl_proto::Presence::Away);
                    }
                },
                std::time::Duration::from_secs(600),
            )
            .ok()
        }
    });

    let line_state = Signal::derive(move || {
        let a = store.app.get();
        if a.phase == Phase::Ready {
            conn_state_key(&a.conn).to_string()
        } else {
            "idle".to_string()
        }
    });
    let line_label = Signal::derive(move || {
        let a = store.app.get();
        let server = a
            .server
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "the server".into());
        if a.phase == Phase::Ready {
            conn_label(&a.conn, &server)
        } else {
            String::new()
        }
    });

    view! {
        <SignalLine state=line_state label=line_label/>
        {move || match store.app.with(|a| a.phase) {
            Phase::Onboarding => view! { <Onboarding/> }.into_any(),
            Phase::SignedOut => view! { <SignIn/> }.into_any(),
            Phase::Ready => view! { <Shell/> }.into_any(),
        }}
        <LinkConfirm/>
        <ToastRegion/>
    }
}

pub fn set_presence(store: Store, p: sl_proto::Presence) {
    store.my_presence.set(p);
    #[derive(Serialize)]
    struct A {
        presence: sl_proto::Presence,
    }
    spawn_local(async move {
        let _ = ipc::invoke::<()>("set_presence", A { presence: p }).await;
    });
}

/// F6 moves between the rail, the chat list, the messages and the composer.
fn cycle_regions(back: bool) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let ids = ["rail", "chat-list", "timeline", "composer-input"];
    let active = doc.active_element();
    let cur = ids.iter().position(|id| {
        doc.get_element_by_id(id)
            .zip(active.as_ref())
            .is_some_and(|(el, a)| el.contains(Some(a)))
    });
    let n = ids.len();
    for step in 1..=n {
        let idx = match cur {
            Some(c) if back => (c + n - step) % n,
            Some(c) => (c + step) % n,
            None => step - 1,
        };
        if let Some(el) = doc.get_element_by_id(ids[idx]) {
            use wasm_bindgen::JsCast;
            if let Some(h) = el.dyn_ref::<web_sys::HtmlElement>() {
                let target = h
                    .query_selector("[aria-current='true'], [aria-current='page'], button, textarea, [tabindex='0']")
                    .ok()
                    .flatten()
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
                match target {
                    Some(t) if ids[idx] != "composer-input" && ids[idx] != "timeline" => {
                        let _ = t.focus();
                    }
                    _ => {
                        let _ = h.focus();
                    }
                }
                return;
            }
        }
    }
}

#[component]
fn LinkConfirm() -> impl IntoView {
    let LinkCtx(pending) = expect_context::<LinkCtx>();
    let open = RwSignal::new(false);
    // Only write when the value changes: two effects writing each other's signals
    // unconditionally would loop forever.
    Effect::new(move |_| {
        let want = pending.with(|p| p.is_some());
        if open.get_untracked() != want {
            open.set(want);
        }
    });
    Effect::new(move |_| {
        if !open.get() && pending.with_untracked(|p| p.is_some()) {
            pending.set(None);
        }
    });
    let go = move |_| {
        if let Some(url) = pending.get_untracked() {
            #[derive(Serialize)]
            struct A {
                url: String,
            }
            spawn_local(async move {
                let _ = ipc::invoke::<()>("open_link", A { url }).await;
            });
        }
        open.set(false);
    };
    view! {
        <sl_ui::Modal open=open title="Open link in your browser?">
            <div class="modal-body stack">
                <p>"This link was sent in a chat. Only open it if you trust the sender."</p>
                <p class="mono" style="overflow-wrap:anywhere">{move || pending.get().unwrap_or_default()}</p>
            </div>
            <div class="modal-foot">
                <button class="btn quiet" on:click=move |_| open.set(false)>"Cancel"</button>
                <button class="btn" on:click=go>"Open link"</button>
            </div>
        </sl_ui::Modal>
    }
}

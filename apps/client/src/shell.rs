//! The signed-in layout: sidebar, chat list, conversation.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::client_ipc::*;
use sl_proto::{ConversationId, ConversationKind, Presence, Severity};
use sl_ui::{AaButton, Avatar, Icon, Mark, format, ipc, mod_key, presence_label, toasts};
use uuid::Uuid;
use wasm_bindgen::JsCast;

use crate::app::{LinkCtx, set_presence};
use crate::dialogs::{AddPeopleDialog, ConfirmCtx, NewChatDialog, Shortcuts, Switcher};
use crate::people::People;
use crate::settings::Settings;
use crate::state::{Store, View, conn_label, store};

#[component]
pub fn Shell() -> impl IntoView {
    let s = store();
    let confirm = ConfirmCtx::provide();
    // Memos only notify on real changes, so panes don't remount needlessly.
    let view = Memo::new(move |_| s.view.get());
    let has_selection = Memo::new(move |_| s.selected.get().is_some());
    view! {
        <a class="skip-link" href="#main">"Skip to main content"</a>
        <div
            class="app frame"
            class:single=move || view.get() != View::Chats
            data-pane=move || if has_selection.get() { "chat" } else { "list" }
        >
            <Sidebar/>
            <div class="workspace inset" class:split=move || view.get() == View::Chats>
                {move || match view.get() {
                    View::Chats => view! { <ChatList/> <ChatPane/> }.into_any(),
                    View::People => view! { <People/> }.into_any(),
                    View::Settings => view! { <Settings/> }.into_any(),
                }}
            </div>
        </div>
        <NewChatDialog/>
        <Switcher/>
        <Shortcuts/>
        {confirm.view()}
    }
}

// -------------------------------------------------------------------------------------
// Sidebar
// -------------------------------------------------------------------------------------

#[component]
fn Sidebar() -> impl IntoView {
    let s = store();
    let server = Memo::new(move |_| {
        s.app
            .with(|a| a.server.as_ref().map(|x| x.name.clone()))
            .unwrap_or_default()
    });
    let item = move |v: View, icon: &'static str, label: &'static str| {
        view! {
            <button
                class="nav-item"
                title=label
                aria-current=move || (s.view.get() == v).then_some("page")
                on:click=move |_| s.view.set(v)
            >
                <Icon name=icon/>
                <span class="grow truncate">{label}</span>
                {(v == View::Chats).then(|| view! {
                    <Show when=move || { s.unread_total() > 0 }>
                        <span class="badge" aria-label=move || format!("{} unread", s.unread_total())>
                            {move || { let n = s.unread_total(); if n > 99 { "99+".to_string() } else { n.to_string() } }}
                        </span>
                    </Show>
                })}
            </button>
        }
    };
    view! {
        <nav class="side" id="rail" aria-label="Main">
            <div class="side-head">
                <Mark/>
                <span class="grow">
                    <span class="side-title truncate">{move || { let n = server.get(); if n.is_empty() { "SpareLine".to_string() } else { n } }}</span>
                    <span class="side-sub truncate">"Backup chat"</span>
                </span>
                <button class="icon-btn" aria-label="New chat" title="New chat (Ctrl+N)" on:click=move |_| s.new_chat_open.set(true)>
                    <Icon name="edit"/>
                </button>
            </div>
            <button class="side-search" title="Search or jump to" aria-keyshortcuts="Control+K Meta+K" on:click=move |_| s.switcher_open.set(true)>
                <Icon name="search"/>
                <span class="grow">"Search or jump to…"</span>
                <span class="kbd" aria-hidden="true">{format!("{}K", mod_key())}</span>
            </button>
            <div class="side-nav">
                {item(View::Chats, "chat", "Chats")}
                {item(View::People, "people", "People")}
                {item(View::Settings, "settings", "Settings")}
            </div>
            <span class="grow"></span>
            <div class="side-foot">
                <MeMenu/>
                <AaButton align_left=true/>
            </div>
        </nav>
    }
}

#[component]
fn MeMenu() -> impl IntoView {
    let s = store();
    let open = RwSignal::new(false);
    let me = move || s.app.get().me;
    let name = Signal::derive(move || me().map(|m| m.display_name).unwrap_or_default());
    let seed = Signal::derive(move || me().map(|m| m.id.to_string()).unwrap_or_default());
    let presence = Signal::derive(move || {
        if matches!(s.app.get().conn, ConnState::Connected { .. }) {
            s.my_presence.get()
        } else {
            Presence::Offline
        }
    });
    let pick = move |p: Presence| {
        set_presence(s, p);
        open.set(false);
    };
    view! {
        <div class="grow" style="position:relative;min-width:0">
            <button
                class="nav-item me-btn"
                aria-label=move || format!("{}, {}. Change status", name.get(), presence_label(presence.get()))
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <Avatar name=name seed=seed size=1.5 presence=presence/>
                <span class="grow truncate" aria-hidden="true">{move || name.get()}</span>
            </button>
            <Show when=move || open.get()>
                <div style="position:fixed;inset:0;z-index:39" on:click=move |_| open.set(false)></div>
                <div class="popover" role="menu" aria-label="Your status" style="left:0;bottom:calc(100% + 6px);width:15rem" on:keydown=move |e| if e.key() == "Escape" { open.set(false) }>
                    <p style="font-weight:600">{move || name.get()}</p>
                    <p class="hint" style="margin-bottom:var(--s3)">{move || me().map(|m| format!("@{}", m.username)).unwrap_or_default()}</p>
                    <div class="stack" style="gap:var(--s1)">
                        <button class="btn ghost" role="menuitemradio" aria-checked=move || (s.my_presence.get() == Presence::Online).to_string() on:click=move |_| pick(Presence::Online) style="justify-content:flex-start">
                            <span class="presence" data-p="online"></span>"Available"
                        </button>
                        <button class="btn ghost" role="menuitemradio" aria-checked=move || (s.my_presence.get() == Presence::Away).to_string() on:click=move |_| pick(Presence::Away) style="justify-content:flex-start">
                            <span class="presence" data-p="away"></span>"Away"
                        </button>
                    </div>
                </div>
            </Show>
        </div>
    }
}

// -------------------------------------------------------------------------------------
// Chat list
// -------------------------------------------------------------------------------------

#[component]
fn ChatList() -> impl IntoView {
    let s = store();
    let filter = RwSignal::new(String::new());
    let ids = Memo::new(move |_| {
        let f = filter.get().to_lowercase();
        s.convs.with(|v| {
            v.iter()
                .filter(|c| f.is_empty() || c.title.to_lowercase().contains(&f))
                .map(|c| c.id)
                .collect::<Vec<_>>()
        })
    });
    let on_keydown = move |e: leptos::ev::KeyboardEvent| {
        let key = e.key();
        if key != "ArrowDown" && key != "ArrowUp" {
            return;
        }
        e.prevent_default();
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let Ok(list) = doc.query_selector_all("#chat-list .conv") else {
            return;
        };
        let active = doc.active_element();
        let mut idx: Option<u32> = None;
        for i in 0..list.length() {
            if let (Some(n), Some(a)) = (list.item(i), active.as_ref())
                && n.is_same_node(Some(a))
            {
                idx = Some(i);
            }
        }
        let next = match (idx, key.as_str()) {
            (None, _) => 0,
            (Some(i), "ArrowUp") => i.saturating_sub(1),
            (Some(i), _) => (i + 1).min(list.length().saturating_sub(1)),
        };
        if let Some(el) = list
            .item(next)
            .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = el.focus();
        }
    };
    view! {
        <aside class="list-pane" aria-labelledby="chats-title">
            <div class="list-head">
                <h1 id="chats-title">"Chats"</h1>
                <button class="icon-btn" aria-label="New chat" title="New chat (Ctrl+N)" on:click=move |_| s.new_chat_open.set(true)>
                    <Icon name="plus"/>
                </button>
            </div>
            <div class="searchbox" role="search">
                <Icon name="search"/>
                <input
                    class="input"
                    type="search"
                    aria-label="Filter chats"
                    placeholder="Filter"
                    prop:value=move || filter.get()
                    on:input=move |e| filter.set(event_target_value(&e))
                    on:keydown=move |e| if e.key() == "Enter" { s.switcher_open.set(true) }
                />
            </div>
            <nav id="chat-list" class="scroll" aria-label="Conversations" style="flex:1;min-height:0" on:keydown=on_keydown>
                <Show
                    when=move || !ids.get().is_empty()
                    fallback=move || view! {
                        <div class="empty">
                            <h2>{move || if filter.get().is_empty() { "No chats yet" } else { "No matching chats" }}</h2>
                            <p>"Start a chat with someone in your groups."</p>
                            <button class="btn" on:click=move |_| s.new_chat_open.set(true)><Icon name="plus"/>"New chat"</button>
                        </div>
                    }
                >
                    <ul class="conv-list">
                        <For each=move || ids.get() key=|id| *id let:id>
                            <li><ConvRow id=id/></li>
                        </For>
                    </ul>
                </Show>
            </nav>
        </aside>
    }
}

#[component]
fn ConvRow(id: ConversationId) -> impl IntoView {
    let s = store();
    let c = Memo::new(move |_| s.conversation(id));
    let other = move || {
        let me = s.me();
        c.get().and_then(|c| {
            (c.kind == ConversationKind::Direct)
                .then(|| c.members.into_iter().find(|m| Some(m.id) != me))
                .flatten()
        })
    };
    let presence = Signal::derive(move || other().map(|o| s.presence_of(o.id)).unwrap_or_default());
    let title = Signal::derive(move || c.get().map(|c| c.title).unwrap_or_default());
    let seed = Signal::derive(move || {
        other()
            .map(|o| o.id.to_string())
            .unwrap_or_else(|| id.to_string())
    });
    let is_group = move || c.get().is_some_and(|c| c.kind == ConversationKind::Group);
    let label = move || {
        let Some(c) = c.get() else {
            return String::new();
        };
        let mut l = c.title.clone();
        if c.unread > 0 {
            l.push_str(&format!(", {} unread", c.unread));
        }
        if let Some(m) = &c.last_message {
            l.push_str(&format!(
                ". {}{}",
                if m.sender_name.is_empty() {
                    String::new()
                } else {
                    format!("{}: ", m.sender_name)
                },
                m.text
            ));
        }
        l
    };
    view! {
        <button
            class="conv"
            class:unread=move || c.get().is_some_and(|c| c.unread > 0)
            class:readonly=move || c.get().is_some_and(|c| c.read_only)
            aria-current=move || (s.selected.get() == Some(id)).then_some("true")
            aria-label=label
            on:click=move |_| s.open(id)
        >
            {move || if is_group() {
                view! { <Avatar name=title seed=seed group=true size=2.0/> }.into_any()
            } else {
                view! { <Avatar name=title seed=seed presence=presence size=2.0/> }.into_any()
            }}
            <span class="conv-title">{move || title.get()}</span>
            <span class="conv-when">{move || c.get().and_then(|c| c.last_message.map(|m| format::short_when(m.ts))).unwrap_or_default()}</span>
            <span class="conv-preview">
                {move || c.get().and_then(|c| c.last_message).map(|m| {
                    if m.sender_name.is_empty() { m.text } else { format!("{}: {}", m.sender_name, m.text) }
                }).unwrap_or_else(|| "No messages yet".into())}
            </span>
            {move || c.get().filter(|c| c.unread > 0).map(|c| view! { <span class="badge" aria-hidden="true">{c.unread.min(99)}</span> })}
        </button>
    }
}

// -------------------------------------------------------------------------------------
// Banners
// -------------------------------------------------------------------------------------

#[component]
pub fn Banners() -> impl IntoView {
    let s = store();
    let broadcasts = move || {
        let dismissed = s.dismissed.get();
        s.app
            .get()
            .broadcasts
            .into_iter()
            .filter(|b| b.severity == Severity::Critical || !dismissed.contains(&b.id))
            .collect::<Vec<_>>()
    };
    let conn = move || s.app.get().conn;
    let retry = move |_| {
        spawn_local(async move {
            let _ = ipc::invoke0::<()>("reconnect_now").await;
        });
    };
    view! {
        <For each=broadcasts key=|b| b.id let:b>
            {
                let level = match b.severity { Severity::Info => "info", Severity::Warning => "warning", Severity::Critical => "critical" };
                let id = b.id;
                let critical = b.severity == Severity::Critical;
                view! {
                    <div class="banner" data-level=level role="status">
                        <Icon name="megaphone"/>
                        <div class="grow">
                            <strong>{b.title.clone()}</strong>
                            {(!b.body.is_empty()).then(|| view! { <p>{b.body.clone()}</p> })}
                            <p class="xs muted">{format!("Announcement from your admin, {}", format::short_when(b.created_at))}</p>
                        </div>
                        {(!critical).then(|| view! {
                            <button class="icon-btn" aria-label="Dismiss announcement" on:click=move |_| s.dismissed.update(|d| { d.insert(id); })>
                                <Icon name="close"/>
                            </button>
                        })}
                    </div>
                }
            }
        </For>
        <Show when=move || !matches!(conn(), ConnState::Connected { .. })>
            <div class="banner" data-level="warning" role="status">
                <Icon name="offline"/>
                <div class="grow">
                    <strong>{move || conn_label(&conn(), &s.app.get().server.map(|x| x.name).unwrap_or_else(|| "the server".into()))}</strong>
                    <p class="small">"You can keep writing. Messages send as soon as the connection is back."</p>
                </div>
                <button class="btn quiet small" on:click=retry>"Retry now"</button>
                <button class="btn ghost small" on:click=move |_| { s.settings_section.set(Some("connection")); s.view.set(View::Settings); }>"Details"</button>
            </div>
        </Show>
    }
}

// -------------------------------------------------------------------------------------
// Conversation
// -------------------------------------------------------------------------------------

#[component]
fn ChatPane() -> impl IntoView {
    let s = store();
    let selected = Memo::new(move |_| s.selected.get());
    move || {
        match selected.get() {
        Some(id) => view! { <Conversation id=id/> }.into_any(),
        None => view! {
            <main class="chat" id="main" tabindex="-1">
                <Banners/>
                <div class="pane-empty">
                    <div class="lines" aria-hidden="true"><i></i><i></i></div>
                    <h2>"Pick a chat, or start a new one"</h2>
                    <p>"Everything you send is encrypted on this computer before it leaves."</p>
                    <div class="row" style="justify-content:center">
                        <button class="btn" on:click=move |_| s.new_chat_open.set(true)><Icon name="plus"/>"New chat"</button>
                        <button class="btn quiet" on:click=move |_| s.view.set(View::People)><Icon name="people"/>"Browse people"</button>
                    </div>
                </div>
            </main>
        }
        .into_any(),
    }
    }
}

#[component]
fn Conversation(id: ConversationId) -> impl IntoView {
    let s = store();
    let c = Memo::new(move |_| s.conversation(id));
    let add_open = RwSignal::new(false);
    let confirm = expect_context::<ConfirmCtx>();
    s.ensure_loaded(id);

    // Read receipts: mark read while the window is focused and the chat is open.
    Effect::new(move |_| {
        let unread = c.get().map(|c| c.unread).unwrap_or(0);
        if unread > 0 && s.focused.get() {
            set_timeout(
                move || {
                    if s.focused.get_untracked() && s.selected.get_untracked() == Some(id) {
                        #[derive(Serialize)]
                        struct A {
                            conversation: ConversationId,
                        }
                        spawn_local(async move {
                            let _ = ipc::invoke::<()>("mark_read", A { conversation: id }).await;
                        });
                    }
                },
                std::time::Duration::from_millis(500),
            );
        }
    });

    let other = move || {
        let me = s.me();
        c.get().and_then(|c| {
            (c.kind == ConversationKind::Direct)
                .then(|| c.members.into_iter().find(|m| Some(m.id) != me))
                .flatten()
        })
    };
    let title = Signal::derive(move || c.get().map(|c| c.title).unwrap_or_default());
    let seed = Signal::derive(move || {
        other()
            .map(|o| o.id.to_string())
            .unwrap_or_else(|| id.to_string())
    });
    let presence = Signal::derive(move || other().map(|o| s.presence_of(o.id)).unwrap_or_default());
    let is_group = move || c.get().is_some_and(|c| c.kind == ConversationKind::Group);
    let subtitle = move || {
        let Some(c) = c.get() else {
            return String::new();
        };
        if c.read_only {
            return "Read-only".into();
        }
        match c.kind {
            ConversationKind::Direct => presence_label(presence.get()).to_string(),
            ConversationKind::Group => {
                let n = c.members.len();
                match c.scope_group.map(|g| g.name).filter(|n| !n.is_empty()) {
                    Some(g) => format!("{n} people in {g}"),
                    None => format!("{n} people"),
                }
            }
        }
    };
    let leave = move |_| {
        confirm.ask(
            "Leave this chat?",
            "You'll stop receiving its messages. Someone in the chat can add you back.",
            "Leave chat",
            Callback::new(move |_| {
                #[derive(Serialize)]
                struct A {
                    conversation: ConversationId,
                }
                spawn_local(async move {
                    match ipc::invoke::<()>("leave", A { conversation: id }).await {
                        Ok(()) => s.selected.set(None),
                        Err(e) => toasts().error(e.message),
                    }
                });
            }),
        );
    };

    view! {
        <main class="chat" id="main" tabindex="-1" aria-labelledby="chat-title">
            <Banners/>
            <header class="chat-head">
                <button class="icon-btn back-btn" aria-label="Back to chats" on:click=move |_| s.selected.set(None)><Icon name="chevron-left"/></button>
                {move || if is_group() {
                    view! { <Avatar name=title seed=seed group=true size=1.6/> }.into_any()
                } else {
                    view! { <Avatar name=title seed=seed presence=presence size=1.6/> }.into_any()
                }}
                <div class="grow chat-title">
                    <h2 id="chat-title" class="truncate">{move || title.get()}</h2>
                    <p class="chat-sub truncate">{subtitle}</p>
                </div>
                <Show when=move || is_group() && !c.get().is_some_and(|c| c.read_only)>
                    <button class="btn quiet small" on:click=move |_| add_open.set(true)><Icon name="invite"/>"Add people"</button>
                    <button class="icon-btn" aria-label="Leave chat" title="Leave chat" on:click=leave><Icon name="logout"/></button>
                </Show>
            </header>
            <Timeline id=id/>
            <TypingLine id=id/>
            {move || if c.get().is_some_and(|c| c.read_only) {
                view! {
                    <div class="composer"><div class="composer-inner"><p class="readonly-note">
                        "This chat is read-only. You no longer share a group with this person, or you were removed. Your admin manages groups."
                    </p></div></div>
                }.into_any()
            } else {
                view! { <Composer id=id/> }.into_any()
            }}
            <AddPeopleDialog open=add_open conversation=id/>
        </main>
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct RowKey {
    id: Uuid,
    cont: bool,
    day: Option<String>,
}

fn build_rows(v: &[MessageView]) -> Vec<RowKey> {
    let mut out = Vec::with_capacity(v.len());
    let mut prev: Option<&MessageView> = None;
    for m in v {
        let day = match prev {
            Some(p) if format::same_day(p.ts, m.ts) => None,
            _ => Some(format::day_label(m.ts)),
        };
        let cont = day.is_none()
            && m.kind == MessageKind::Text
            && prev.is_some_and(|p| {
                p.kind == MessageKind::Text && p.sender == m.sender && m.ts - p.ts < 5 * 60_000
            });
        out.push(RowKey {
            id: m.id,
            cont,
            day,
        });
        prev = Some(m);
    }
    out
}

#[component]
fn Timeline(id: ConversationId) -> impl IntoView {
    let s = store();
    let rows = Memo::new(move |_| s.msgs.with(|m| m.get(&id).map(|v| build_rows(v))));
    let complete = move || s.complete.with(|c| c.contains(&id));
    let el: NodeRef<leptos::html::Div> = NodeRef::new();
    let near_bottom = StoredValue::new(true);
    let anchor = StoredValue::new(None::<i32>);

    // Messages off-screen are laid out lazily (`content-visibility: auto`), so heights
    // settle a moment after the first scroll. Re-pin to the bottom a couple of times
    // unless the person has scrolled up in the meantime.
    let pin_bottom = move || {
        if let Some(d) = el.get_untracked()
            && near_bottom.get_value()
        {
            d.set_scroll_top(d.scroll_height());
        }
    };
    Effect::new(move |_| {
        rows.track();
        request_animation_frame(move || {
            let Some(d) = el.get_untracked() else { return };
            if let Some(a) = anchor.get_value() {
                d.set_scroll_top(d.scroll_height() - a);
                anchor.set_value(None);
            } else if near_bottom.get_value() {
                d.set_scroll_top(d.scroll_height());
                set_timeout(pin_bottom, std::time::Duration::from_millis(80));
                set_timeout(pin_bottom, std::time::Duration::from_millis(320));
            }
        });
    });

    let older = move || {
        if let Some(d) = el.get_untracked() {
            anchor.set_value(Some(d.scroll_height() - d.scroll_top()));
        }
        s.load_older(id);
    };
    let on_scroll = move |_| {
        let Some(d) = el.get_untracked() else { return };
        near_bottom.set_value(d.scroll_height() - d.scroll_top() - d.client_height() < 120);
        if d.scroll_top() < 60
            && !s.complete.with_untracked(|c| c.contains(&id))
            && anchor.get_value().is_none()
        {
            older();
        }
    };

    // Arrow keys move between messages inside the log.
    let on_keydown = move |e: leptos::ev::KeyboardEvent| {
        let k = e.key();
        if k != "ArrowUp" && k != "ArrowDown" && k != "Home" && k != "End" {
            return;
        }
        let Some(d) = el.get_untracked() else { return };
        let Ok(list) = d.query_selector_all(".msg[tabindex]") else {
            return;
        };
        let n = list.length();
        if n == 0 {
            return;
        }
        let active = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|doc| doc.active_element());
        let mut idx: Option<u32> = None;
        for i in 0..n {
            if let (Some(node), Some(a)) = (list.item(i), active.as_ref())
                && node.contains(Some(a))
            {
                idx = Some(i);
            }
        }
        let next = match (k.as_str(), idx) {
            ("Home", _) => 0,
            ("End", _) | (_, None) => n - 1,
            ("ArrowUp", Some(i)) => i.saturating_sub(1),
            (_, Some(i)) => (i + 1).min(n - 1),
        };
        e.prevent_default();
        if let Some(t) = list
            .item(next)
            .and_then(|x| x.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = t.focus();
        }
    };

    view! {
        <div class="timeline scroll" id="timeline" node_ref=el tabindex="0" role="log" aria-label="Messages" aria-live="polite" aria-relevant="additions" on:scroll=on_scroll on:keydown=on_keydown>
            <div class="timeline-inner">
                {move || if complete() {
                    view! {
                        <div class="beginning">
                            <span class="lock"><Icon name="lock"/>"Messages here are end-to-end encrypted. Only the people in this chat can read them."</span>
                        </div>
                    }.into_any()
                } else {
                    view! { <div class="older"><button class="btn ghost small" on:click=move |_| older()>"Load earlier messages"</button></div> }.into_any()
                }}
                {move || match rows.get() {
                    None => view! { <div class="empty"><sl_ui::Spinner label="Loading messages"/></div> }.into_any(),
                    Some(r) if r.is_empty() => view! {
                        <div class="empty"><p>"No messages yet. Say hello."</p></div>
                    }.into_any(),
                    Some(_) => view! {
                        <For each=move || rows.get().unwrap_or_default() key=|r| r.clone() let:r>
                            {r.day.clone().map(|d| view! { <div class="day" role="separator">{d}</div> })}
                            <MessageRow conv=id mid=r.id cont=r.cont/>
                        </For>
                    }.into_any(),
                }}
            </div>
        </div>
    }
}

#[component]
fn MessageRow(conv: ConversationId, mid: Uuid, cont: bool) -> impl IntoView {
    let s = store();
    let m = Memo::new(move |_| {
        s.msgs.with(|map| {
            map.get(&conv)
                .and_then(|v| v.iter().find(|x| x.id == mid).cloned())
        })
    });
    move || m.get().map(|m| render_message(s, m, cont))
}

fn render_message(s: Store, m: MessageView, cont: bool) -> AnyView {
    if m.kind == MessageKind::System {
        return view! { <div class="msg system" role="note"><span class="sys">{m.body.clone()}</span></div> }.into_any();
    }
    let LinkCtx(link) = expect_context::<LinkCtx>();
    let confirm = expect_context::<ConfirmCtx>();
    let picking = RwSignal::new(false);
    let (conv, id) = (m.conversation, m.id);
    let who = if m.mine {
        "You".to_string()
    } else {
        m.sender_name.clone()
    };
    let when = format::time(m.ts);
    let full = format::full(m.ts);
    let aria = format!("{who}, {}", full);
    let name_sig = Signal::stored(m.sender_name.clone());
    let seed_sig = Signal::stored(m.sender.map(|u| u.to_string()).unwrap_or_default());
    let presence = Signal::derive(move || m.sender.map(|u| s.presence_of(u)).unwrap_or_default());
    let mv = m.clone();
    let react = move |emoji: String| {
        picking.set(false);
        #[derive(Serialize)]
        struct A {
            conversation: ConversationId,
            id: Uuid,
            emoji: String,
        }
        spawn_local(async move {
            if let Err(e) = ipc::invoke::<()>(
                "react",
                A {
                    conversation: conv,
                    id,
                    emoji,
                },
            )
            .await
            {
                toasts().error(e.message);
            }
        });
    };
    let reply = {
        let mv = mv.clone();
        move |_| {
            s.editing.set(None);
            s.reply_to.set(Some(mv.clone()));
        }
    };
    let edit = {
        let mv = mv.clone();
        move |_| {
            s.reply_to.set(None);
            s.editing.set(Some(mv.clone()));
        }
    };
    let delete = move |_| {
        confirm.ask(
            "Delete this message?",
            "It will be removed for everyone in the chat.",
            "Delete",
            Callback::new(move |_| {
                #[derive(Serialize)]
                struct A {
                    conversation: ConversationId,
                    id: Uuid,
                }
                spawn_local(async move {
                    if let Err(e) = ipc::invoke::<()>(
                        "delete",
                        A {
                            conversation: conv,
                            id,
                        },
                    )
                    .await
                    {
                        toasts().error(e.message);
                    }
                });
            }),
        );
    };
    let on_link = Callback::new(move |url: String| link.set(Some(url)));
    let status = (m.mine).then(|| match m.status {
        SendStatus::Pending => view! { <span title="Sending"><Icon name="clock"/><span class="sr-only">"Sending"</span></span> }.into_any(),
        SendStatus::Sent => view! { <span title="Sent"><Icon name="check"/><span class="sr-only">"Sent"</span></span> }.into_any(),
        SendStatus::Read => view! { <span class="read" title="Read"><Icon name="check2"/><span class="sr-only">"Read"</span></span> }.into_any(),
        SendStatus::Failed => view! { <span class="failed"><Icon name="alert"/>"Not sent"</span> }.into_any(),
    });
    let can_edit = m.mine && !m.deleted && m.status != SendStatus::Failed;
    let reactions = m.reactions.clone();
    view! {
        <article class="msg" class:mine=m.mine class:cont=cont class:deleted=m.deleted tabindex="-1" aria-label=aria>
            {if cont {
                view! { <span class="gutter"><time class="gutter-time" title=full.clone()>{when.clone()}</time></span> }.into_any()
            } else if m.mine {
                view! { <span class="gutter"><Avatar name=name_sig seed=seed_sig size=1.75/></span> }.into_any()
            } else {
                view! { <span class="gutter"><Avatar name=name_sig seed=seed_sig presence=presence size=1.75/></span> }.into_any()
            }}
            <div class="msg-col">
                {(!cont).then(|| view! {
                    <div class="msg-meta">
                        <span class="who">{if m.sender_name.is_empty() { who.clone() } else { m.sender_name.clone() }}</span>
                        {m.mine.then(|| view! { <span class="you">"You"</span> })}
                        <time title=full.clone()>{when.clone()}</time>
                    </div>
                })}
                {m.reply_to.clone().map(|r| view! {
                    <span class="quote"><strong>{r.sender_name}</strong>": "{r.text}</span>
                })}
                <div class="bubble">
                    {if m.deleted {
                        "This message was deleted.".into_any()
                    } else {
                        crate::markdown::render(&m.body, on_link).into_any()
                    }}
                </div>
                {(!reactions.is_empty()).then(|| view! {
                    <div class="reactions">
                        {reactions.into_iter().map(|r| {
                            let e = r.emoji.clone();
                            let label = format!("{} {}, by {}", r.emoji, r.count, r.names.join(", "));
                            view! {
                                <button class="reaction" aria-pressed=r.mine.to_string() aria-label=label.clone() title=label on:click=move |_| react(e.clone())>
                                    <span aria-hidden="true">{r.emoji.clone()}</span>
                                    <span aria-hidden="true">{r.count}</span>
                                </button>
                            }
                        }).collect_view()}
                    </div>
                })}
                {(m.mine || m.edited).then(|| view! {
                    <div class="msg-foot">
                        {m.edited.then(|| view! { <span>"Edited"</span> })}
                        {status}
                    </div>
                })}
            </div>
            {(!m.deleted && m.status != SendStatus::Pending).then(|| view! {
                <div class="msg-actions" role="toolbar" aria-label="Message actions">
                    <Show
                        when=move || picking.get()
                        fallback=move || view! {
                            <button class="icon-btn" aria-label="Add reaction" title="React" on:click=move |_| picking.set(true)><Icon name="smile"/></button>
                        }
                    >
                        <div class="react-pick" role="group" aria-label="Choose a reaction">
                            {sl_proto_quick().into_iter().map(|e| {
                                let e2 = e.to_string();
                                view! { <button aria-label=format!("React with {e}") on:click=move |_| react(e2.clone())>{e}</button> }
                            }).collect_view()}
                        </div>
                    </Show>
                    <button class="icon-btn" aria-label="Reply" title="Reply" on:click=reply.clone()><Icon name="reply"/></button>
                    {can_edit.then(|| view! {
                        <button class="icon-btn" aria-label="Edit" title="Edit" on:click=edit.clone()><Icon name="edit"/></button>
                        <button class="icon-btn" aria-label="Delete" title="Delete" on:click=delete><Icon name="trash"/></button>
                    })}
                </div>
            })}
        </article>
    }
    .into_any()
}

fn sl_proto_quick() -> [&'static str; 6] {
    ["👍", "❤️", "😂", "😮", "😢", "🎉"]
}

#[component]
fn TypingLine(id: ConversationId) -> impl IntoView {
    let s = store();
    let text = move || {
        let now = format::now();
        let names: Vec<String> = s.typing.with(|t| {
            t.get(&id)
                .map(|v| {
                    v.iter()
                        .filter(|(_, _, u)| *u > now)
                        .map(|(_, n, _)| n.clone())
                        .collect()
                })
                .unwrap_or_default()
        });
        match names.as_slice() {
            [] => String::new(),
            [a] => format!("{a} is typing…"),
            [a, b] => format!("{a} and {b} are typing…"),
            _ => "Several people are typing…".into(),
        }
    };
    view! { <div class="typing" aria-live="polite"><div class="typing-inner">{text}</div></div> }
}

#[component]
fn Composer(id: ConversationId) -> impl IntoView {
    let s = store();
    let text = RwSignal::new(String::new());
    let ta: NodeRef<leptos::html::Textarea> = NodeRef::new();
    let last_typing = StoredValue::new(0i64);
    let title = move || s.conversation(id).map(|c| c.title).unwrap_or_default();

    let autosize = move || {
        if let Some(t) = ta.get_untracked() {
            let he: &web_sys::HtmlElement = t.as_ref();
            let st = he.style();
            let _ = st.set_property("height", "auto");
            let _ = st.set_property("height", &format!("{}px", t.scroll_height().min(320)));
        }
    };
    let focus = move || {
        if let Some(t) = ta.get_untracked() {
            let _ = t.focus();
        }
    };

    Effect::new(move |_| {
        if let Some(m) = s.editing.get()
            && m.conversation == id
        {
            text.set(m.body.clone());
            request_animation_frame(move || {
                autosize();
                focus();
            });
        }
    });
    Effect::new(move |_| {
        if s.reply_to.get().is_some_and(|m| m.conversation == id) {
            request_animation_frame(focus);
        }
    });
    request_animation_frame(focus);

    let submit = move || {
        let body = text.get_untracked();
        if body.trim().is_empty() {
            return;
        }
        let editing = s.editing.get_untracked().filter(|m| m.conversation == id);
        let reply_to = s
            .reply_to
            .get_untracked()
            .filter(|m| m.conversation == id)
            .map(|m| m.id);
        text.set(String::new());
        s.reply_to.set(None);
        s.editing.set(None);
        request_animation_frame(autosize);
        spawn_local(async move {
            if let Some(m) = editing {
                #[derive(Serialize)]
                struct A {
                    conversation: ConversationId,
                    id: Uuid,
                    body: String,
                }
                if let Err(e) = ipc::invoke::<()>(
                    "edit",
                    A {
                        conversation: id,
                        id: m.id,
                        body,
                    },
                )
                .await
                {
                    toasts().error(e.message);
                }
            } else {
                #[derive(Serialize)]
                struct A {
                    input: SendInput,
                }
                match ipc::invoke::<MessageView>(
                    "send",
                    A {
                        input: SendInput {
                            conversation: id,
                            body: body.clone(),
                            reply_to,
                        },
                    },
                )
                .await
                {
                    Ok(v) => {
                        s.msgs.update(|map| {
                            let list = map.entry(id).or_default();
                            if !list.iter().any(|x| x.id == v.id) {
                                list.push(v);
                            }
                        });
                    }
                    Err(e) => {
                        text.set(body);
                        toasts().error(e.message);
                    }
                }
            }
        });
    };

    let on_keydown = move |e: leptos::ev::KeyboardEvent| match e.key().as_str() {
        "Enter" if !e.shift_key() && !e.is_composing() => {
            e.prevent_default();
            submit();
        }
        "Escape" => {
            if s.editing.get_untracked().is_some() {
                text.set(String::new());
            }
            s.editing.set(None);
            s.reply_to.set(None);
        }
        "ArrowUp" if text.get_untracked().is_empty() => {
            let last = s.msgs.with_untracked(|m| {
                m.get(&id).and_then(|v| {
                    v.iter()
                        .rev()
                        .find(|x| x.mine && !x.deleted && x.kind == MessageKind::Text)
                        .cloned()
                })
            });
            if let Some(m) = last {
                e.prevent_default();
                s.editing.set(Some(m));
            }
        }
        _ => {}
    };
    let on_input = move |e: leptos::ev::Event| {
        text.set(event_target_value(&e));
        autosize();
        let now = format::now();
        if s.editing.get_untracked().is_none() && now - last_typing.get_value() > 3000 {
            last_typing.set_value(now);
            #[derive(Serialize)]
            struct A {
                conversation: ConversationId,
            }
            spawn_local(async move {
                let _ = ipc::invoke::<()>("typing", A { conversation: id }).await;
            });
        }
    };

    let chip = move || {
        if let Some(m) = s.editing.get().filter(|m| m.conversation == id) {
            let _ = m;
            Some(view! {
                <div class="composer-chip" role="status">
                    <Icon name="edit"/>
                    <span class="grow">"Editing your message. Esc cancels."</span>
                    <button type="button" class="icon-btn" aria-label="Cancel editing" on:click=move |_| { s.editing.set(None); text.set(String::new()); }><Icon name="close"/></button>
                </div>
            }.into_any())
        } else {
            s.reply_to.get().filter(|m| m.conversation == id).map(|m| view! {
                <div class="composer-chip" role="status">
                    <Icon name="reply"/>
                    <span class="grow truncate">"Replying to "<strong>{m.sender_name.clone()}</strong>": "{m.body.chars().take(100).collect::<String>()}</span>
                    <button type="button" class="icon-btn" aria-label="Cancel reply" on:click=move |_| s.reply_to.set(None)><Icon name="close"/></button>
                </div>
            }.into_any())
        }
    };

    view! {
        <form class="composer" on:submit=move |e| { e.prevent_default(); submit(); }>
            <div class="composer-inner">
                {chip}
                <div class="composer-box">
                    <textarea
                        id="composer-input"
                        node_ref=ta
                        rows="1"
                        aria-label=move || format!("Message {}", title())
                        placeholder=move || format!("Message {}", title())
                        prop:value=move || text.get()
                        on:input=on_input
                        on:keydown=on_keydown
                    ></textarea>
                    <button type="submit" class="btn send" aria-label="Send" title="Send" disabled=move || text.get().trim().is_empty()>
                        <Icon name="send"/>
                    </button>
                </div>
                <p class="hint">"Enter sends. Shift+Enter adds a line. Up edits your last message."</p>
            </div>
        </form>
    }
}

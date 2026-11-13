//! Dialogs: confirm, new chat, quick switcher, add people, shortcuts.

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Serialize;
use sl_proto::client_ipc::*;
use sl_proto::{ConversationId, ConversationKind, Directory, GroupId, UserId};
use sl_ui::{Avatar, Icon, Modal, format, ipc, toasts};

use crate::state::store;

// -------------------------------------------------------------------------------------
// Confirm
// -------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct ConfirmCtx {
    open: RwSignal<bool>,
    title: RwSignal<String>,
    body: RwSignal<String>,
    action: RwSignal<String>,
    cb: StoredValue<Option<Callback<()>>>,
}

impl ConfirmCtx {
    pub fn provide() -> Self {
        let c = Self {
            open: RwSignal::new(false),
            title: RwSignal::new(String::new()),
            body: RwSignal::new(String::new()),
            action: RwSignal::new(String::new()),
            cb: StoredValue::new(None),
        };
        provide_context(c);
        c
    }

    pub fn ask(&self, title: &str, body: &str, action: &str, cb: Callback<()>) {
        self.title.set(title.into());
        self.body.set(body.into());
        self.action.set(action.into());
        self.cb.set_value(Some(cb));
        self.open.set(true);
    }

    pub fn view(self) -> impl IntoView {
        let c = self;
        view! {
            <Modal open=c.open title=Signal::derive(move || c.title.get())>
                <div class="modal-body"><p>{move || c.body.get()}</p></div>
                <div class="modal-foot">
                    <button class="btn quiet" on:click=move |_| c.open.set(false)>"Cancel"</button>
                    <button class="btn danger" on:click=move |_| {
                        c.open.set(false);
                        if let Some(cb) = c.cb.get_value() { cb.run(()); }
                    }>{move || c.action.get()}</button>
                </div>
            </Modal>
        }
    }
}

// -------------------------------------------------------------------------------------
// New chat
// -------------------------------------------------------------------------------------

fn refresh_directory() {
    let s = store();
    spawn_local(async move {
        if let Ok(d) = ipc::invoke0::<Directory>("directory").await {
            s.directory.set(d);
        }
    });
}

async fn start(input: StartChatInput) -> Result<ConversationView, UiError> {
    #[derive(Serialize)]
    struct A {
        input: StartChatInput,
    }
    ipc::invoke::<ConversationView>("start_chat", A { input }).await
}

#[component]
pub fn NewChatDialog() -> impl IntoView {
    let s = store();
    let open = s.new_chat_open;
    let group_mode = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let scope = RwSignal::new(None::<GroupId>);
    let title = RwSignal::new(String::new());
    let picked = RwSignal::new(BTreeSet::<UserId>::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        if open.get() {
            refresh_directory();
            query.set(String::new());
            title.set(String::new());
            picked.set(BTreeSet::new());
            error.set(None);
            if scope.get_untracked().is_none() {
                scope.set(
                    s.app
                        .get_untracked()
                        .me
                        .and_then(|m| m.groups.first().map(|g| g.id)),
                );
            }
        }
    });

    let people = move || {
        let q = query.get().to_lowercase();
        let g = scope.get();
        let gm = group_mode.get();
        s.directory.with(|d| {
            d.users
                .iter()
                .filter(|u| {
                    q.is_empty()
                        || u.display_name.to_lowercase().contains(&q)
                        || u.username.contains(&q)
                })
                .filter(|u| !gm || g.is_some_and(|g| u.groups.contains(&g)))
                .cloned()
                .collect::<Vec<_>>()
        })
    };

    let open_direct = move |u: UserId| {
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match start(StartChatInput {
                kind: ConversationKind::Direct,
                members: vec![u],
                scope_group: None,
                title: String::new(),
            })
            .await
            {
                Ok(c) => {
                    s.upsert_conversation(c.clone());
                    s.open(c.id);
                    open.set(false);
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    let create_group = move |_| {
        let members: Vec<UserId> = picked.get_untracked().into_iter().collect();
        if members.is_empty() {
            error.set(Some("Choose at least one person.".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        let input = StartChatInput {
            kind: ConversationKind::Group,
            members,
            scope_group: scope.get_untracked(),
            title: title.get_untracked(),
        };
        spawn_local(async move {
            match start(input).await {
                Ok(c) => {
                    s.upsert_conversation(c.clone());
                    s.open(c.id);
                    open.set(false);
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };

    let groups = move || s.app.get().me.map(|m| m.groups).unwrap_or_default();

    view! {
        <Modal open=open title="New chat">
            <div class="modal-body stack">
                <div class="seg" role="radiogroup" aria-label="Chat type">
                    <label><input type="radio" name="chat-kind" prop:checked=move || !group_mode.get() on:change=move |_| group_mode.set(false)/>"One person"</label>
                    <label><input type="radio" name="chat-kind" prop:checked=move || group_mode.get() on:change=move |_| group_mode.set(true)/>"Group chat"</label>
                </div>
                <Show when=move || group_mode.get()>
                    <div class="field">
                        <label for="nc-group">"Group"</label>
                        <select id="nc-group" class="select" on:change=move |e| { scope.set(event_target_value(&e).parse().ok()); picked.set(BTreeSet::new()); }>
                            {move || groups().into_iter().map(|g| {
                                let id = g.id;
                                view! { <option value=g.id.to_string() selected=move || scope.get() == Some(id)>{g.name}</option> }
                            }).collect_view()}
                        </select>
                        <span class="hint">"Everyone in a group chat must be in this group."</span>
                    </div>
                    <div class="field">
                        <label for="nc-title">"Chat name (optional)"</label>
                        <input id="nc-title" class="input" placeholder="Incident room" prop:value=move || title.get() on:input=move |e| title.set(event_target_value(&e))/>
                    </div>
                </Show>
                <div class="field">
                    <label for="nc-q">{move || if group_mode.get() { "Add people" } else { "Who do you want to message?" }}</label>
                    <input id="nc-q" class="input" type="search" placeholder="Search by name" autofocus prop:value=move || query.get() on:input=move |e| query.set(event_target_value(&e))/>
                    <ul class="pick-list" aria-label="People">
                        <For each=people key=|u| u.id let:u>
                            {
                                let uid = u.id;
                                let name = Signal::stored(u.display_name.clone());
                                let seed = Signal::stored(u.id.to_string());
                                let presence = Signal::derive(move || store().presence_of(uid));
                                view! {
                                    <li>
                                        {move || if group_mode.get() {
                                            view! {
                                                <label class="pick">
                                                    <input type="checkbox" prop:checked=move || picked.get().contains(&uid) on:change=move |e| {
                                                        let on = event_target_checked(&e);
                                                        picked.update(|p| { if on { p.insert(uid); } else { p.remove(&uid); } });
                                                    }/>
                                                    <Avatar name=name seed=seed size=1.9 presence=presence/>
                                                    <span class="grow">{name.get()}</span>
                                                </label>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <button class="pick" disabled=move || busy.get() on:click=move |_| open_direct(uid)>
                                                    <Avatar name=name seed=seed size=1.9 presence=presence/>
                                                    <span class="grow">{name.get()}<span class="hint" style="display:block">{format!("@{}", u.username)}</span></span>
                                                    <Icon name="chat"/>
                                                </button>
                                            }.into_any()
                                        }}
                                    </li>
                                }
                            }
                        </For>
                    </ul>
                    <Show when=move || people().is_empty()>
                        <p class="hint">"Nobody matches. You can only message people who share a group with you."</p>
                    </Show>
                </div>
                <Show when=move || error.get().is_some()>
                    <div class="notice fail" role="alert"><Icon name="alert"/><span>{move || error.get().unwrap_or_default()}</span></div>
                </Show>
            </div>
            <Show when=move || group_mode.get()>
                <div class="modal-foot">
                    <span class="grow hint">{move || format::plural(picked.get().len() as u64, "person chosen", "people chosen")}</span>
                    <button class="btn quiet" on:click=move |_| open.set(false)>"Cancel"</button>
                    <button class="btn" disabled=move || busy.get() on:click=create_group>"Create group chat"</button>
                </div>
            </Show>
        </Modal>
    }
}

// -------------------------------------------------------------------------------------
// Add people to a group chat
// -------------------------------------------------------------------------------------

#[component]
pub fn AddPeopleDialog(open: RwSignal<bool>, conversation: ConversationId) -> impl IntoView {
    let s = store();
    let picked = RwSignal::new(BTreeSet::<UserId>::new());
    let busy = RwSignal::new(false);
    Effect::new(move |_| {
        if open.get() {
            picked.set(BTreeSet::new());
            refresh_directory();
        }
    });
    let candidates = move || {
        let Some(c) = s.conversation(conversation) else {
            return vec![];
        };
        let g = c.scope_group.map(|g| g.id);
        let members: BTreeSet<UserId> = c.members.iter().map(|m| m.id).collect();
        s.directory.with(|d| {
            d.users
                .iter()
                .filter(|u| !members.contains(&u.id) && g.is_some_and(|g| u.groups.contains(&g)))
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let add = move |_| {
        let users: Vec<UserId> = picked.get_untracked().into_iter().collect();
        if users.is_empty() {
            return;
        }
        busy.set(true);
        #[derive(Serialize)]
        struct A {
            conversation: ConversationId,
            users: Vec<UserId>,
        }
        spawn_local(async move {
            match ipc::invoke::<()>(
                "add_people",
                A {
                    conversation,
                    users,
                },
            )
            .await
            {
                Ok(()) => {
                    toasts().info("Added to the chat.");
                    open.set(false);
                }
                Err(e) => toasts().error(e.message),
            }
            busy.set(false);
        });
    };
    view! {
        <Modal open=open title="Add people">
            <div class="modal-body">
                <p class="hint">"You can add people from this chat's group."</p>
                <ul class="pick-list">
                    <For each=candidates key=|u| u.id let:u>
                        {
                            let uid = u.id;
                            view! {
                                <li>
                                    <label class="pick">
                                        <input type="checkbox" on:change=move |e| {
                                            let on = event_target_checked(&e);
                                            picked.update(|p| { if on { p.insert(uid); } else { p.remove(&uid); } });
                                        }/>
                                        <Avatar name=Signal::stored(u.display_name.clone()) seed=Signal::stored(u.id.to_string()) size=1.9/>
                                        <span class="grow">{u.display_name.clone()}</span>
                                    </label>
                                </li>
                            }
                        }
                    </For>
                </ul>
                <Show when=move || candidates().is_empty()>
                    <p class="hint" style="margin-top:var(--s3)">"Everyone in the group is already here."</p>
                </Show>
            </div>
            <div class="modal-foot">
                <button class="btn quiet" on:click=move |_| open.set(false)>"Cancel"</button>
                <button class="btn" disabled=move || busy.get() || picked.get().is_empty() on:click=add>"Add to chat"</button>
            </div>
        </Modal>
    }
}

// -------------------------------------------------------------------------------------
// Quick switcher (Ctrl+K)
// -------------------------------------------------------------------------------------

#[component]
pub fn Switcher() -> impl IntoView {
    let s = store();
    let open = s.switcher_open;
    let q = RwSignal::new(String::new());
    let hits = RwSignal::new(Vec::<SearchHit>::new());
    let seq = StoredValue::new(0u32);
    Effect::new(move |_| {
        if open.get() {
            q.set(String::new());
            hits.set(vec![]);
        }
    });
    Effect::new(move |_| {
        let query = q.get();
        let n = seq.get_value() + 1;
        seq.set_value(n);
        if query.trim().chars().count() < 2 {
            hits.set(vec![]);
            return;
        }
        set_timeout(
            move || {
                if seq.get_value() != n {
                    return;
                }
                #[derive(Serialize)]
                struct A {
                    query: String,
                }
                spawn_local(async move {
                    if let Ok(r) = ipc::invoke::<Vec<SearchHit>>("search", A { query }).await
                        && seq.get_value() == n
                    {
                        hits.set(r);
                    }
                });
            },
            std::time::Duration::from_millis(220),
        );
    });
    let convs = move || {
        let query = q.get().to_lowercase();
        s.convs.with(|v| {
            v.iter()
                .filter(|c| query.is_empty() || c.title.to_lowercase().contains(&query))
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
        })
    };
    let go = move |id: ConversationId| {
        open.set(false);
        s.open(id);
    };
    view! {
        <Modal open=open title="Go to a chat or search messages">
            <div class="modal-body">
                <input class="input" type="search" aria-label="Search" placeholder="Type a name or words from a message" autofocus prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
                <ul class="switcher-list">
                    <li><span class="label">"Chats"</span></li>
                    <For each=convs key=|c| c.id let:c>
                        {
                            let id = c.id;
                            view! {
                                <li><button class="pick" on:click=move |_| go(id)>
                                    <Avatar name=Signal::stored(c.title.clone()) seed=Signal::stored(c.id.to_string()) size=1.8 group=c.kind == ConversationKind::Group/>
                                    <span class="grow">{c.title.clone()}</span>
                                    {(c.unread > 0).then(|| view! { <span class="badge">{c.unread}</span> })}
                                </button></li>
                            }
                        }
                    </For>
                    <Show when=move || !hits.get().is_empty()>
                        <li><span class="label">"Messages"</span></li>
                    </Show>
                    <For each=move || hits.get() key=|h| h.message.id let:h>
                        {
                            let id = h.conversation;
                            view! {
                                <li><button class="pick" on:click=move |_| go(id)>
                                    <span class="grow">
                                        <strong>{h.message.sender_name.clone()}</strong>
                                        " in " {h.conversation_title.clone()}
                                        <span class="hint" style="display:block">{h.message.body.chars().take(140).collect::<String>()}</span>
                                    </span>
                                    <span class="hint">{format::short_when(h.message.ts)}</span>
                                </button></li>
                            }
                        }
                    </For>
                </ul>
                <p class="hint" style="margin-top:var(--s3)">"Search looks through messages stored on this computer."</p>
            </div>
        </Modal>
    }
}

// -------------------------------------------------------------------------------------
// Shortcuts
// -------------------------------------------------------------------------------------

#[component]
pub fn Shortcuts() -> impl IntoView {
    let s = store();
    let rows: [(&str, &str); 10] = [
        ("Ctrl + K", "Go to a chat or search messages"),
        ("Ctrl + N", "Start a new chat"),
        ("Alt + ↑ / ↓", "Previous or next chat"),
        (
            "F6",
            "Move between the menu, chat list, messages and message box",
        ),
        ("↑ / ↓ in messages", "Move between messages"),
        ("Enter", "Send"),
        ("Shift + Enter", "New line"),
        ("↑ in an empty message box", "Edit your last message"),
        ("Ctrl + + / − / 0", "Larger, smaller or default text"),
        ("Ctrl + /", "Show these shortcuts"),
    ];
    view! {
        <Modal open=s.shortcuts_open title="Keyboard shortcuts">
            <div class="modal-body">
                <table class="table">
                    <tbody>
                        {rows.into_iter().map(|(k, d)| view! { <tr><td style="white-space:nowrap"><span class="kbd">{k}</span></td><td>{d}</td></tr> }).collect_view()}
                    </tbody>
                </table>
            </div>
        </Modal>
    }
}

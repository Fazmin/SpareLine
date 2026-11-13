//! Reactive app store fed by engine events.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use sl_proto::client_ipc::*;
use sl_proto::{ConversationId, Directory, Presence, UserId};
use sl_ui::ipc;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Chats,
    People,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyPrefs {
    pub enabled: bool,
    pub preview: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            preview: true,
        }
    }
}

const NOTIFY_KEY: &str = "spareline.notify";

fn load_notify() -> NotifyPrefs {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(NOTIFY_KEY).ok().flatten())
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn save_notify(p: &NotifyPrefs) {
    if let (Some(s), Ok(v)) = (
        web_sys::window().and_then(|w| w.local_storage().ok().flatten()),
        serde_json::to_string(p),
    ) {
        let _ = s.set_item(NOTIFY_KEY, &v);
    }
}

/// Who is typing where: `(user, display name, expires at)`.
pub type TypingMap = HashMap<ConversationId, Vec<(UserId, String, i64)>>;

#[derive(Clone, Copy)]
pub struct Store {
    pub app: RwSignal<AppState>,
    pub convs: RwSignal<Vec<ConversationView>>,
    pub msgs: RwSignal<HashMap<ConversationId, Vec<MessageView>>>,
    pub complete: RwSignal<HashSet<ConversationId>>,
    pub selected: RwSignal<Option<ConversationId>>,
    pub view: RwSignal<View>,
    pub directory: RwSignal<Directory>,
    pub presence: RwSignal<HashMap<UserId, Presence>>,
    pub typing: RwSignal<TypingMap>,
    pub reply_to: RwSignal<Option<MessageView>>,
    pub editing: RwSignal<Option<MessageView>>,
    pub dismissed: RwSignal<HashSet<Uuid>>,
    pub focused: RwSignal<bool>,
    pub my_presence: RwSignal<Presence>,
    pub notify: RwSignal<NotifyPrefs>,
    pub new_chat_open: RwSignal<bool>,
    pub switcher_open: RwSignal<bool>,
    pub shortcuts_open: RwSignal<bool>,
    pub settings_section: RwSignal<Option<&'static str>>,
}

pub fn store() -> Store {
    expect_context::<Store>()
}

impl Store {
    pub fn new() -> Self {
        Self {
            app: RwSignal::new(AppState {
                phase: Phase::Onboarding,
                me: None,
                server: None,
                conn: ConnState::Offline,
                broadcasts: vec![],
            }),
            convs: RwSignal::new(vec![]),
            msgs: RwSignal::new(HashMap::new()),
            complete: RwSignal::new(HashSet::new()),
            selected: RwSignal::new(None),
            view: RwSignal::new(View::Chats),
            directory: RwSignal::new(Directory::default()),
            presence: RwSignal::new(HashMap::new()),
            typing: RwSignal::new(HashMap::new()),
            reply_to: RwSignal::new(None),
            editing: RwSignal::new(None),
            dismissed: RwSignal::new(HashSet::new()),
            focused: RwSignal::new(true),
            my_presence: RwSignal::new(Presence::Online),
            notify: RwSignal::new(load_notify()),
            new_chat_open: RwSignal::new(false),
            switcher_open: RwSignal::new(false),
            shortcuts_open: RwSignal::new(false),
            settings_section: RwSignal::new(None),
        }
    }

    pub fn me(&self) -> Option<UserId> {
        self.app.with_untracked(|a| a.me.as_ref().map(|m| m.id))
    }

    pub fn conversation(&self, id: ConversationId) -> Option<ConversationView> {
        self.convs.with(|v| v.iter().find(|c| c.id == id).cloned())
    }

    pub fn unread_total(&self) -> u32 {
        self.convs.with(|v| v.iter().map(|c| c.unread).sum())
    }

    pub fn presence_of(&self, u: UserId) -> Presence {
        self.presence
            .with(|m| m.get(&u).copied().unwrap_or_default())
    }

    /// Initial load once signed in.
    pub fn load(&self) {
        let s = *self;
        spawn_local(async move {
            if let Ok(list) = ipc::invoke0::<Vec<ConversationView>>("conversations").await {
                s.convs.set(sort(list));
            }
            if let Ok(d) = ipc::invoke0::<Directory>("directory").await {
                s.set_directory(d);
            }
        });
    }

    fn set_directory(&self, d: Directory) {
        self.presence.update(|m| {
            for u in &d.users {
                m.insert(u.id, u.presence);
            }
        });
        self.directory.set(d);
    }

    /// Select a conversation. Writes only what changes: a signal `set` always notifies,
    /// and the chat pane re-renders on `selected`.
    pub fn open(&self, id: ConversationId) {
        if self.selected.get_untracked() != Some(id) {
            self.selected.set(Some(id));
            if self.reply_to.with_untracked(Option::is_some) {
                self.reply_to.set(None);
            }
            if self.editing.with_untracked(Option::is_some) {
                self.editing.set(None);
            }
        }
        if self.view.get_untracked() != View::Chats {
            self.view.set(View::Chats);
        }
        self.ensure_loaded(id);
    }

    /// Load the latest page of messages if we don't have it yet.
    pub fn ensure_loaded(&self, id: ConversationId) {
        let loaded = self.msgs.with_untracked(|m| m.contains_key(&id));
        if !loaded {
            let s = *self;
            spawn_local(async move {
                #[derive(Serialize)]
                struct A {
                    conversation: ConversationId,
                    before: Option<i64>,
                    limit: u32,
                }
                if let Ok(list) = ipc::invoke::<Vec<MessageView>>(
                    "messages",
                    A {
                        conversation: id,
                        before: None,
                        limit: 60,
                    },
                )
                .await
                {
                    let done = list.len() < 60;
                    s.msgs.update(|m| {
                        m.insert(id, list);
                    });
                    if done {
                        s.complete.update(|c| {
                            c.insert(id);
                        });
                    }
                }
            });
        }
    }

    pub fn load_older(&self, id: ConversationId) {
        let before = self
            .msgs
            .with_untracked(|m| m.get(&id).and_then(|v| v.first().map(|x| x.ts)));
        let Some(before) = before else { return };
        let s = *self;
        spawn_local(async move {
            #[derive(Serialize)]
            struct A {
                conversation: ConversationId,
                before: Option<i64>,
                limit: u32,
            }
            if let Ok(list) = ipc::invoke::<Vec<MessageView>>(
                "messages",
                A {
                    conversation: id,
                    before: Some(before),
                    limit: 60,
                },
            )
            .await
            {
                if list.len() < 60 {
                    s.complete.update(|c| {
                        c.insert(id);
                    });
                }
                s.msgs.update(|m| {
                    let v = m.entry(id).or_default();
                    let mut merged = list;
                    merged.append(v);
                    *v = merged;
                });
            }
        });
    }

    pub fn upsert_conversation(&self, c: ConversationView) {
        self.convs.update(|v| {
            match v.iter_mut().find(|x| x.id == c.id) {
                Some(x) => *x = c,
                None => v.push(c),
            }
            let sorted = sort(std::mem::take(v));
            *v = sorted;
        });
    }

    fn upsert_message(&self, m: MessageView) -> bool {
        let mut is_new = false;
        self.msgs.update(|map| {
            let Some(v) = map.get_mut(&m.conversation) else {
                return;
            };
            match v.iter_mut().find(|x| x.id == m.id) {
                Some(x) => *x = m,
                None => {
                    is_new = true;
                    let pos = v
                        .iter()
                        .rposition(|x| x.ts <= m.ts)
                        .map(|p| p + 1)
                        .unwrap_or(0);
                    v.insert(pos, m);
                }
            }
        });
        is_new
    }

    pub fn apply(&self, e: ClientEvent) {
        match e {
            ClientEvent::State(s) => {
                let was_ready = self.app.with_untracked(|a| a.phase == Phase::Ready);
                let ready = s.phase == Phase::Ready;
                self.app.set(s);
                if ready && !was_ready {
                    self.load();
                }
            }
            ClientEvent::Conn(c) => {
                let reconnected = matches!(c, ConnState::Connected { .. })
                    && !self
                        .app
                        .with_untracked(|a| matches!(a.conn, ConnState::Connected { .. }));
                self.app.update(|a| a.conn = c);
                if reconnected {
                    self.load();
                }
            }
            ClientEvent::Conversation(c) => self.upsert_conversation(c),
            ClientEvent::ConversationRemoved { id } => {
                let s = *self;
                spawn_local(async move {
                    if let Ok(list) = ipc::invoke0::<Vec<ConversationView>>("conversations").await {
                        s.convs.set(sort(list));
                    }
                });
                let _ = id;
            }
            ClientEvent::Message(m) => {
                let conv = m.conversation;
                if let Some(sender) = m.sender {
                    self.typing.update(|t| {
                        if let Some(v) = t.get_mut(&conv) {
                            v.retain(|(u, _, _)| *u != sender);
                        }
                    });
                }
                let is_new_from_other = !m.mine && m.kind == MessageKind::Text && !m.deleted;
                let fresh = self.upsert_message(m.clone())
                    || !self.msgs.with_untracked(|x| x.contains_key(&conv));
                if fresh && is_new_from_other && m.ts > sl_ui::format::now() - 60_000 {
                    self.maybe_notify(&m);
                }
            }
            ClientEvent::Typing {
                conversation,
                user,
                name,
            } => {
                let until = sl_ui::format::now() + 5000;
                self.typing.update(|t| {
                    let v = t.entry(conversation).or_default();
                    v.retain(|(u, _, _)| *u != user);
                    v.push((user, name, until));
                });
                let s = *self;
                set_timeout(
                    move || {
                        let now = sl_ui::format::now();
                        s.typing.update(|t| {
                            for v in t.values_mut() {
                                v.retain(|(_, _, until)| *until > now);
                            }
                        });
                    },
                    std::time::Duration::from_millis(5200),
                );
            }
            ClientEvent::Presence { user, presence } => {
                self.presence.update(|m| {
                    m.insert(user, presence);
                });
            }
            ClientEvent::Directory(d) => self.set_directory(d),
            ClientEvent::Broadcasts(b) => self.app.update(|a| a.broadcasts = b),
            ClientEvent::Notice { level, text } => {
                let t = sl_ui::toasts();
                match level {
                    sl_proto::Severity::Info => t.info(text),
                    sl_proto::Severity::Warning => t.warn(text),
                    sl_proto::Severity::Critical => t.error(text),
                }
            }
        }
    }

    fn maybe_notify(&self, m: &MessageView) {
        let prefs = self.notify.get_untracked();
        if !prefs.enabled {
            return;
        }
        let looking = self.focused.get_untracked()
            && self.selected.get_untracked() == Some(m.conversation)
            && self.view.get_untracked() == View::Chats;
        if looking {
            return;
        }
        let conv = self.conversation(m.conversation);
        let title = match conv {
            Some(c) if c.kind == sl_proto::ConversationKind::Group => {
                format!("{} in {}", m.sender_name, c.title)
            }
            _ => m.sender_name.clone(),
        };
        let body = if prefs.preview {
            m.body.chars().take(140).collect::<String>()
        } else {
            "New message".to_string()
        };
        #[derive(Serialize)]
        struct A {
            title: String,
            body: String,
        }
        spawn_local(async move {
            let _ = ipc::invoke::<()>("notify", A { title, body }).await;
        });
    }
}

fn sort(mut v: Vec<ConversationView>) -> Vec<ConversationView> {
    v.sort_by(|a, b| {
        let ta = a
            .last_message
            .as_ref()
            .map(|m| m.ts)
            .unwrap_or(a.updated_at);
        let tb = b
            .last_message
            .as_ref()
            .map(|m| m.ts)
            .unwrap_or(b.updated_at);
        tb.cmp(&ta)
    });
    v
}

/// "Connected", "Reconnecting…" etc. for the line and status text.
pub fn conn_state_key(c: &ConnState) -> &'static str {
    match c {
        ConnState::Connected { .. } => "connected",
        ConnState::Connecting => "connecting",
        ConnState::Reconnecting { .. } => "reconnecting",
        ConnState::Offline => "offline",
    }
}

pub fn conn_label(c: &ConnState, server: &str) -> String {
    match c {
        ConnState::Connected { rtt_ms: Some(ms) } => format!("Connected to {server} ({ms} ms)"),
        ConnState::Connected { rtt_ms: None } => format!("Connected to {server}"),
        ConnState::Connecting => format!("Connecting to {server}…"),
        ConnState::Reconnecting { next_in_ms, .. } => {
            format!(
                "Can't reach {server}. Trying again in {} s.",
                (next_in_ms / 1000).max(1)
            )
        }
        ConnState::Offline => "Not connected".into(),
    }
}

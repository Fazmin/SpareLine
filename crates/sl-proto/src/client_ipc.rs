//! Client IPC DTOs (client Tauri backend ⇄ chat UI).
//!
//! These carry decrypted content, but only between the client's own UI and its own
//! engine on the same device.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Timestamp;
use crate::model::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    Mdns,
    Manual,
    InviteHint,
    Cached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredServer {
    pub name: String,
    pub server_id: Option<Uuid>,
    /// `host:port` candidates, best first.
    pub addresses: Vec<String>,
    pub fingerprint_hint: Option<String>,
    pub source: DiscoverySource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConnState {
    Offline,
    Connecting,
    Connected {
        rtt_ms: Option<u32>,
    },
    Reconnecting {
        attempt: u32,
        next_in_ms: u64,
        last_error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// No server pinned yet: show onboarding.
    Onboarding,
    /// Pinned, but no valid session: show sign-in.
    SignedOut,
    /// Signed in.
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedServerView {
    pub name: String,
    pub server_id: Uuid,
    pub fingerprint: String,
    pub last_address: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppState {
    pub phase: Phase,
    pub me: Option<SelfUser>,
    pub server: Option<PinnedServerView>,
    pub conn: ConnState,
    pub broadcasts: Vec<Broadcast>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinInput {
    /// Pasted invite message, link or bare code.
    pub invite_text: String,
    /// Chosen server address (`host:port`); if empty, hints from the invite / discovery are used.
    pub address: String,
    pub username: String,
    pub display_name: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedeemInput {
    pub invite_text: String,
    pub address: String,
    pub new_password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserLite {
    pub id: UserId,
    pub display_name: String,
    pub username: String,
    pub presence: Presence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePreview {
    pub sender_name: String,
    pub text: String,
    pub ts: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationView {
    pub id: ConversationId,
    pub kind: ConversationKind,
    pub title: String,
    pub scope_group: Option<GroupRef>,
    pub members: Vec<UserLite>,
    pub last_message: Option<MessagePreview>,
    pub unread: u32,
    pub read_only: bool,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendStatus {
    /// Queued locally (offline or in flight).
    Pending,
    /// Accepted by the server.
    Sent,
    /// Read by at least one other member.
    Read,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyPreview {
    pub message_id: Uuid,
    pub sender_name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionView {
    pub emoji: String,
    pub count: u32,
    pub mine: bool,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Text,
    /// "Alex joined", "Sam added a new device"... generated locally from MLS events.
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageView {
    pub id: Uuid,
    pub conversation: ConversationId,
    pub kind: MessageKind,
    pub sender: Option<UserId>,
    pub sender_name: String,
    pub body: String,
    pub ts: Timestamp,
    pub edited: bool,
    pub deleted: bool,
    pub mine: bool,
    pub reply_to: Option<ReplyPreview>,
    pub reactions: Vec<ReactionView>,
    pub status: SendStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartChatInput {
    pub kind: ConversationKind,
    pub members: Vec<UserId>,
    pub scope_group: Option<GroupId>,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendInput {
    pub conversation: ConversationId,
    pub body: String,
    pub reply_to: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub conversation: ConversationId,
    pub conversation_title: String,
    pub message: MessageView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub app_version: String,
    pub platform: String,
    pub server: Option<PinnedServerView>,
    pub conn: ConnState,
    pub last_attempt: Vec<ConnStep>,
    pub discovered: Vec<DiscoveredServer>,
    pub recent_failures: Vec<(String, u32)>,
    pub log: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ClientEvent {
    State(AppState),
    Conn(ConnState),
    Conversation(ConversationView),
    ConversationRemoved {
        id: ConversationId,
    },
    Message(MessageView),
    Typing {
        conversation: ConversationId,
        user: UserId,
        name: String,
    },
    Presence {
        user: UserId,
        presence: Presence,
    },
    Directory(Directory),
    Broadcasts(Vec<Broadcast>),
    /// A user-facing notice (e.g. "Your admin signed you out").
    Notice {
        level: Severity,
        text: String,
    },
}

/// Error shape returned to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiError {
    /// Machine code (e.g. `invalid_invite`, `offline`).
    pub code: String,
    /// Plain-language message to show.
    pub message: String,
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

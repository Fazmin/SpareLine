//! Shared domain types sent over the wire.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Timestamp;
pub use crate::b64::Bytes;

pub type UserId = Uuid;
pub type DeviceId = Uuid;
pub type GroupId = Uuid;
pub type ConversationId = Uuid;

/// Public identity of a SpareLine server (sent before authentication).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub server_id: Uuid,
    pub name: String,
    pub proto: u16,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRef {
    pub id: GroupId,
    pub name: String,
}

/// The signed-in user, as seen by themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfUser {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
    pub groups: Vec<GroupRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Online,
    Away,
    #[default]
    Offline,
}

/// Someone the signed-in user is allowed to see (shares at least one group).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryUser {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
    /// Only the groups shared with the viewer.
    pub groups: Vec<GroupId>,
    pub presence: Presence,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Directory {
    pub groups: Vec<GroupRef>,
    pub users: Vec<DirectoryUser>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Direct,
    Group,
}

/// Server's view of a conversation: routing metadata only. Titles and content live
/// inside MLS and are never known to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationInfo {
    pub id: ConversationId,
    pub kind: ConversationKind,
    /// For group chats: the admin Group the chat is scoped to.
    pub scope_group: Option<GroupId>,
    pub members: Vec<UserId>,
    /// Devices allowed in the MLS group (active devices of current members).
    /// Clients reconcile the MLS tree against this list: any leaf whose device is not
    /// listed is removed by the next member to commit.
    pub devices: Vec<DeviceRef>,
    pub mls_group_id: Bytes,
    pub epoch: u64,
    /// Highest envelope sequence number in this conversation.
    pub last_seq: u64,
    /// True when a direct chat's participants no longer share a group.
    pub read_only: bool,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeKind {
    /// MLS PrivateMessage carrying an application payload.
    Application,
    /// MLS PublicMessage commit.
    Commit,
    /// MLS proposal (by a member or by the server as external sender).
    Proposal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceRef {
    pub user: UserId,
    pub device: DeviceId,
}

/// MLS credential identity for a device: `"<user uuid>:<device uuid>"`.
pub fn credential_identity(user: UserId, device: DeviceId) -> String {
    format!("{user}:{device}")
}

/// Parse a credential identity produced by [`credential_identity`].
pub fn parse_credential_identity(s: &str) -> Option<DeviceRef> {
    let (u, d) = s.split_once(':')?;
    Some(DeviceRef {
        user: u.parse().ok()?,
        device: d.parse().ok()?,
    })
}

/// An MLS message relayed by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub conversation: ConversationId,
    pub seq: u64,
    /// `None` for server-issued (external sender) proposals.
    pub sender_user: Option<UserId>,
    pub sender_device: Option<DeviceId>,
    pub kind: EnvelopeKind,
    pub epoch: u64,
    pub data: Bytes,
    pub ts: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    #[default]
    Info,
    Warning,
    Critical,
}

/// An admin announcement shown as a banner on every client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Broadcast {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub severity: Severity,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadMarker {
    pub conversation: ConversationId,
    pub user: UserId,
    pub seq: u64,
}

/// Describes the device being registered or signed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub platform: String,
    pub client_version: String,
    /// Ed25519 MLS signature public key for this device.
    pub signature_key: Bytes,
}

/// A device's key package as handed to another member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedKeyPackage {
    pub user: UserId,
    pub device: DeviceId,
    pub key_package: Bytes,
}

/// Small, content-free summary a client sends after it had trouble connecting.
/// Shown to the admin in per-user troubleshooting.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClientHealthReport {
    pub client_version: String,
    pub platform: String,
    pub window_start: Timestamp,
    pub window_end: Timestamp,
    /// e.g. `("tls_handshake", 5)`, `("dns_sd_timeout", 2)`.
    pub failures: Vec<(String, u32)>,
    pub last_error: String,
    pub discovery_method: String,
}

pub fn now_ms() -> Timestamp {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

//! WebSocket frames (JSON text frames over TLS).
//!
//! Connection lifecycle:
//! 1. Server sends [`ServerFrame::Hello`] immediately.
//! 2. Unauthenticated, the client may send `Prove` (any number of times), then exactly one
//!    of `Register`, `Redeem`, `Login` or `Resume`.
//! 3. On success the server sends [`ServerFrame::Authed`]; the connection is now bound to
//!    one user and device. Requests use `Req { id, body }` and are answered by
//!    `Res { id, result }`. The server pushes [`Event`]s at any time.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Timestamp;
use crate::error::ApiError;
use crate::model::*;

// ---------------------------------------------------------------------------------------
// Client → server
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "c", rename_all = "snake_case")]
pub enum ClientFrame {
    /// Ask the server to prove it holds the invite whose id is given (see `crypto`).
    Prove {
        invite_id: String,
        nonce: Bytes,
    },
    /// Create a new account with a single-use invite code.
    Register(RegisterRequest),
    /// Use a setup code for an existing account (new device or password reset).
    Redeem(RedeemRequest),
    /// Username + password on a device that already pinned this server.
    Login(LoginRequest),
    /// Reconnect with a stored session token.
    Resume(ResumeRequest),
    /// Authenticated request.
    Req {
        id: u64,
        body: Request,
    },
    /// Ephemeral typing signal. Never stored.
    Typing {
        conversation: ConversationId,
    },
    Ping {
        ts: Timestamp,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub proto: u16,
    pub code: String,
    pub username: String,
    pub display_name: String,
    pub password: String,
    pub device: DeviceInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedeemRequest {
    pub proto: u16,
    pub code: String,
    pub new_password: String,
    pub device: DeviceInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginRequest {
    pub proto: u16,
    pub username: String,
    pub password: String,
    /// Reuse an existing device (keeps its MLS state); `None` registers a new device.
    pub device_id: Option<DeviceId>,
    pub device: DeviceInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeRequest {
    pub proto: u16,
    pub token: String,
    pub device_id: DeviceId,
    pub client_version: String,
}

/// Authenticated requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// People and groups the caller may see.
    Directory,
    /// Conversations the caller belongs to.
    Conversations,
    /// Everything waiting for this device: welcomes and envelopes past its cursors.
    Sync {
        limit: u32,
    },
    /// Advance this device's delivery cursor.
    Ack {
        conversation: ConversationId,
        seq: u64,
    },

    UploadKeyPackages {
        key_packages: Vec<Bytes>,
        last_resort: Option<Bytes>,
    },
    /// One key package for every active device of each user (except the caller's own device).
    ClaimKeyPackages {
        users: Vec<UserId>,
    },

    /// Register a new MLS group the caller already created and populated locally.
    CreateConversation {
        kind: ConversationKind,
        scope_group: Option<GroupId>,
        members: Vec<UserId>,
        mls_group_id: Bytes,
        /// Epoch after the initial add commit was merged.
        epoch: u64,
        welcome: Option<Bytes>,
        welcome_devices: Vec<DeviceId>,
        group_info: Bytes,
    },
    /// A commit for epoch `epoch` (must equal the server's current epoch).
    Commit {
        conversation: ConversationId,
        epoch: u64,
        commit: Bytes,
        welcome: Option<Bytes>,
        welcome_devices: Vec<DeviceId>,
        added_users: Vec<UserId>,
        removed_devices: Vec<DeviceId>,
        group_info: Bytes,
    },
    /// Encrypted application message for epoch `epoch`.
    Send {
        conversation: ConversationId,
        epoch: u64,
        ciphertext: Bytes,
    },
    /// Envelopes after `after_seq` (catch-up). The caller's own envelopes are included
    /// with empty `data` so sequence numbers stay contiguous.
    Fetch {
        conversation: ConversationId,
        after_seq: u64,
        limit: u32,
    },
    /// Latest GroupInfo, for joining by external commit (new device).
    GroupInfo {
        conversation: ConversationId,
    },
    /// Join an existing conversation with an external commit (own new device).
    ExternalJoin {
        conversation: ConversationId,
        epoch: u64,
        commit: Bytes,
        group_info: Bytes,
    },
    /// Delete one of the caller's own envelopes from the server ("delete for everyone").
    Purge {
        conversation: ConversationId,
        seq: u64,
    },
    /// Leave a group conversation (the caller then commits their own removal via others).
    Leave {
        conversation: ConversationId,
    },

    MarkRead {
        conversation: ConversationId,
        seq: u64,
    },
    SetPresence {
        presence: Presence,
    },
    SetDisplayName {
        display_name: String,
    },
    ChangePassword {
        current: String,
        new: String,
    },
    ReportHealth(ClientHealthReport),
    Logout,
}

// ---------------------------------------------------------------------------------------
// Server → client
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "c", rename_all = "snake_case")]
pub enum ServerFrame {
    Hello {
        server: ServerInfo,
    },
    Proof {
        proof: Bytes,
    },
    Authed(Box<AuthOk>),
    AuthError(ApiError),
    Res {
        id: u64,
        result: Result<Response, ApiError>,
    },
    Event(Event),
    Pong {
        ts: Timestamp,
    },
    /// Server is closing this connection (revoked, disabled, shutting down...).
    Close(ApiError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthOk {
    pub user: SelfUser,
    pub device_id: DeviceId,
    /// Present after Register/Redeem/Login; absent after Resume.
    pub session_token: Option<String>,
    pub server: ServerInfo,
    pub broadcasts: Vec<Broadcast>,
    pub key_packages_remaining: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "r", content = "v", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Directory(Directory),
    Conversations(Vec<ConversationInfo>),
    Sync(SyncBatch),
    KeyPackages(Vec<ClaimedKeyPackage>),
    Conversation(ConversationInfo),
    Committed { seq: u64, epoch: u64 },
    Sent { seq: u64, ts: Timestamp },
    Envelopes(Vec<Envelope>),
    GroupInfo { info: Bytes, epoch: u64 },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SyncBatch {
    pub welcomes: Vec<PendingWelcome>,
    pub envelopes: Vec<Envelope>,
    /// True if more is waiting (call Sync again).
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingWelcome {
    pub conversation: ConversationInfo,
    pub welcome: Bytes,
    /// Sequence number at which this device joined; envelopes after it are for the device.
    pub start_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e", content = "v", rename_all = "snake_case")]
pub enum Event {
    Welcome(PendingWelcome),
    Envelope(Envelope),
    /// Membership, epoch or read-only state changed.
    Conversation(ConversationInfo),
    /// The caller was removed from this conversation.
    ConversationRemoved {
        conversation: ConversationId,
    },
    /// The directory changed (group membership, new users...). Refetch it.
    DirectoryChanged,
    Presence {
        user: UserId,
        presence: Presence,
    },
    Typing {
        conversation: ConversationId,
        user: UserId,
    },
    ReadMarker(ReadMarker),
    Broadcast(Broadcast),
    BroadcastEnded {
        id: Uuid,
    },
    KeyPackagesLow {
        remaining: u32,
    },
    Profile(SelfUser),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip() {
        let f = ClientFrame::Req {
            id: 7,
            body: Request::Sync { limit: 100 },
        };
        let s = serde_json::to_string(&f).expect("ser");
        assert_eq!(
            s,
            r#"{"t":"req","c":{"id":7,"body":{"op":"sync","limit":100}}}"#
        );
        let back: ClientFrame = serde_json::from_str(&s).expect("de");
        assert_eq!(back, f);

        let r = ServerFrame::Res {
            id: 7,
            result: Ok(Response::Sent { seq: 3, ts: 10 }),
        };
        let s = serde_json::to_string(&r).expect("ser");
        let back: ServerFrame = serde_json::from_str(&s).expect("de");
        assert_eq!(back, r);

        let e = ServerFrame::Res {
            id: 8,
            result: Err(ApiError::new(crate::ErrorCode::StaleEpoch)),
        };
        let s = serde_json::to_string(&e).expect("ser");
        let back: ServerFrame = serde_json::from_str(&s).expect("de");
        assert_eq!(back, e);
    }
}

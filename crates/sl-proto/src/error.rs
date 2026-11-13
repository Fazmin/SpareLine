//! Typed error codes with plain-language messages for people.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    ProtocolTooOld,
    ProtocolTooNew,
    InvalidInvite,
    InviteExpired,
    InviteUsed,
    UsernameTaken,
    InvalidUsername,
    WeakPassword,
    InvalidCredentials,
    AccountDisabled,
    SessionExpired,
    SessionRevoked,
    NotAuthenticated,
    NotAuthorized,
    NotFound,
    AlreadyExists,
    RateLimited,
    StaleEpoch,
    ReadOnly,
    PinMismatch,
    ServerUnreachable,
    Internal,
    ShuttingDown,
}

impl ErrorCode {
    /// A short, plain-language explanation suitable for showing to a person.
    pub fn user_message(self) -> &'static str {
        match self {
            Self::BadRequest => "The request wasn't understood. Please update SpareLine.",
            Self::ProtocolTooOld => {
                "This version of SpareLine is too old for the server. Please ask your admin for the latest version."
            }
            Self::ProtocolTooNew => {
                "This version of SpareLine is newer than the server. Ask your admin to update the server."
            }
            Self::InvalidInvite => "That invite code isn't valid. Check it and try again.",
            Self::InviteExpired => "That invite code has expired. Ask your admin for a new one.",
            Self::InviteUsed => {
                "That invite code has already been used. Ask your admin for a new one."
            }
            Self::UsernameTaken => "That username is taken. Try another.",
            Self::InvalidUsername => {
                "Usernames use 3–32 letters, numbers, dots, dashes or underscores."
            }
            Self::WeakPassword => "Choose a longer password (at least 10 characters).",
            Self::InvalidCredentials => "Username or password is incorrect.",
            Self::AccountDisabled => "Your account has been disabled. Contact your admin.",
            Self::SessionExpired => "Your session has expired. Please sign in again.",
            Self::SessionRevoked => "You were signed out by your admin. Please sign in again.",
            Self::NotAuthenticated => "Please sign in first.",
            Self::NotAuthorized => "You don't have access to that.",
            Self::NotFound => "That item no longer exists.",
            Self::AlreadyExists => "That already exists.",
            Self::RateLimited => "Too many attempts. Please wait a minute and try again.",
            Self::StaleEpoch => "The chat changed while sending. Retrying…",
            Self::ReadOnly => "This chat is read-only because you no longer share a group.",
            Self::PinMismatch => {
                "The server's identity has changed. This could be an impostor. Contact your admin before continuing."
            }
            Self::ServerUnreachable => "Can't reach the SpareLine server.",
            Self::Internal => "Something went wrong on the server. Your admin can check the logs.",
            Self::ShuttingDown => "The server is restarting. Reconnecting shortly…",
        }
    }
}

/// Error returned over the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    /// Technical detail (never includes message content). Human text comes from
    /// [`ErrorCode::user_message`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// For `StaleEpoch`: the server's current epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
}

impl ApiError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            detail: String::new(),
            epoch: None,
        }
    }
    pub fn with_detail(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            epoch: None,
        }
    }
    pub fn user_message(&self) -> &'static str {
        self.code.user_message()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{:?}", self.code)
        } else {
            write!(f, "{:?}: {}", self.code, self.detail)
        }
    }
}

impl std::error::Error for ApiError {}

impl From<ErrorCode> for ApiError {
    fn from(code: ErrorCode) -> Self {
        Self::new(code)
    }
}

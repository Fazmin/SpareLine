use sl_proto::client_ipc::UiError;
use sl_proto::{ApiError, ErrorCode};

#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    #[error("{}", .0.user_message())]
    Api(ApiError),
    #[error("{0}")]
    Connect(String),
    #[error("You're offline. Messages will send when the connection is back.")]
    Offline,
    #[error("{0}")]
    Invalid(String),
    #[error("Something went wrong: {0}")]
    Internal(String),
}

impl ClientError {
    pub fn code(&self) -> String {
        match self {
            Self::Api(e) => serde_json::to_value(e.code)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "api".into()),
            Self::Connect(_) => "connect".into(),
            Self::Offline => "offline".into(),
            Self::Invalid(_) => "invalid".into(),
            Self::Internal(_) => "internal".into(),
        }
    }

    pub fn api_code(&self) -> Option<ErrorCode> {
        match self {
            Self::Api(e) => Some(e.code),
            _ => None,
        }
    }

    pub fn to_ui(&self) -> UiError {
        UiError {
            code: self.code(),
            message: self.to_string(),
        }
    }
}

impl From<ApiError> for ClientError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl From<anyhow::Error> for ClientError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(format!("{e:#}"))
    }
}

impl From<rusqlite::Error> for ClientError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(e: serde_json::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

pub type ClientResult<T> = Result<T, ClientError>;

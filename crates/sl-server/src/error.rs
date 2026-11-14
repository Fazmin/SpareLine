//! Internal error type. Converts to a client-safe [`ApiError`] at the edges.

use sl_proto::{ApiError, ErrorCode};

#[derive(Debug)]
pub enum AppError {
    /// An expected, client-facing error.
    Api(ApiError),
    /// An unexpected failure. Logged; clients only see `Internal`.
    Internal(anyhow::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    /// The error as a plain phrase for the admin, without the error code.
    pub fn plain(&self) -> String {
        match self {
            Self::Api(a) if !a.detail.is_empty() => a.detail.clone(),
            Self::Api(a) => a.code.user_message().to_string(),
            Self::Internal(e) => format!("{e:#}"),
        }
    }

    pub fn into_api(self) -> ApiError {
        match self {
            Self::Api(e) => e,
            Self::Internal(e) => {
                tracing::error!(error = %format!("{e:#}"), "internal error");
                ApiError::new(ErrorCode::Internal)
            }
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(e) => write!(f, "{e}"),
            Self::Internal(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<ApiError> for AppError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}
impl From<ErrorCode> for AppError {
    fn from(c: ErrorCode) -> Self {
        Self::Api(ApiError::new(c))
    }
}
impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(e.into())
    }
}
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.into())
    }
}
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::Internal(e.into())
    }
}
impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Internal(e.into())
    }
}

/// Shorthand for an API error result.
pub fn api<T>(code: ErrorCode) -> AppResult<T> {
    Err(AppError::Api(ApiError::new(code)))
}

pub fn api_detail<T>(code: ErrorCode, detail: impl Into<String>) -> AppResult<T> {
    Err(AppError::Api(ApiError::with_detail(code, detail)))
}

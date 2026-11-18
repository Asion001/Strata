use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("not signed in")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("range not satisfiable")]
    RangeNotSatisfiable,
    /// The file changed since the client read it (If-Match / If-None-Match).
    /// Carries the current version's ETag, if the file exists.
    #[error("the file was changed by someone else")]
    PreconditionFailed(Option<String>),
    /// A precondition (header) the write needs is missing; says which.
    #[error("{0}")]
    PreconditionRequired(&'static str),
    /// The storage behind this path is offline (see the health probes).
    #[error("{0}")]
    Unavailable(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::BadRequest(msg.into())
}

pub fn conflict(msg: impl Into<String>) -> ApiError {
    ApiError::Conflict(msg.into())
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => ApiError::NotFound,
            sqlx::Error::Database(d) if d.code().as_deref() == Some("23505") => {
                ApiError::Conflict("already exists".into())
            }
            e => ApiError::Internal(e.into()),
        }
    }
}

impl From<opendal::Error> for ApiError {
    fn from(e: opendal::Error) -> Self {
        match e.kind() {
            opendal::ErrorKind::NotFound => ApiError::NotFound,
            opendal::ErrorKind::AlreadyExists => ApiError::Conflict("already exists".into()),
            opendal::ErrorKind::PermissionDenied => ApiError::Forbidden,
            _ => ApiError::Internal(e.into()),
        }
    }
}

impl From<strata_storage::paths::PathError> for ApiError {
    fn from(e: strata_storage::paths::PathError) -> Self {
        ApiError::BadRequest(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::RangeNotSatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
            ApiError::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            ApiError::PreconditionRequired(_) => StatusCode::PRECONDITION_REQUIRED,
            ApiError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        // Never leak internal details (paths, SQL) to clients: they are logged.
        let msg = match &self {
            ApiError::Internal(_) => "internal error".to_string(),
            other => other.to_string(),
        };
        if let ApiError::PreconditionFailed(current) = &self {
            let mut resp = (status, Json(json!({ "error": msg, "etag": current }))).into_response();
            if let Some(et) = current
                .as_deref()
                .and_then(|e| axum::http::HeaderValue::from_str(e).ok())
            {
                resp.headers_mut().insert(axum::http::header::ETAG, et);
            }
            return resp;
        }
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

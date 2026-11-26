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
    #[error("this folder is read-only")]
    ReadOnly,
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
            // Text Postgres can't hold (a NUL character): the input is bad.
            sqlx::Error::Database(d) if d.code().as_deref() == Some("22021") => {
                ApiError::BadRequest("text with a character that can't be stored (NUL)".into())
            }
            e => ApiError::Internal(e.into()),
        }
    }
}

impl From<opendal::Error> for ApiError {
    fn from(e: opendal::Error) -> Self {
        if strata_storage::readonly::is_read_only_refusal(&e) {
            return ApiError::ReadOnly;
        }
        // A path below a file ("not a directory" from a filesystem) is
        // simply not there.
        if not_a_directory(&e) {
            return ApiError::NotFound;
        }
        match e.kind() {
            opendal::ErrorKind::NotFound => ApiError::NotFound,
            opendal::ErrorKind::AlreadyExists => ApiError::Conflict("already exists".into()),
            opendal::ErrorKind::PermissionDenied => ApiError::Forbidden,
            // A pinned read found a newer version (see `vfs::read_stream`).
            opendal::ErrorKind::ConditionNotMatch => {
                ApiError::Conflict("the file changed while being read; try again".into())
            }
            _ => ApiError::Internal(e.into()),
        }
    }
}

/// Does this backend error come from a path that runs through a file?
fn not_a_directory(e: &opendal::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(c) = cur {
        if let Some(io) = c.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::NotADirectory
        {
            return true;
        }
        cur = c.source();
    }
    false
}

/// An error while reading a file's stream, before anything was sent: a
/// file that changed since it was stat'ed (a pinned read found a newer
/// version, or it ended early) is a 409; anything else is internal.
pub fn read_error(e: std::io::Error) -> ApiError {
    let changed = e.kind() == std::io::ErrorKind::UnexpectedEof
        || e.get_ref()
            .and_then(|i| i.downcast_ref::<opendal::Error>())
            .is_some_and(|o| o.kind() == opendal::ErrorKind::ConditionNotMatch);
    if changed {
        ApiError::Conflict("the file changed while being read; try again".into())
    } else {
        ApiError::Internal(e.into())
    }
}

/// A filesystem error from direct local access (outside OpenDAL).
pub fn io_error(e: std::io::Error) -> ApiError {
    if e.kind() == std::io::ErrorKind::ReadOnlyFilesystem {
        ApiError::ReadOnly
    } else {
        ApiError::Internal(e.into())
    }
}

impl From<strata_storage::paths::PathError> for ApiError {
    fn from(e: strata_storage::paths::PathError) -> Self {
        ApiError::BadRequest(e.to_string())
    }
}

impl ApiError {
    /// The HTTP status this error answers with.
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden | ApiError::ReadOnly => StatusCode::FORBIDDEN,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::RangeNotSatisfiable => StatusCode::RANGE_NOT_SATISFIABLE,
            ApiError::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            ApiError::PreconditionRequired(_) => StatusCode::PRECONDITION_REQUIRED,
            ApiError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The message a client sees. Never internal details (paths, SQL):
    /// those go to the log (see `log_internal`).
    pub fn public_message(&self) -> String {
        match self {
            ApiError::Internal(_) => "internal error".to_string(),
            other => other.to_string(),
        }
    }

    /// Log an internal error's details (a client gets none of them).
    pub fn log_internal(&self) {
        if let ApiError::Internal(e) = self {
            tracing::error!("internal error: {e:#}");
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.log_internal();
        let status = self.status();
        let msg = self.public_message();
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

#[cfg(test)]
mod read_error_tests {
    use super::*;

    #[test]
    fn changed_files_are_conflicts() {
        let stale = opendal::Error::new(opendal::ErrorKind::ConditionNotMatch, "412");
        let e = std::io::Error::other(stale);
        assert!(matches!(read_error(e), ApiError::Conflict(_)));
        let short = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "short");
        assert!(matches!(read_error(short), ApiError::Conflict(_)));
        let other = std::io::Error::other(opendal::Error::new(opendal::ErrorKind::Unexpected, "x"));
        assert!(matches!(read_error(other), ApiError::Internal(_)));
    }
}

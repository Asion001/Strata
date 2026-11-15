//! File browser HTTP API.

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde_json::json;
use strata_storage::paths;

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult, bad};
use crate::mounts::{self, Mount};
use crate::state::AppState;
use crate::vfs::{self, Node};

#[derive(Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub download: Option<String>,
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = paths::normalize(&q.path)?;
    let (m, entries) = vfs::list(&st, u.id, &path).await?;
    Ok(Json(json!({
        "path": path,
        "mount": { "id": m.id, "path": m.path, "mode": m.mode, "connection_name": m.connection_name,
                   "connection_kind": m.connection_kind },
        "entries": entries,
    })))
}

/// Parse a single `Range: bytes=a-b` header against `size`.
pub fn parse_range(headers: &HeaderMap, size: u64) -> ApiResult<Option<(u64, u64)>> {
    let Some(v) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let Some(spec) = v.strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None); // multi-range: serve the whole file
    }
    let (a, b) = spec.split_once('-').ok_or(ApiError::RangeNotSatisfiable)?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().map_err(|_| ApiError::RangeNotSatisfiable)?;
        (size.saturating_sub(n), size)
    } else {
        let s: u64 = a.parse().map_err(|_| ApiError::RangeNotSatisfiable)?;
        let e = if b.is_empty() {
            size
        } else {
            b.parse::<u64>()
                .map_err(|_| ApiError::RangeNotSatisfiable)?
                + 1
        };
        (s, e.min(size))
    };
    if start >= end || start >= size {
        return Err(ApiError::RangeNotSatisfiable);
    }
    Ok(Some((start, end)))
}

/// Serve file content with Range, ETag and Content-Disposition support.
pub async fn serve(
    st: &AppState,
    m: &Mount,
    rel: &str,
    node: &Node,
    headers: &HeaderMap,
    download: bool,
) -> ApiResult<Response> {
    if node.is_dir {
        return Err(bad("this is a folder"));
    }
    let size = node.size as u64;
    let etag = node.etag.as_ref().map(|e| format!("\"{e}\""));
    if let (Some(et), Some(inm)) = (&etag, headers.get(header::IF_NONE_MATCH))
        && inm.as_bytes() == et.as_bytes()
    {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let range = parse_range(headers, size)?;
    let (start, end) = range.unwrap_or((0, size));
    let stream = vfs::read_stream(st, m, rel, node, start, end).await?;
    let mime = mime_guess::from_path(&node.name).first_or_octet_stream();
    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime.as_ref()).unwrap(),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    // Never let user content run as active HTML on our origin.
    h.insert("content-security-policy", HeaderValue::from_static("sandbox; default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    if let Some(et) = etag {
        h.insert(header::ETAG, HeaderValue::from_str(&et).unwrap());
    }
    let disp = if download { "attachment" } else { "inline" };
    let enc = utf8_percent_encode(&node.name, NON_ALPHANUMERIC).to_string();
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("{disp}; filename*=UTF-8''{enc}")).unwrap(),
    );
    if range.is_some() {
        *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
        resp.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{}/{size}", end - 1)).unwrap(),
        );
    }
    Ok(resp)
}

pub async fn content(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<PathQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let path = paths::normalize(&q.path)?;
    let (m, rel) = mounts::resolve(&st, u.id, &path).await?;
    let node = vfs::stat(&st, &m, &rel).await?;
    serve(&st, &m, &rel, &node, &headers, q.download.is_some()).await
}

pub async fn stat(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<PathQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = paths::normalize(&q.path)?;
    let (m, rel) = mounts::resolve(&st, u.id, &path).await?;
    let n = vfs::stat(&st, &m, &rel).await?;
    Ok(Json(
        json!({ "path": path, "name": n.name, "is_dir": n.is_dir, "size": n.size, "mtime": n.mtime, "etag": n.etag, "mode": m.mode }),
    ))
}

#[derive(Deserialize)]
pub struct PathBody {
    pub path: String,
}

pub async fn mkdir(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(b): Json<PathBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = paths::normalize(&b.path)?;
    if path.is_empty() {
        return Err(bad("folder name required"));
    }
    vfs::mkdir_p(&st, u.id, &path).await?;
    Ok(Json(json!({ "path": path })))
}

#[derive(Deserialize)]
pub struct FromTo {
    pub from: String,
    pub to: String,
}

pub async fn move_(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(b): Json<FromTo>,
) -> ApiResult<Json<serde_json::Value>> {
    let from = paths::normalize(&b.from)?;
    let to = paths::normalize(&b.to)?;
    vfs::move_path(&st, u.id, &from, &to).await?;
    Ok(Json(json!({ "path": to })))
}

pub async fn copy(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(b): Json<FromTo>,
) -> ApiResult<Json<serde_json::Value>> {
    let from = paths::normalize(&b.from)?;
    let to = paths::normalize(&b.to)?;
    vfs::copy_path(&st, u.id, &from, &to).await?;
    Ok(Json(json!({ "path": to })))
}

#[derive(Deserialize)]
pub struct DeleteBody {
    pub paths: Vec<String>,
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(b): Json<DeleteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    for p in &b.paths {
        let p = paths::normalize(p)?;
        vfs::delete(&st, u.id, &p).await?;
    }
    Ok(Json(json!({ "deleted": b.paths.len() })))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub limit: Option<i64>,
}

pub async fn search(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let term = q.q.trim();
    if term.is_empty() {
        return Ok(Json(json!({ "hits": [] })));
    }
    let hits = vfs::search(&st, u.id, term, q.limit.unwrap_or(100).clamp(1, 500)).await?;
    Ok(Json(json!({ "hits": hits })))
}

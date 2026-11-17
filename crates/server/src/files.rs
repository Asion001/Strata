//! File browser HTTP API.

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use strata_storage::paths;

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult, bad};
use crate::mounts::{self, Mount};
use crate::safe;
use crate::state::AppState;
use crate::vfs::{self, Node};
use futures::StreamExt;

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

/// Read the first bytes of a file, for content sniffing.
async fn head_bytes(st: &AppState, m: &Mount, rel: &str, node: &Node) -> ApiResult<Vec<u8>> {
    let n = (node.size as u64).min(safe::SNIFF_LEN);
    let mut s = vfs::read_stream(st, m, rel, node, 0, n).await?;
    let mut out = Vec::with_capacity(n as usize);
    while let Some(b) = s.next().await {
        out.extend_from_slice(&b.map_err(|e| ApiError::Internal(e.into()))?);
    }
    Ok(out)
}

/// The serving decision needs the first bytes of the file. Its result is
/// cached per (volume, path, version), so Range requests (video seeking) and
/// repeat views don't read the head again.
fn class_key(m: &Mount, rel: &str, node: &Node) -> Option<String> {
    node.etag
        .as_ref()
        .map(|e| format!("{}|{rel}|{e}", m.volume_id))
}

fn cached_class(st: &AppState, m: &Mount, rel: &str, node: &Node) -> Option<safe::Class> {
    st.decisions.get(&class_key(m, rel, node)?)
}

fn remember_class(st: &AppState, m: &Mount, rel: &str, node: &Node, class: safe::Class) {
    if let Some(k) = class_key(m, rel, node) {
        st.decisions.insert(k, class);
    }
}

async fn classify_cached(
    st: &AppState,
    m: &Mount,
    rel: &str,
    node: &Node,
) -> ApiResult<safe::Class> {
    let key = node
        .etag
        .as_ref()
        .map(|e| format!("{}|{rel}|{e}", m.volume_id));
    if let Some(k) = &key
        && let Some(c) = st.decisions.get(k)
    {
        return Ok(c);
    }
    let head = head_bytes(st, m, rel, node).await?;
    let class = safe::classify(&node.name, &head);
    if let Some(k) = key {
        st.decisions.insert(k, class);
    }
    Ok(class)
}

/// Serve file bytes safely (see `safe`): sniffed Content-Type, sandboxing
/// headers, Range, ETag and a header-safe Content-Disposition.
pub async fn serve(
    st: &AppState,
    m: &Mount,
    rel: &str,
    node: &Node,
    headers: &HeaderMap,
    purpose: safe::Purpose,
) -> ApiResult<Response> {
    if node.is_dir {
        return Err(bad("this is a folder"));
    }
    let size = node.size as u64;
    // The validator covers the decision too, so a cached response matches.
    // `s2`: version of the serving rules (bump when they change), so responses cached under older
    // rules are not revalidated into place.
    let etag = node
        .etag
        .as_deref()
        .map(safe::header_safe_tag)
        .map(|e| match purpose {
            safe::Purpose::ImagePreview => format!("\"s2i-{e}\""),
            safe::Purpose::Content { .. } => format!("\"s2-{e}\""),
        });
    if let (Some(et), Some(inm)) = (&etag, headers.get(header::IF_NONE_MATCH))
        && inm.as_bytes() == et.as_bytes()
    {
        let mut resp = StatusCode::NOT_MODIFIED.into_response();
        let h = resp.headers_mut();
        safe::protect(h);
        h.insert(header::ETAG, HeaderValue::from_str(et).unwrap());
        h.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-cache"),
        );
        return Ok(resp);
    }
    let range = parse_range(headers, size)?;
    let (start, end) = range.unwrap_or((0, size));
    // One read serves both the type decision and the body when the body
    // starts at 0: peek its first bytes, then hand them on. The type and the
    // bytes therefore always come from the same version of the file.
    let mut stream = vfs::read_stream(st, m, rel, node, start, end).await?;
    let class = match cached_class(st, m, rel, node) {
        Some(c) => c,
        None if start == 0 => {
            let mut head: Vec<u8> = Vec::new();
            let want = (end - start).min(safe::SNIFF_LEN) as usize;
            let mut peeked = Vec::new();
            while head.len() < want {
                match stream.next().await {
                    Some(Ok(b)) => {
                        head.extend_from_slice(&b[..b.len().min(want - head.len())]);
                        peeked.push(b);
                    }
                    Some(Err(e)) => return Err(ApiError::Internal(e.into())),
                    None => break,
                }
            }
            let class = safe::classify(&node.name, &head);
            remember_class(st, m, rel, node, class);
            let replay = futures::stream::iter(peeked.into_iter().map(Ok));
            stream = Box::pin(replay.chain(stream));
            class
        }
        // Mid-file range with no cached type: classify from the head.
        None => classify_cached(st, m, rel, node).await?,
    };
    let decision = safe::decide_class(class, purpose);
    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    safe::protect(h);
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(decision.content_type),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(end - start));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    if let Some(et) = etag {
        h.insert(header::ETAG, HeaderValue::from_str(&et).unwrap());
    }
    h.insert(
        header::CONTENT_DISPOSITION,
        safe::content_disposition(decision.inline, &node.name),
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
    let purpose = safe::Purpose::Content {
        download: q.download.is_some(),
    };
    serve(&st, &m, &rel, &node, &headers, purpose).await
}

/// `GET /api/files/image?path=`: the image-preview route. The UI loads it only
/// through `<img>`; it is the one place SVG is served as `image/svg+xml`.
pub async fn image(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<PathQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let path = paths::normalize(&q.path)?;
    let (m, rel) = mounts::resolve(&st, u.id, &path).await?;
    let node = vfs::stat(&st, &m, &rel).await?;
    serve(&st, &m, &rel, &node, &headers, safe::Purpose::ImagePreview).await
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

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

/// Cache key for a file version's classification.
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

/// Classify a file's head; only a complete head (min(size, SNIFF_LEN)) may
/// decide, and be cached, for this version.
fn classify_head(st: &AppState, m: &Mount, rel: &str, node: &Node, head: &[u8]) -> safe::Class {
    let class = safe::classify(&node.name, head);
    if head.len() as u64 == (node.size as u64).min(safe::SNIFF_LEN) {
        remember_class(st, m, rel, node, class);
    }
    class
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
    // Content responses carry the file version's ETag, the same value a
    // write's If-Match must present (`vfs::etag_for`).
    let etag = Some(match purpose {
        safe::Purpose::ImagePreview => format!("\"vi-{}\"", vfs::version(node)),
        safe::Purpose::Content { .. } => vfs::etag_for(node),
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
    // The type is decided from the file's first SNIFF_LEN bytes (never
    // fewer, so a tiny Range probe can't decide it for everyone). When the
    // body covers them, type and bytes come from the same read (peek, then
    // replay). Mid-file or short ranges read the head separately first; on
    // external backends a concurrent overwrite can then mislabel that one
    // response, but the type is always a safe, non-executable one.
    let full_head = size.min(safe::SNIFF_LEN);
    let (class, stream) = match cached_class(st, m, rel, node) {
        Some(c) => (c, vfs::read_stream(st, m, rel, node, start, end).await?),
        None if start == 0 => {
            // Read at least the full head in this one read, even for a tiny
            // probe like bytes=0-1, then serve only what was asked for.
            let mut stream = vfs::read_stream(st, m, rel, node, 0, end.max(full_head)).await?;
            let mut head: Vec<u8> = Vec::new();
            let mut peeked = Vec::new();
            while (head.len() as u64) < full_head {
                match stream.next().await {
                    Some(Ok(b)) => {
                        let take = b.len().min(full_head as usize - head.len());
                        head.extend_from_slice(&b[..take]);
                        peeked.push(b);
                    }
                    Some(Err(e)) => return Err(ApiError::Internal(e.into())),
                    None => break,
                }
            }
            let class = classify_head(st, m, rel, node, &head);
            let stream: vfs::ByteStream = if end < full_head {
                let part = bytes::Bytes::from(head).slice(..(end as usize).min(full_head as usize));
                Box::pin(futures::stream::once(async move { Ok(part) }))
            } else {
                let replay = futures::stream::iter(peeked.into_iter().map(Ok));
                Box::pin(replay.chain(stream))
            };
            (class, stream)
        }
        None => {
            let head = head_bytes(st, m, rel, node).await?;
            let class = classify_head(st, m, rel, node, &head);
            (class, vfs::read_stream(st, m, rel, node, start, end).await?)
        }
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

/// Largest file the in-browser editor may save.
pub const MAX_EDIT_BYTES: usize = 5 * 1024 * 1024;

/// One ETag from an If-Match / If-None-Match value (`W/` and lists are not
/// meaningful for our strong, single-version tags).
fn single_etag(v: &HeaderValue) -> ApiResult<String> {
    let s = v
        .to_str()
        .map_err(|_| bad("invalid precondition header"))?
        .trim();
    if s.contains(',') || s.starts_with("W/") {
        return Err(bad("If-Match takes exactly one strong ETag"));
    }
    Ok(s.to_string())
}

/// `PUT /api/files/content?path=`: save a whole file (the text editor).
///
/// * existing file: `If-Match: <ETag from GET>`, 412 if it changed, 428
///   if the header is missing (no blind overwrites);
/// * new file: `If-None-Match: *`, 412 if it exists meanwhile.
///
/// The precondition is checked under the volume lock in the same critical
/// section as the write, and the write takes the normal store paths, so
/// the journal and websocket push fire as for uploads.
pub async fn put_content(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<PathQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    let path = paths::normalize(&q.path)?;
    let (m, rel) = mounts::resolve(&st, u.id, &path).await?;
    if rel.is_empty() {
        return Err(bad("cannot write to a mount point"));
    }
    if body.len() > MAX_EDIT_BYTES {
        return Err(bad("file too large to save from the editor"));
    }
    let expect = match (
        headers.get(header::IF_MATCH),
        headers.get(header::IF_NONE_MATCH),
    ) {
        (Some(v), None) => vfs::Expect::Etag(single_etag(v)?),
        (None, Some(v)) if single_etag(v)? == "*" => vfs::Expect::Absent,
        (None, None) => return Err(ApiError::PreconditionRequired),
        _ => return Err(bad("use either If-Match: <etag> or If-None-Match: *")),
    };
    let input: vfs::ByteStream = Box::pin(futures::stream::once(async move { Ok(body) }));
    vfs::write_stream_if(&st, &m, &rel, input, &expect).await?;
    let node = vfs::stat(&st, &m, &rel).await?;
    let etag = vfs::etag_for(&node);
    let mut resp = Json(json!({
        "path": path, "size": node.size, "mtime": node.mtime, "etag": etag,
    }))
    .into_response();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    Ok(resp)
}

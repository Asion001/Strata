//! File browser HTTP API.

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_storage::paths;

use crate::auth::AuthUser;
use crate::conflicts::{self, OnConflict, Source};
use crate::error::{ApiError, ApiResult, bad};
use crate::mounts::{self, Mount};
use crate::range;
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
        // Whose listing this is (the app's offline cache keys by it).
        "user": u.id,
        "mount": { "id": m.id, "path": m.path, "mode": m.mode, "connection_name": m.connection_name,
                   "connection_kind": m.connection_kind, "offline": st.is_offline(m.connection_id),
                   "trash": crate::trash::enabled(&st) && crate::trash::supports_trash(&m) },
        "entries": entries,
    })))
}

/// Read the first bytes of a file, for content sniffing.
async fn head_bytes(st: &AppState, m: &Mount, rel: &str, node: &Node) -> ApiResult<Vec<u8>> {
    let n = (node.size as u64).min(safe::SNIFF_LEN);
    let mut s = vfs::read_stream(st, m, rel, node, 0, n).await?;
    let mut out = Vec::with_capacity(n as usize);
    while let Some(b) = s.next().await {
        out.extend_from_slice(&b.map_err(crate::error::read_error)?);
    }
    Ok(out)
}

/// Cache key for a file version's classification.
fn class_key(m: &Mount, rel: &str, node: &Node) -> Option<String> {
    // A coarse version may not change when the content does: don't cache by it.
    if vfs::is_coarse(node) {
        return None;
    }
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
    // Content responses carry the file version's ETag, the same value a
    // write's If-Match must present (`vfs::etag_for`, which also carries the
    // serving-rules version, so responses cached under older rules are not
    // revalidated into place).
    let etag = Some(match purpose {
        // A different validator for the image route's different headers.
        safe::Purpose::ImagePreview => format!("\"r3i-{}\"", vfs::version(node)),
        safe::Purpose::Content { .. } => vfs::etag_for(node),
    });
    // Coarse raw versions can stay the same across a change: never 304 them.
    if let (Some(et), Some(inm)) = (&etag, headers.get(header::IF_NONE_MATCH))
        && !vfs::is_coarse(node)
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
    // A range only for the version the client has (If-Range: an exact match
    // of the current ETag; a header that can't be read counts as no match).
    // A coarse raw version (no backend etag, seconds-only mtime) can stay
    // the same across a rewrite, so it never vouches for a range: a resumed
    // download there gets the whole file, never a splice of two versions.
    // (Media players send a plain Range, which still works.)
    let etag_now = etag.clone().unwrap_or_default();
    // (Nor does a weak backend validator: If-Range needs a strong one.)
    let weak = node.etag.as_deref().is_some_and(|t| t.starts_with("W/"));
    let vouches = !((vfs::is_coarse(node) || weak) && headers.contains_key(header::IF_RANGE));
    let range = if vouches
        && range::if_range_allows(
            headers
                .get(header::IF_RANGE)
                .map(|v| v.to_str().unwrap_or("")),
            &etag_now,
        ) {
        range::parse(
            headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
            size,
        )
    } else {
        range::Range::Full
    };
    let (start, end, range) = match range {
        range::Range::Full => (0, size, None),
        range::Range::Part { start, end } => (start, end, Some((start, end))),
        range::Range::Unsatisfiable => {
            let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            let h = resp.headers_mut();
            safe::protect(h);
            h.insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
            );
            h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            h.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-cache"),
            );
            if let Some(et) = etag.as_deref().and_then(|e| HeaderValue::from_str(e).ok()) {
                h.insert(header::ETAG, et);
            }
            return Ok(resp);
        }
    };
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
                    Some(Err(e)) => return Err(crate::error::read_error(e)),
                    None => break,
                }
            }
            let class = classify_head(st, m, rel, node, &head);
            if (head.len() as u64) < end.min(full_head) {
                // Shorter than its stat: it changed since.
                return Err(ApiError::Conflict(
                    "the file changed while being read; try again".into(),
                ));
            }
            let stream: vfs::ByteStream = if end < full_head {
                let part = bytes::Bytes::from(head).slice(..end as usize);
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
    // Byte-range responses go out as stored (see `routes::Uncompressed`).
    resp.extensions_mut().insert(crate::routes::Uncompressed);
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

#[derive(Deserialize, utoipa::ToSchema)]
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

#[derive(Deserialize, utoipa::ToSchema)]
pub struct FromTo {
    pub from: String,
    pub to: String,
    /// When `to` is taken: fail (409 `exists`, the default), replace it, or
    /// keep both ("name (2)"; the answer says the path).
    #[serde(default)]
    pub on_conflict: OnConflict,
}

/// Is the item at `path` a folder (a mount point is one)?
async fn is_folder(st: &AppState, user_id: i64, path: &str) -> ApiResult<bool> {
    let (m, rel) = mounts::resolve(st, user_id, path).await?;
    if rel.is_empty() {
        return Ok(true);
    }
    Ok(vfs::stat(st, &m, &rel).await?.is_dir)
}

/// The paths of a move or copy, checked before anything is placed.
async fn from_to(st: &AppState, user_id: i64, b: &FromTo) -> ApiResult<(String, String, bool)> {
    let from = paths::normalize(&b.from)?;
    let to = paths::normalize(&b.to)?;
    if from.is_empty() || to.is_empty() {
        return Err(bad("cannot move or copy the root folder"));
    }
    let is_dir = is_folder(st, user_id, &from).await?;
    Ok((from, to, is_dir))
}

pub async fn move_(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(b): Json<FromTo>,
) -> ApiResult<Json<serde_json::Value>> {
    let (from, to, is_dir) = from_to(&st, u.id, &b).await?;
    if from == to {
        return Err(bad("source and destination are the same"));
    }
    let path = conflicts::write_at(
        &st,
        u.id,
        &to,
        is_dir,
        b.on_conflict,
        Source::Move(&from),
        |at| {
            let (st, from) = (st.clone(), from.clone());
            async move { vfs::move_path(&st, u.id, &from, &at).await }
        },
    )
    .await?;
    Ok(Json(json!({ "path": path })))
}

pub async fn copy(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(b): Json<FromTo>,
) -> ApiResult<Json<serde_json::Value>> {
    let (from, to, is_dir) = from_to(&st, u.id, &b).await?;
    if from == to && b.on_conflict == OnConflict::Replace {
        return Err(bad("an item can't replace itself"));
    }
    let path = conflicts::write_at(
        &st,
        u.id,
        &to,
        is_dir,
        b.on_conflict,
        Source::Copy(&from),
        |at| {
            let (st, from) = (st.clone(), from.clone());
            async move { vfs::copy_path(&st, u.id, &from, &at).await }
        },
    )
    .await?;
    Ok(Json(json!({ "path": path })))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct DeleteBody {
    pub paths: Vec<String>,
    /// Skip the trash and delete for good.
    #[serde(default)]
    pub permanent: bool,
    /// Go on past a path that fails, and report it in `results` (otherwise
    /// the first failure is the answer, and later paths aren't tried).
    #[serde(default)]
    pub keep_going: bool,
}

/// What happened to one path of a delete.
#[derive(Serialize, utoipa::ToSchema)]
pub struct DeleteResult {
    pub path: String,
    /// It went to the trash (rather than being deleted for good).
    pub trashed: bool,
    /// Its trash id, for an undo. None when deleted for good, failed, or (rarely)
    /// while its move into the trash is still being completed.
    pub trash_id: Option<i64>,
    /// Why it failed (only with `keep_going`), and the HTTP status that error
    /// would have had.
    pub error: Option<String>,
    pub status: Option<u16>,
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(b): Json<DeleteBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut results = Vec::new();
    for raw in &b.paths {
        let done = match paths::normalize(raw) {
            Ok(p) => vfs::delete(&st, u.id, &p, !b.permanent).await,
            Err(e) => Err(e.into()),
        };
        let r = match done {
            Ok(d) => DeleteResult {
                path: raw.clone(),
                trashed: matches!(d, vfs::Deleted::Trashed(_)),
                trash_id: match d {
                    vfs::Deleted::Trashed(id) => id,
                    vfs::Deleted::Permanently => None,
                },
                error: None,
                status: None,
            },
            Err(e) if b.keep_going => {
                e.log_internal();
                DeleteResult {
                    path: raw.clone(),
                    trashed: false,
                    trash_id: None,
                    status: Some(e.status().as_u16()),
                    error: Some(e.public_message()),
                }
            }
            Err(e) => return Err(e),
        };
        results.push(r);
    }
    let ok = results.iter().filter(|r| r.error.is_none()).count();
    let trashed = results.iter().filter(|r| r.trashed).count();
    let trash_ids: Vec<i64> = results.iter().filter_map(|r| r.trash_id).collect();
    Ok(Json(json!({
        "deleted": ok,
        "trashed": trashed,
        "trash_ids": trash_ids,
        "results": results,
    })))
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
    if s.contains(',') {
        return Err(bad("If-Match takes exactly one ETag"));
    }
    // Our tags are strong; a proxy may have weakened one in transit (e.g.
    // when compressing). The opaque part still names the same version.
    Ok(s.trim_start_matches("W/").to_string())
}

/// `PUT /api/files/content?path=`: save a whole file (the text editor).
///
/// * existing file: `If-Match: <ETag from GET>`, 412 if it changed, 428
///   if the header is missing (no blind overwrites);
/// * new file: `If-None-Match: *`, 412 if it exists meanwhile.
///
/// Optional `X-Strata-Base-Hash`: BLAKE3 (hex) of the bytes the edit is based
/// on; on raw storage it is compared with the current content as well.
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
        (Some(v), None) => vfs::Expect::Etag {
            etag: single_etag(v)?,
            // BLAKE3 of the bytes the client's edit is based on: lets raw
            // storage with coarse metadata still detect any outside change.
            base: match headers.get("x-strata-base-hash") {
                Some(h) => Some(
                    h.to_str()
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| bad("invalid X-Strata-Base-Hash"))?,
                ),
                None => None,
            },
        },
        (None, Some(v)) if single_etag(v)? == "*" => vfs::Expect::Absent,
        (None, None) => {
            return Err(ApiError::PreconditionRequired(
                "this write needs If-Match (or If-None-Match: * to create a file)",
            ));
        }
        _ => return Err(bad("use either If-Match: <etag> or If-None-Match: *")),
    };
    let input: vfs::ByteStream = Box::pin(futures::stream::once(async move { Ok(body) }));
    // The version comes from inside the write's lock: never a later one.
    let (_, node) = vfs::write_stream_if(&st, &m, &rel, input, &expect).await?;
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

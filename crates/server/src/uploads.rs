//! Resumable, chunked uploads.
//!
//! 1. `POST /api/uploads {path, size, chunks:[{hash,size}]}`: the client
//!    splits the file with the shared FastCDC chunker and hashes each chunk
//!    with BLAKE3. The answer lists the chunk indexes the server is missing.
//! 2. `PUT /api/uploads/{id}/chunks/{hash}` for each missing chunk.
//! 3. `POST /api/uploads/{id}/commit`.
//!
//! Hashed volumes: chunks go straight into the volume's blob store. The
//! missing list only leaves out chunks *this user* stored before, so it never
//! reveals whether someone else has a file; if another user already stored a
//! chunk, the server still receives it and just skips writing it again.
//!
//! Raw volumes do not negotiate: every chunk of the file is sent and staged
//! per upload on the server (the list only tracks resume progress), then
//! streamed to the backend on commit.

use std::collections::HashSet;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use strata_storage::chunker::MAX_SIZE;
use strata_storage::{Hash, paths};

use crate::auth::{AuthUser, random_token};
use crate::conflicts::{self, OnConflict, Source};
use crate::error::{ApiError, ApiResult, Existing, bad, conflict};
use crate::mounts;
use crate::state::AppState;
use crate::vfs::{self, ByteStream};

#[derive(Deserialize, utoipa::ToSchema)]
pub struct ChunkSpec {
    pub hash: String,
    pub size: i64,
}

#[derive(Deserialize, utoipa::ToSchema)]
#[schema(as = UploadInput)]
pub struct CreateReq {
    pub path: String,
    pub size: i64,
    pub chunks: Vec<ChunkSpec>,
    pub mtime: Option<DateTime<Utc>>,
    /// When the name is taken: replace it (the default: an upload saves over
    /// a file of that name), fail (409 `exists`, so the client can ask), or
    /// keep both. Checked here and again at commit (which can answer anew).
    #[serde(default = "saves_over")]
    #[schema(default = "replace", required = false)]
    pub on_conflict: OnConflict,
}

fn saves_over() -> OnConflict {
    OnConflict::Replace
}

#[derive(Deserialize, Default, utoipa::ToSchema)]
pub struct CommitReq {
    /// The answer, if the name was taken meanwhile (left out: as asked at
    /// create).
    #[serde(default)]
    pub on_conflict: Option<OnConflict>,
}

#[derive(sqlx::FromRow)]
struct Upload {
    path: String,
    on_conflict: String,
    committed: Option<sqlx::types::Json<serde_json::Value>>,
    volume_id: i64,
    size: i64,
    chunks: Vec<Vec<u8>>,
    chunk_sizes: Vec<i64>,
    mtime: Option<DateTime<Utc>>,
}

async fn load(st: &AppState, user_id: i64, id: &str) -> ApiResult<Upload> {
    sqlx::query_as("SELECT path, on_conflict, committed, volume_id, size, chunks, chunk_sizes, mtime FROM uploads WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .fetch_optional(&st.db)
        .await?
        .ok_or(ApiError::NotFound)
}

async fn is_hashed_volume(st: &AppState, volume_id: i64) -> ApiResult<bool> {
    let mode: String = sqlx::query_scalar("SELECT mode FROM volumes WHERE id = $1")
        .bind(volume_id)
        .fetch_one(&st.db)
        .await?;
    Ok(mode == "hashed")
}

fn staging_dir(st: &AppState, id: &str) -> std::path::PathBuf {
    st.staging.join(id)
}

/// Indexes of chunks the client still has to send.
async fn missing(st: &AppState, user_id: i64, id: &str, up: &Upload) -> ApiResult<Vec<usize>> {
    let present: HashSet<Vec<u8>> = if is_hashed_volume(st, up.volume_id).await? {
        // Only chunks this user stored count (no cross-user disclosure), and
        // only if GC is not deleting them; touching them keeps them until commit.
        let owned: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT DISTINCT o.hash FROM blob_owners o
             WHERE o.volume_id = $1 AND o.user_id = $2 AND o.hash = ANY($3)",
        )
        .bind(up.volume_id)
        .bind(user_id)
        .bind(&up.chunks)
        .fetch_all(&st.db)
        .await?;
        let gone: HashSet<Vec<u8>> = vfs::touch_blobs(st, up.volume_id, &owned)
            .await?
            .into_iter()
            .collect();
        owned.into_iter().filter(|h| !gone.contains(h)).collect()
    } else {
        let dir = staging_dir(st, id);
        let mut s = HashSet::new();
        for c in up.chunks.iter().collect::<HashSet<_>>() {
            if tokio::fs::try_exists(dir.join(hex::encode(c)))
                .await
                .unwrap_or(false)
            {
                s.insert(c.clone());
            }
        }
        s
    };
    Ok(up
        .chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| !present.contains(*c))
        .map(|(i, _)| i)
        .collect())
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(req): Json<CreateReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = paths::normalize(&req.path)?;
    let (m, rel) = mounts::resolve(&st, u.id, &path).await?;
    if rel.is_empty() {
        return Err(bad("cannot upload onto a mount point"));
    }
    // Refuse before the client sends any data, not at commit.
    vfs::ensure_writable(&st, &m).await?;
    if req.chunks.is_empty() || req.chunks.len() > 1_000_000 {
        return Err(bad("bad chunk list"));
    }
    let mut hashes = Vec::with_capacity(req.chunks.len());
    let mut sizes = Vec::with_capacity(req.chunks.len());
    for c in &req.chunks {
        let h: Hash = c.hash.parse().map_err(|_| bad("bad chunk hash"))?;
        if c.size < 0 || c.size as usize > MAX_SIZE {
            return Err(bad("chunk too large"));
        }
        hashes.push(h.0.to_vec());
        sizes.push(c.size);
    }
    if sizes.iter().sum::<i64>() != req.size {
        return Err(bad("chunk sizes do not add up to the file size"));
    }
    match vfs::stat(&st, &m, &rel).await {
        Ok(n) if n.is_dir && req.on_conflict == OnConflict::Replace => {
            return Err(conflict(
                "a folder has that name; keep both instead, or rename the folder",
            ));
        }
        Ok(n) if req.on_conflict == OnConflict::Fail => {
            return Err(ApiError::Exists(Box::new(Existing {
                name: n.name,
                path: path.clone(),
                is_dir: n.is_dir,
                size: n.size,
                mtime: n.mtime,
            })));
        }
        _ => {}
    }
    vfs::check_raw_place(&st, &m, &rel).await?;
    let id = random_token(16);
    sqlx::query(
        "INSERT INTO uploads (id, user_id, path, volume_id, size, chunks, chunk_sizes, mtime, on_conflict)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&id)
    .bind(u.id)
    .bind(&path)
    .bind(m.volume_id)
    .bind(req.size)
    .bind(&hashes)
    .bind(&sizes)
    .bind(req.mtime)
    .bind(req.on_conflict.as_str())
    .execute(&st.db)
    .await?;
    let up = load(&st, u.id, &id).await?;
    let missing = missing(&st, u.id, &id, &up).await?;
    Ok(Json(json!({ "id": id, "missing": missing })))
}

pub async fn status(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let up = load(&st, u.id, &id).await?;
    // (Committed: where it landed.)
    let path = up
        .committed
        .as_ref()
        .and_then(|c| c["path"].as_str())
        .unwrap_or(&up.path);
    Ok(Json(
        json!({ "id": id, "path": path, "size": up.size, "missing": missing(&st, u.id, &id, &up).await? }),
    ))
}

pub async fn put_chunk(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path((id, hash)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let up = load(&st, u.id, &id).await?;
    let h: Hash = hash.parse().map_err(|_| bad("bad chunk hash"))?;
    let idx = up
        .chunks
        .iter()
        .position(|c| c[..] == h.0[..])
        .ok_or_else(|| bad("chunk is not part of this upload"))?;
    if up.chunk_sizes[idx] != body.len() as i64 {
        return Err(bad("chunk size mismatch"));
    }
    let data = body;
    let h2 = h;
    let ok = tokio::task::spawn_blocking(move || (Hash::of(&data) == h2, data))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    let (matches, data) = ok;
    if !matches {
        return Err(bad("chunk content does not match its hash"));
    }
    if is_hashed_volume(&st, up.volume_id).await? {
        let m = volume_mount_for(&st, u.id, up.volume_id, &up.path).await?;
        let store = vfs::hashed_store(&st, &m).await?;
        vfs::store_blob(&st, &store, up.volume_id, &h, data).await?;
        sqlx::query("INSERT INTO blob_owners (volume_id, hash, user_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(up.volume_id)
            .bind(h.0.to_vec())
            .bind(u.id)
            .execute(&st.db)
            .await?;
    } else {
        let dir = staging_dir(&st, &id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| ApiError::Internal(e.into()))?;
        let tmp = dir.join(format!(".{}.{}", h.hex(), rand::random::<u32>()));
        tokio::fs::write(&tmp, &data)
            .await
            .map_err(|e| ApiError::Internal(e.into()))?;
        tokio::fs::rename(&tmp, dir.join(h.hex()))
            .await
            .map_err(|e| ApiError::Internal(e.into()))?;
    }
    Ok(Json(json!({})))
}

/// The mount an upload targets (must still be the same volume).
async fn volume_mount_for(
    st: &AppState,
    user_id: i64,
    volume_id: i64,
    path: &str,
) -> ApiResult<mounts::Mount> {
    let (m, _) = mounts::resolve(st, user_id, path).await?;
    if m.volume_id != volume_id {
        return Err(conflict(
            "the destination folder changed; restart the upload",
        ));
    }
    Ok(m)
}

pub async fn commit(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    // (No body, or `{}`: as asked at create. A body is JSON, checked as the
    // `Json` extractor checks any other.)
    let req = if body.iter().all(u8::is_ascii_whitespace) {
        CommitReq::default()
    } else {
        Json::<CommitReq>::from_bytes(&body)
            .map_err(|e| ApiError::BadRequest(e.body_text()))?
            .0
    };
    load(&st, u.id, &id).await?;
    // One commit of an upload at a time: a retry sent while the first is
    // still writing waits for it (and then gets the same answer).
    // (Past the wait, not a refusal: the first one is still at it.)
    let one = conflicts::wait_claim(&st, format!("upload:{id}"))
        .await
        .ok_or_else(|| {
            ApiError::Unavailable("this upload is still being saved; try again".into())
        })?;
    // (To the end, even if this request goes away.)
    conflicts::detached(async move {
        let _one = one;
        commit_detached(st, u.id, id, req).await
    })
    .await
}

async fn commit_detached(
    st: AppState,
    uid: i64,
    id: String,
    req: CommitReq,
) -> ApiResult<Json<serde_json::Value>> {
    let up = load(&st, uid, &id).await?;
    if let Some(answer) = up.committed {
        // Committed already (its answer was lost): the same answer, about
        // what was uploaded, whatever happened to the file since. (Never
        // "not found", which would have the client upload it again.)
        return Ok(Json(answer.0));
    }
    let mode = req
        .on_conflict
        .unwrap_or_else(|| OnConflict::parse(&up.on_conflict));
    let miss = missing(&st, uid, &id, &up).await?;
    if !miss.is_empty() {
        return Err(conflict(format!("{} chunks still missing", miss.len())));
    }
    let (m, _) = mounts::resolve(&st, uid, &up.path).await?;
    if m.volume_id != up.volume_id {
        return Err(conflict(
            "the destination folder changed; restart the upload",
        ));
    }
    let hashes: Vec<Hash> = up
        .chunks
        .iter()
        .filter_map(|c| Hash::from_slice(c))
        .collect();
    let path = conflicts::write_at(
        &st,
        uid,
        &up.path,
        false,
        mode,
        Source::Upload,
        |at, in_place| {
            let (st, hashes, sizes, dir) = (
                st.clone(),
                hashes.clone(),
                up.chunk_sizes.clone(),
                staging_dir(&st, &id),
            );
            let (mtime, volume) = (up.mtime, up.volume_id);
            // Saving over the file (its new content, atomically), as asked
            // even if it appeared only now; otherwise only where nothing is
            // (checked under the volume lock): a file written meanwhile
            // outside these claims is never overwritten.
            let expect = if in_place || mode == OnConflict::Replace {
                vfs::Expect::Any
            } else {
                vfs::Expect::Absent
            };
            async move {
                let (m, rel) = mounts::resolve(&st, uid, &at).await?;
                if m.volume_id != volume {
                    return Err(conflict(
                        "the destination folder changed; restart the upload",
                    ));
                }
                if m.is_hashed() {
                    match vfs::put_hashed_file_if(&st, &m, &rel, &hashes, &sizes, mtime, &expect)
                        .await
                    {
                        Err(ApiError::PreconditionFailed(_)) => {
                            Err(conflict("something was saved there meanwhile; try again"))
                        }
                        r => r.map(|_| ()),
                    }
                } else {
                    let stream: ByteStream =
                        Box::pin(futures::stream::iter(hashes).then(move |h| {
                            let f = dir.join(h.hex());
                            async move { tokio::fs::read(f).await.map(Bytes::from) }
                        }));
                    match vfs::write_stream_if(&st, &m, &rel, stream, &expect).await {
                        Err(ApiError::PreconditionFailed(_)) => {
                            Err(conflict("something was saved there meanwhile; try again"))
                        }
                        r => r.map(|_| ()),
                    }
                }
            }
        },
    )
    .await?;
    // (Saved: whatever fails from here, the answer says so, and is kept.)
    let node = match mounts::resolve(&st, uid, &path).await {
        Ok((m, rel)) => vfs::stat(&st, &m, &rel).await.ok(),
        Err(_) => None,
    };
    let answer = match node {
        Some(n) => {
            json!({ "path": path, "name": n.name, "size": n.size, "mtime": n.mtime, "etag": n.etag })
        }
        None => json!({
            "path": path, "name": paths::split(&path).map_or("", |(_, n)| n), "size": up.size,
            "mtime": up.mtime.unwrap_or_else(Utc::now), "etag": null
        }),
    };
    // Kept (without its chunks) until the uploads expire, so a commit sent
    // again gets this answer instead of writing a second copy. (The file is
    // saved either way: a failure here doesn't make this one fail.)
    // (Twice if need be; if it still fails, the chunks stay, so a commit
    // sent again can still save it.)
    let record = || {
        sqlx::query(
            "UPDATE uploads SET committed = $2, chunks = '{}', chunk_sizes = '{}' WHERE id = $1",
        )
        .bind(&id)
        .bind(sqlx::types::Json(&answer))
        .execute(&st.db)
    };
    match record().await.or(record().await) {
        Ok(_) => {
            let _ = tokio::fs::remove_dir_all(staging_dir(&st, &id)).await;
        }
        Err(e) => tracing::warn!("upload {id} saved at {path}, but not recorded: {e}"),
    }
    Ok(Json(answer))
}

pub async fn cancel(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let r = sqlx::query("DELETE FROM uploads WHERE id = $1 AND user_id = $2")
        .bind(&id)
        .bind(u.id)
        .execute(&st.db)
        .await?;
    if r.rows_affected() == 1 {
        let _ = tokio::fs::remove_dir_all(staging_dir(&st, &id)).await;
    }
    Ok(Json(json!({})))
}

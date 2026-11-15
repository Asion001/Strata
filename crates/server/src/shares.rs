//! Public share links with optional password and expiry.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_storage::paths;

use crate::auth::{AuthUser, hash_password, random_token, verify_password_async};
use crate::error::{ApiError, ApiResult, bad};
use crate::mounts::{self, Mount};
use crate::state::AppState;
use crate::{files, thumbs, vfs};

#[derive(Serialize, sqlx::FromRow)]
pub struct Share {
    pub id: i64,
    pub token: String,
    pub path: String,
    pub is_dir: bool,
    pub has_password: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
) -> ApiResult<Json<Vec<Share>>> {
    Ok(Json(
        sqlx::query_as(
            "SELECT id, token, path, is_dir, password_hash IS NOT NULL AS has_password, expires_at, created_at
             FROM shares WHERE owner_id = $1 ORDER BY created_at DESC",
        )
        .bind(u.id)
        .fetch_all(&st.db)
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub path: String,
    pub password: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(req): Json<CreateReq>,
) -> ApiResult<Json<Share>> {
    let path = paths::normalize(&req.path)?;
    if path.is_empty() {
        return Err(bad("sharing your whole root folder is not allowed"));
    }
    let (m, rel) = mounts::resolve(&st, u.id, &path).await?;
    let node = vfs::stat(&st, &m, &rel).await?;
    vfs::ensure_cached(&st, &m, &rel).await?;
    let node_id = vfs::node_id(&st, m.volume_id, &rel).await?;
    let pw = match req.password.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => Some(hash_password(p)?),
        None => None,
    };
    let token = random_token(12);
    let share: Share = sqlx::query_as(
        "INSERT INTO shares (token, owner_id, path, volume_id, rel_path, node_id, is_dir, password_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING id, token, path, is_dir, password_hash IS NOT NULL AS has_password, expires_at, created_at",
    )
    .bind(&token)
    .bind(u.id)
    .bind(&path)
    .bind(m.volume_id)
    .bind(&rel)
    .bind(node_id)
    .bind(node.is_dir)
    .bind(pw)
    .bind(req.expires_at)
    .fetch_one(&st.db)
    .await?;
    Ok(Json(share))
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("DELETE FROM shares WHERE id = $1 AND owner_id = $2")
        .bind(id)
        .bind(u.id)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({})))
}

// ---- public side ----

#[derive(sqlx::FromRow)]
struct ShareRow {
    id: i64,
    owner_id: i64,
    path: String,
    volume_id: i64,
    rel_path: String,
    node_id: Option<i64>,
    is_dir: bool,
    password_hash: Option<String>,
}

#[derive(Deserialize)]
pub struct PublicQuery {
    #[serde(default)]
    pub path: String,
    /// Access key returned by `unlock` for password-protected shares.
    pub k: Option<String>,
    pub size: Option<u32>,
    pub download: Option<String>,
}

fn access_key(st: &AppState, s: &ShareRow) -> String {
    let key = blake3::derive_key("strata share access v1", st.cfg.secret_key.as_bytes());
    let msg = format!("{}:{}", s.id, s.password_hash.as_deref().unwrap_or(""));
    blake3::keyed_hash(&key, msg.as_bytes()).to_hex()[..32].to_string()
}

async fn load_share(st: &AppState, token: &str) -> ApiResult<ShareRow> {
    sqlx::query_as(
        "SELECT id, owner_id, path, volume_id, rel_path, node_id, is_dir, password_hash FROM shares
         WHERE token = $1 AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(token)
    .fetch_optional(&st.db)
    .await?
    .ok_or(ApiError::NotFound)
}

/// Resolve a path inside a share, enforcing the password.
async fn open(
    st: &AppState,
    token: &str,
    q: &PublicQuery,
) -> ApiResult<(ShareRow, Mount, String, String)> {
    let s = load_share(st, token).await?;
    if s.password_hash.is_some() && q.k.as_deref() != Some(access_key(st, &s).as_str()) {
        return Err(ApiError::Unauthorized);
    }
    let sub = paths::normalize(&q.path)?;
    if !s.is_dir && !sub.is_empty() {
        return Err(ApiError::NotFound);
    }
    // The owner's path must still lead to what was shared; if it was renamed
    // or something else is mounted there now, the link is dead.
    let (root_m, root_rel) = mounts::resolve(st, s.owner_id, &s.path).await?;
    if root_m.volume_id != s.volume_id
        || root_rel != s.rel_path
        || (s.node_id.is_none() && !s.rel_path.is_empty())
        || vfs::node_id(st, root_m.volume_id, &root_rel).await? != s.node_id
    {
        return Err(ApiError::NotFound);
    }
    let full = paths::join(&s.path, &sub);
    let (m, rel) = mounts::resolve(st, s.owner_id, &full).await?;
    Ok((s, m, rel, full))
}

pub async fn info(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Query(q): Query<PublicQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let s = load_share(&st, &token).await?;
    let name = paths::split(&s.path)
        .map(|(_, n)| n.to_string())
        .unwrap_or_default();
    if s.password_hash.is_some() && q.k.as_deref() != Some(access_key(&st, &s).as_str()) {
        return Ok(Json(
            json!({ "name": name, "is_dir": s.is_dir, "needs_password": true }),
        ));
    }
    let (s, m, rel, full) = open(&st, &token, &q).await?;
    if s.is_dir {
        let (_, entries) = vfs::list(&st, s.owner_id, &full).await?;
        Ok(Json(
            json!({ "name": name, "is_dir": true, "needs_password": false, "path": paths::normalize(&q.path)?, "entries": entries }),
        ))
    } else {
        let node = vfs::stat(&st, &m, &rel).await?;
        Ok(Json(
            json!({ "name": name, "is_dir": false, "needs_password": false, "size": node.size, "mtime": node.mtime }),
        ))
    }
}

#[derive(Deserialize)]
pub struct UnlockReq {
    pub password: String,
}

pub async fn unlock(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Json(req): Json<UnlockReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let s = load_share(&st, &token).await?;
    let Some(h) = s.password_hash.clone() else {
        return Ok(Json(json!({ "key": null })));
    };
    if !verify_password_async(req.password, h).await {
        return Err(bad("wrong password"));
    }
    Ok(Json(json!({ "key": access_key(&st, &s) })))
}

pub async fn content(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Query(q): Query<PublicQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let (_, m, rel, _) = open(&st, &token, &q).await?;
    let node = vfs::stat(&st, &m, &rel).await?;
    files::serve(&st, &m, &rel, &node, &headers, q.download.is_some()).await
}

pub async fn thumb(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Query(q): Query<PublicQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let (_, m, rel, _) = open(&st, &token, &q).await?;
    let node = vfs::stat(&st, &m, &rel).await?;
    thumbs::thumbnail(&st, &m, &rel, &node, q.size.unwrap_or(256), &headers).await
}

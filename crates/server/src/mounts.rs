//! Mounts: show a volume (connection + folder + mode) at a folder of a user's tree.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_storage::paths;

use crate::auth::AuthUser;
use crate::connections::check_access;
use crate::error::{ApiError, ApiResult, bad, conflict};
use crate::journal::{self, Change};
use crate::state::AppState;
use crate::vfs;

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct Mount {
    pub id: i64,
    pub owner_id: i64,
    /// Folder in the owner's tree ('' = root).
    pub path: String,
    pub volume_id: i64,
    pub connection_id: i64,
    pub connection_name: String,
    pub connection_kind: String,
    /// Folder inside the connection.
    pub base_path: String,
    /// 'raw' or 'hashed'
    pub mode: String,
    /// Journal entries of the volume up to this seq predate the mount.
    pub journal_from: i64,
    pub created_at: DateTime<Utc>,
}

impl Mount {
    pub fn is_hashed(&self) -> bool {
        self.mode == "hashed"
    }
}

const SELECT: &str = "SELECT m.id, m.owner_id, m.path, m.volume_id, v.connection_id, c.name AS connection_name,
    c.kind AS connection_kind, v.base_path, v.mode, m.journal_from, m.created_at
    FROM mounts m JOIN volumes v ON v.id = m.volume_id JOIN connections c ON c.id = v.connection_id";

/// A user's mounts, longest path first (cached).
pub async fn user_mounts(st: &AppState, user_id: i64) -> ApiResult<Arc<Vec<Mount>>> {
    if let Some(m) = st.mounts.get(&user_id) {
        return Ok(m.clone());
    }
    let mut v: Vec<Mount> = sqlx::query_as(&format!("{SELECT} WHERE m.owner_id = $1"))
        .bind(user_id)
        .fetch_all(&st.db)
        .await?;
    v.sort_by_key(|m| std::cmp::Reverse(m.path.len()));
    let v = Arc::new(v);
    st.mounts.insert(user_id, v.clone());
    Ok(v)
}

pub fn invalidate(st: &AppState, user_id: i64) {
    st.mounts.remove(&user_id);
}

/// Find the mount that holds `path` (normalised) and the path inside it.
pub async fn resolve(st: &AppState, user_id: i64, path: &str) -> ApiResult<(Mount, String)> {
    let mounts = user_mounts(st, user_id).await?;
    for m in mounts.iter() {
        if let Some(rest) = paths::strip_prefix(path, &m.path) {
            if crate::vfs::is_reserved(m, rest) {
                return Err(ApiError::NotFound);
            }
            return Ok((m.clone(), rest.to_string()));
        }
    }
    Err(ApiError::Internal(anyhow::anyhow!(
        "user {user_id} has no root mount"
    )))
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
) -> ApiResult<Json<Vec<Mount>>> {
    let mut v: Vec<Mount> = (*user_mounts(&st, u.id).await?).clone();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub path: String,
    pub connection_id: i64,
    #[serde(default)]
    pub base_path: String,
    pub mode: String,
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(req): Json<CreateReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = paths::normalize(&req.path)?;
    if path.is_empty() {
        return Err(bad(
            "pick a folder to mount at; the root is your private folder",
        ));
    }
    if req.mode != "raw" && req.mode != "hashed" {
        return Err(bad("mode must be 'raw' or 'hashed'"));
    }
    let base = paths::normalize(&req.base_path)?;
    check_access(&st, &u, req.connection_id).await?;
    if base.split('/').next() == Some(strata_storage::backend::ATOMIC_TMP_DIR) {
        return Err(bad("that folder is reserved"));
    }
    check_storage_overlap(&st, req.connection_id, &base, &req.mode).await?;
    let (parent, name) = paths::split(&path).unwrap();
    // The mount point's parent must be a folder (create it if missing).
    vfs::mkdir_p(&st, u.id, parent).await?;
    let volume_id: i64 = sqlx::query_scalar(
        "INSERT INTO volumes (connection_id, base_path, mode) VALUES ($1, $2, $3)
         ON CONFLICT (connection_id, base_path, mode) DO UPDATE SET mode = EXCLUDED.mode RETURNING id",
    )
    .bind(req.connection_id)
    .bind(&base)
    .bind(&req.mode)
    .fetch_one(&st.db)
    .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO mounts (owner_id, path, parent_path, name, volume_id, journal_from)
         VALUES ($1, $2, $3, $4, $5, (SELECT journal_seq FROM volumes WHERE id = $5)) RETURNING id",
    )
    .bind(u.id)
    .bind(&path)
    .bind(parent)
    .bind(name)
    .bind(volume_id)
    .fetch_one(&st.db)
    .await
    .map_err(|e| match ApiError::from(e) {
        ApiError::Conflict(_) => conflict("something is already mounted there"),
        e => e,
    })?;
    invalidate(&st, u.id);
    journal::record_user(
        &st,
        u.id,
        vec![Change::new("mount_added", &path, None, true)],
    )
    .await?;
    let m: Mount = sqlx::query_as(&format!("{SELECT} WHERE m.id = $1"))
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    // Warn when another mount shows an overlapping folder of the same connection.
    let overlaps = journal::overlapping_mounts(&st, u.id, id).await?;
    let mut v = serde_json::to_value(&m).map_err(|e| ApiError::Internal(e.into()))?;
    v["overlaps"] = json!(overlaps);
    Ok(Json(v))
}

/// Where a volume's files physically live: (connection or local directory, folder).
async fn storage_root(
    st: &AppState,
    connection_id: i64,
    base: &str,
) -> ApiResult<(String, String)> {
    let cfg = st.connection_config(connection_id).await?;
    Ok(match cfg.local_dir(&st.cfg.local_base)? {
        // Two local connections may point at overlapping server folders.
        Some(dir) => (
            String::from("local"),
            paths::join(dir.to_string_lossy().trim_matches('/'), base),
        ),
        None => (format!("conn:{connection_id}"), base.to_string()),
    })
}

/// A raw view over a hashed store (or a hashed store inside a raw folder) on
/// the same storage would let raw deletes and moves destroy blobs. Refuse it.
pub async fn check_storage_overlap(
    st: &AppState,
    connection_id: i64,
    base: &str,
    mode: &str,
) -> ApiResult<()> {
    let (root, folder) = storage_root(st, connection_id, base).await?;
    let other_mode = if mode == "raw" { "hashed" } else { "raw" };
    let others: Vec<(i64, String)> =
        sqlx::query_as("SELECT connection_id, base_path FROM volumes WHERE mode = $1")
            .bind(other_mode)
            .fetch_all(&st.db)
            .await?;
    for (cid, obase) in others {
        let Ok((oroot, ofolder)) = storage_root(st, cid, &obase).await else {
            continue;
        };
        if oroot == root
            && (paths::strip_prefix(&folder, &ofolder).is_some()
                || paths::strip_prefix(&ofolder, &folder).is_some())
        {
            return Err(conflict(
                "that folder overlaps a hashed store (or a raw folder holds this store); pick a separate folder",
            ));
        }
    }
    Ok(())
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let m = user_mounts(&st, u.id)
        .await?
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .ok_or(ApiError::NotFound)?;
    if m.path.is_empty() {
        return Err(bad("the root folder cannot be unmounted"));
    }
    let nested = user_mounts(&st, u.id)
        .await?
        .iter()
        .any(|o| o.id != m.id && paths::strip_prefix(&o.path, &m.path).is_some());
    if nested {
        return Err(conflict(
            "other mounts live inside this one; remove them first",
        ));
    }
    sqlx::query("DELETE FROM mounts WHERE id = $1")
        .bind(id)
        .execute(&st.db)
        .await?;
    invalidate(&st, u.id);
    journal::record_user(
        &st,
        u.id,
        vec![Change::new("mount_removed", &m.path, None, true)],
    )
    .await?;
    Ok(Json(json!({})))
}

/// Rescan a raw mount from the backend in the background.
pub async fn scan(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let m = user_mounts(&st, u.id)
        .await?
        .iter()
        .find(|m| m.id == id)
        .cloned()
        .ok_or(ApiError::NotFound)?;
    if m.is_hashed() {
        return Ok(Json(
            json!({ "started": false, "reason": "hashed mounts need no scanning" }),
        ));
    }
    tokio::spawn(async move {
        match vfs::scan_tree(&st, &m, "").await {
            Ok(n) => tracing::info!("scanned mount {} ({n} folders)", m.id),
            Err(e) => tracing::warn!("scan of mount {} failed: {e}", m.id),
        }
    });
    Ok(Json(json!({ "started": true })))
}

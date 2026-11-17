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
    if strata_storage::backend::has_reserved_component(&base)
        && st.connection_config(req.connection_id).await?.kind() == "local"
    {
        return Err(bad("that folder is reserved"));
    }
    // Refuse an overlap before making any folder; the parent folder is made
    // outside the layout lock (it may be slow remote storage).
    check_storage_overlap(&st, req.connection_id, &base, &req.mode).await?;
    let (parent, name) = paths::split(&path).unwrap();
    // The mount point's parent must be a folder (create it if missing).
    vfs::mkdir_p(&st, u.id, parent).await?;
    // Checked again and created under the layout lock, so a concurrent
    // volume or connection edit can't slip an overlap in between.
    let (mut tx, _layout) = lock_layout(&st).await?;
    check_storage_overlap(&st, req.connection_id, &base, &req.mode).await?;
    let volume_id: i64 = sqlx::query_scalar(
        "INSERT INTO volumes (connection_id, base_path, mode) VALUES ($1, $2, $3)
         ON CONFLICT (connection_id, base_path, mode) DO UPDATE SET mode = EXCLUDED.mode RETURNING id",
    )
    .bind(req.connection_id)
    .bind(&base)
    .bind(&req.mode)
    .fetch_one(&mut *tx)
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
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match ApiError::from(e) {
        ApiError::Conflict(_) => conflict("something is already mounted there"),
        e => e,
    })?;
    tx.commit().await?;
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
/// Do two storage locations overlap (same storage, one folder inside the
/// other or equal)? The single rule used by mount guards and save locks.
pub fn overlaps(a: &(String, String), b: &(String, String)) -> bool {
    a.0 == b.0
        && (paths::strip_prefix(&a.1, &b.1).is_some() || paths::strip_prefix(&b.1, &a.1).is_some())
}

/// Volume locations for loops over many volumes: each connection's root is
/// looked up once. A root comes from the connection's stored columns (as
/// fetched with a JOIN), else from its config. It is unknown (`None`) when
/// the config names no valid location, or can't be decrypted and no
/// location was stored before: such a connection can't reach any storage
/// now. The one rule shared by mount guards and save locks.
#[derive(Default)]
pub struct Locations(std::collections::HashMap<i64, Option<(String, String)>>);

impl Locations {
    pub async fn of(
        &mut self,
        st: &AppState,
        connection_id: i64,
        base: &str,
        stored: (Option<String>, Option<String>),
    ) -> ApiResult<Option<(String, String)>> {
        let root = match self.0.get(&connection_id) {
            Some(r) => r.clone(),
            None => {
                let r = match stored {
                    (Some(id), Some(folder)) => Some((id, folder)),
                    _ => {
                        let enc: Vec<u8> =
                            sqlx::query_scalar("SELECT config_enc FROM connections WHERE id = $1")
                                .bind(connection_id)
                                .fetch_one(&st.db)
                                .await?;
                        st.decrypt_config(&enc)
                            .ok()
                            .and_then(|c| c.location(&st.cfg.local_base).ok())
                    }
                };
                self.0.insert(connection_id, r.clone());
                r
            }
        };
        Ok(root.map(|(id, folder)| (id, paths::join(&folder, base))))
    }
}

pub async fn storage_root(
    st: &AppState,
    connection_id: i64,
    base: &str,
) -> ApiResult<(String, String)> {
    let stored: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT storage_id, storage_folder FROM connections WHERE id = $1")
            .bind(connection_id)
            .fetch_one(&st.db)
            .await?;
    let (id, folder) = match stored {
        (Some(id), Some(folder)) => (id, folder),
        _ => crate::connections::store_location(st, connection_id).await?,
    };
    Ok((id, paths::join(&folder, base)))
}

/// Serialises changes to where volumes live (new volumes, re-pointed
/// connections) together with their overlap checks. Write the change in the
/// returned transaction and keep the guard until it commits; checks may read
/// outside it, since every earlier change has committed by the time the lock
/// is granted. Waiters queue in-process first, so at most one pooled
/// connection waits inside Postgres (like `vfs::lock_volumes`).
pub async fn lock_layout(
    st: &AppState,
) -> ApiResult<(
    sqlx::Transaction<'static, sqlx::Postgres>,
    tokio::sync::OwnedMutexGuard<()>,
)> {
    let guard = st.layout_lock.clone().lock_owned().await;
    let mut tx = st.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1398035010, 0)")
        .execute(&mut *tx)
        .await?;
    Ok((tx, guard))
}

/// A raw view over a hashed store (or a hashed store inside a raw folder) on
/// the same storage would let raw deletes and moves destroy blobs. Refuse it.
/// Call under `lock_layout`.
pub async fn check_storage_overlap(
    st: &AppState,
    connection_id: i64,
    base: &str,
    mode: &str,
) -> ApiResult<()> {
    let (root, folder) = storage_root(st, connection_id, base).await?;
    check_overlap_at(st, &[(root, folder, mode.to_string())], None).await
}

/// Are these (`root`, `folder`, `mode`) places clear of every volume of the
/// opposite mode? Volumes on `except` (the connection being edited) are left
/// out, as are connections whose location is unknown (see `Locations`).
pub async fn check_overlap_at(
    st: &AppState,
    places: &[(String, String, String)],
    except: Option<i64>,
) -> ApiResult<()> {
    if places.is_empty() {
        return Ok(());
    }
    // Volumes of a mode opposite to some place's, fetched once.
    // (connection id, base path, mode, stored storage id, stored folder)
    type Row = (i64, String, String, Option<String>, Option<String>);
    let modes: Vec<&str> = places.iter().map(|(_, _, m)| m.as_str()).collect();
    let others: Vec<Row> = sqlx::query_as(
        "SELECT v.connection_id, v.base_path, v.mode, c.storage_id, c.storage_folder
         FROM volumes v JOIN connections c ON c.id = v.connection_id
         WHERE v.mode <> ANY($1) AND c.id IS DISTINCT FROM $2",
    )
    .bind(&modes)
    .bind(except)
    .fetch_all(&st.db)
    .await?;
    let mut locs = Locations::default();
    for (cid, obase, omode, sid, sfolder) in others {
        let Some(other) = locs.of(st, cid, &obase, (sid, sfolder)).await? else {
            continue; // location unknown: can't reach storage
        };
        for (root, folder, mode) in places {
            if *mode != omode && overlaps(&(root.clone(), folder.clone()), &other) {
                return Err(conflict(
                    "that folder overlaps a hashed store (or a raw folder holds this store); pick a separate folder",
                ));
            }
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

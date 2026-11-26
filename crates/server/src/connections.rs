//! Connections CRUD and health probes.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_storage::{ConnectionConfig, paths};

use crate::auth::{AuthUser, User};
use crate::error::{ApiError, ApiResult, bad, conflict};
use crate::state::AppState;

#[derive(Serialize, utoipa::ToSchema)]
pub struct ConnectionView {
    pub id: i64,
    pub owner_id: i64,
    pub name: String,
    pub kind: String,
    pub config: ConnectionConfig,
    pub capabilities: serde_json::Value,
    pub is_default: bool,
    /// ok | degraded | offline | unknown (not probed yet)
    pub health: String,
    /// Why the last probe failed, in plain words anyone may see.
    pub health_reason: Option<String>,
    /// The backend's own (sanitized) message: admins only, since it can
    /// name hosts and paths.
    pub health_error: Option<String>,
    pub health_latency_ms: Option<i32>,
    pub health_checked_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    owner_id: i64,
    name: String,
    kind: String,
    config_enc: Vec<u8>,
    capabilities: serde_json::Value,
    is_default: bool,
    health: String,
    health_reason: Option<String>,
    health_error: Option<String>,
    health_latency_ms: Option<i32>,
    health_checked_at: Option<DateTime<Utc>>,
}

const COLS: &str = "id, owner_id, name, kind, config_enc, capabilities, is_default, health, health_reason, health_error, health_latency_ms, health_checked_at";

impl Row {
    fn view(self, st: &AppState, admin: bool) -> ApiResult<ConnectionView> {
        let cfg = st.decrypt_config(&self.config_enc)?;
        Ok(ConnectionView {
            id: self.id,
            owner_id: self.owner_id,
            name: self.name,
            kind: self.kind,
            config: cfg.redacted(),
            capabilities: self.capabilities,
            is_default: self.is_default,
            health: self.health,
            health_reason: self.health_reason,
            health_error: if admin { self.health_error } else { None },
            health_latency_ms: self.health_latency_ms,
            health_checked_at: self.health_checked_at,
        })
    }
}

/// Compute and store a connection's storage location (see migration 0006).
/// A config that names no valid location (e.g. a bad path) has its stored
/// one cleared: it can't reach storage. One that can't be decrypted or
/// parsed keeps the last known location (the fault may be the server's key,
/// and overlap checks keep working from it) and returns the error.
pub async fn store_location(st: &AppState, id: i64) -> ApiResult<(String, String)> {
    // The row lock orders this against a concurrent `update`, which writes
    // its config and location together.
    let mut tx = st.db.begin().await?;
    let enc: Vec<u8> =
        sqlx::query_scalar("SELECT config_enc FROM connections WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ApiError::NotFound)?;
    let cfg = st.decrypt_config(&enc)?;
    let loc = cfg.location(&st.cfg.local_base).map_err(ApiError::Internal);
    let (sid, folder) = match &loc {
        Ok((sid, folder)) => (Some(sid.as_str()), Some(folder.as_str())),
        Err(_) => (None, None),
    };
    sqlx::query(
        "UPDATE connections SET storage_id = $2, storage_folder = $3
         WHERE id = $1 AND (storage_id IS DISTINCT FROM $2 OR storage_folder IS DISTINCT FROM $3)",
    )
    .bind(id)
    .bind(sid)
    .bind(folder)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    loc
}

/// Recompute every connection's location at start-up, so rows written by
/// older versions (missing, or in an older format) match today's rule.
/// Configs that can't be read keep theirs (see `store_location`).
pub async fn backfill_locations(st: &AppState) {
    let mut ids: Option<Vec<i64>> = None;
    for attempt in 0..5 {
        match sqlx::query_scalar("SELECT id FROM connections")
            .fetch_all(&st.db)
            .await
        {
            Ok(v) => {
                ids = Some(v);
                break;
            }
            Err(e) => {
                tracing::warn!(
                    "recomputing connection locations (attempt {}): {e}",
                    attempt + 1
                );
                if attempt < 4 {
                    tokio::time::sleep(std::time::Duration::from_millis(250 << attempt)).await;
                }
            }
        }
    }
    let Some(ids) = ids else {
        tracing::error!(
            "could not recompute connection locations; overlap checks may use stale rows"
        );
        return;
    };
    for id in ids {
        if let Err(e) = store_location(st, id).await {
            tracing::warn!("connection {id}: cannot determine its storage location: {e}");
        }
    }
}

/// Can `user` mount or edit this connection?
pub async fn check_access(st: &AppState, user: &User, connection_id: i64) -> ApiResult<()> {
    let owner: Option<i64> = sqlx::query_scalar("SELECT owner_id FROM connections WHERE id = $1")
        .bind(connection_id)
        .fetch_optional(&st.db)
        .await?;
    match owner {
        None => Err(ApiError::NotFound),
        Some(o) if o == user.id || user.is_admin => Ok(()),
        Some(_) => Err(ApiError::NotFound),
    }
}

/// Only a connection's own keys are ever used (never the server's), so an
/// S3 connection needs them (checked once blank secrets are filled in from
/// the stored connection, when editing).
fn own_keys(cfg: &ConnectionConfig) -> ApiResult<()> {
    if let ConnectionConfig::S3 {
        access_key_id,
        secret_access_key,
        ..
    } = cfg
        && (access_key_id.trim().is_empty() || secret_access_key.is_empty())
    {
        return Err(bad("an S3 connection needs an access key and its secret"));
    }
    Ok(())
}

fn validate(user: &User, name: &str, cfg: &ConnectionConfig) -> ApiResult<()> {
    if name.trim().is_empty() || name.len() > 100 {
        return Err(bad("name is required"));
    }
    // Local folders are on the server itself: admins only.
    if matches!(cfg, ConnectionConfig::Local { .. }) && !user.is_admin {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

pub async fn list(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
) -> ApiResult<Json<Vec<ConnectionView>>> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM connections WHERE owner_id = $1 OR $2 ORDER BY is_default DESC, name"
    ))
    .bind(u.id)
    .bind(u.is_admin)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| r.view(&st, u.is_admin))
            .collect::<ApiResult<_>>()?,
    ))
}

#[derive(Deserialize, utoipa::ToSchema)]
#[schema(as = ConnectionInput)]
pub struct UpsertReq {
    pub name: String,
    pub config: ConnectionConfig,
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(req): Json<UpsertReq>,
) -> ApiResult<Json<ConnectionView>> {
    validate(&u, &req.name, &req.config)?;
    own_keys(&req.config)?;
    req.config
        .create_local_root(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    req.config
        .operator(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    // Capabilities come from the first probe, right after the insert
    // ('{}': not detected yet).
    let caps = serde_json::json!({});
    let enc = st.encrypt_config(&req.config)?;
    let (sid, sfolder) = req
        .config
        .location(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO connections (owner_id, name, kind, config_enc, capabilities, storage_id, storage_folder)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(u.id)
    .bind(req.name.trim())
    .bind(req.config.kind())
    .bind(enc)
    .bind(caps)
    .bind(sid)
    .bind(sfolder)
    .fetch_one(&st.db)
    .await?;
    // A local folder gets its marker now (see `state::LocalRoot`).
    // (A read-only folder can't take one; nothing is written there anyway.)
    if req.config.kind() == "local"
        && let Err(e) = st.mark_root(id, None).await
    {
        tracing::warn!("connection {id}: no folder marker: {e}");
    }
    crate::health::probe_soon(&st, id).await;
    get_view(&st, &u, id).await.map(Json)
}

async fn get_view(st: &AppState, u: &User, id: i64) -> ApiResult<ConnectionView> {
    let r: Row = sqlx::query_as(&format!("SELECT {COLS} FROM connections WHERE id = $1"))
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    r.view(st, u.is_admin)
}

pub async fn get(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ConnectionView>> {
    check_access(&st, &u, id).await?;
    get_view(&st, &u, id).await.map(Json)
}

pub async fn update(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
    Json(mut req): Json<UpsertReq>,
) -> ApiResult<Json<ConnectionView>> {
    check_access(&st, &u, id).await?;
    validate(&u, &req.name, &req.config)?;
    let (kind, old_enc): (String, Vec<u8>) =
        sqlx::query_as("SELECT kind, config_enc FROM connections WHERE id = $1")
            .bind(id)
            .fetch_one(&st.db)
            .await?;
    if kind != req.config.kind() {
        return Err(bad("the backend type of a connection cannot change"));
    }
    // Blank secrets mean "unchanged", so the old config must be readable.
    let old = st.decrypt_config(&old_enc).map_err(|e| {
        tracing::warn!("connection {id}: stored config can't be read: {e:#}");
        bad("the stored settings of this connection can't be read (wrong server key, or an unknown format)")
    })?;
    req.config.merge_secrets(&old);
    own_keys(&req.config)?;
    // Only a re-pointed connection gets its folder made, a fresh failure
    // count and capabilities to detect: a rename never touches the storage
    // (an unmounted disk's folder must not be created again).
    let repointed = req.config != old;
    // Held until the new config is in place: a delete going to the trash
    // meanwhile would land in the old folder.
    let mut raw_lock = None;
    if repointed {
        // A raw trash lives in the connection's folder: re-pointed, its
        // items could no longer be found (and reconcile would drop them).
        let raw: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM volumes WHERE connection_id = $1 AND mode = 'raw'")
                .bind(id)
                .fetch_all(&st.db)
                .await?;
        if !raw.is_empty() {
            raw_lock = Some(crate::vfs::lock_volumes(&st, &raw).await?);
        }
        let raw_trash: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM trash t JOIN volumes v ON v.id = t.volume_id
             WHERE v.connection_id = $1 AND v.mode = 'raw'",
        )
        .bind(id)
        .fetch_one(&st.db)
        .await?;
        if raw_trash > 0 {
            return Err(conflict(format!(
                "{raw_trash} deleted item(s) from this connection are still in the trash; restore or delete them before pointing it somewhere else"
            )));
        }
        req.config
            .create_local_root(&st.cfg.local_base)
            .map_err(|e| bad(format!("invalid config: {e}")))?;
        let op = req
            .config
            .operator(&st.cfg.local_base)
            .map_err(|e| bad(format!("invalid config: {e}")))?;
        if !op.info().capability().write {
            // Hashed volumes (and the default store, which holds every
            // user's root) must keep storing blobs.
            let needs_writes: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM volumes WHERE connection_id = $1 AND mode = 'hashed')
                 OR EXISTS (SELECT 1 FROM connections WHERE id = $1 AND is_default)",
            )
            .bind(id)
            .fetch_one(&st.db)
            .await?;
            if needs_writes {
                return Err(conflict(
                    "that folder is read-only, but this connection holds hashed storage that needs writing",
                ));
            }
        }
    }
    let (sid, sfolder) = req
        .config
        .location(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    // Re-pointing a connection must not make its volumes overlap volumes of
    // the opposite mode (a raw view over a hashed store): checked and
    // written under the layout lock. The row lock and the compare make a
    // concurrent edit (whose secrets were not merged here) a conflict.
    let (mut tx, layout) = crate::mounts::lock_layout(&st).await?;
    let now_enc: Vec<u8> =
        sqlx::query_scalar("SELECT config_enc FROM connections WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if now_enc != old_enc {
        return Err(conflict(
            "the connection was changed meanwhile; reload it and try again",
        ));
    }
    let volumes: Vec<(String, String)> =
        sqlx::query_as("SELECT base_path, mode FROM volumes WHERE connection_id = $1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
    let places: Vec<_> = volumes
        .into_iter()
        .map(|(base, mode)| (sid.clone(), paths::join(&sfolder, &base), mode))
        .collect();
    crate::mounts::check_overlap_at(&st, &places, Some(id)).await?;
    let enc = st.encrypt_config(&req.config)?;
    // Config and location change together, in one statement.
    sqlx::query(
        // A re-pointed connection starts a fresh failure count, and its
        // capabilities come from the probe below.
        "UPDATE connections SET name = $2, config_enc = $3, storage_id = $4, storage_folder = $5,
                health_failures = CASE WHEN $6 THEN 0 ELSE health_failures END,
                health = CASE WHEN $6 THEN 'unknown' ELSE health END,
                health_reason = CASE WHEN $6 THEN NULL ELSE health_reason END,
                health_error = CASE WHEN $6 THEN NULL ELSE health_error END,
                health_latency_ms = CASE WHEN $6 THEN NULL ELSE health_latency_ms END,
                health_checked_at = CASE WHEN $6 THEN NULL ELSE health_checked_at END,
                root_marker = CASE WHEN $6 THEN FALSE ELSE root_marker END,
                capabilities = CASE WHEN $6 THEN '{}'::jsonb ELSE capabilities END
         WHERE id = $1",
    )
    .bind(id)
    .bind(req.name.trim())
    .bind(enc)
    .bind(&sid)
    .bind(&sfolder)
    .bind(repointed)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    drop(layout);
    st.forget_operator(id);
    if let Some(l) = raw_lock {
        l.release().await?;
    }
    if repointed {
        // A new target starts over: not offline, a fresh failure count, and
        // (local) its own folder marker.
        st.offline.remove(&id);
        // Cached mounts carry the old target's capabilities (range_read).
        crate::mounts::invalidate_all(&st);
        if req.config.kind() == "local"
            && let Err(e) = st.mark_root(id, None).await
        {
            tracing::warn!("connection {id}: no folder marker: {e}");
        }
        // Probed again at once: it may come back online, and goes offline
        // only after three failed probes as usual. (A rename touches no
        // storage, so it isn't probed.)
        crate::health::probe_soon(&st, id).await;
    }
    get_view(&st, &u, id).await.map(Json)
}

#[derive(Deserialize, Default)]
pub struct DeleteQuery {
    /// Also purge every trashed item that came from this connection.
    #[serde(default)]
    pub purge_trash: bool,
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<DeleteQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    check_access(&st, &u, id).await?;
    let in_use: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM mounts m JOIN volumes v ON v.id = m.volume_id WHERE v.connection_id = $1)
         OR EXISTS (SELECT 1 FROM connections WHERE id = $1 AND is_default)",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    if in_use {
        return Err(conflict(
            "connection is still mounted somewhere; remove those mounts first",
        ));
    }
    // A hashed volume's folder tree lives only in the database: refuse rather
    // than lose it. Raw volumes only hold a listing cache.
    let holds_tree: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM volumes v JOIN nodes n ON n.volume_id = v.id
                        WHERE v.connection_id = $1 AND v.mode = 'hashed')",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    if holds_tree {
        return Err(conflict(
            "this connection holds hashed volumes with files; mount them again and delete the files first",
        ));
    }
    // Trashed items keep their volume until they are restored or purged
    // (hashed ones hold blobs, raw ones files under `.strata-trash`).
    let trashed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM trash t JOIN volumes v ON v.id = t.volume_id WHERE v.connection_id = $1",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    if trashed > 0 && !q.purge_trash {
        return Err(conflict(format!(
            "{trashed} deleted item(s) from this connection are still in the trash; \
             restore them, or delete the connection with purge_trash to remove them for good"
        )));
    }
    if trashed > 0 {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT t.id FROM trash t JOIN volumes v ON v.id = t.volume_id WHERE v.connection_id = $1",
        )
        .bind(id)
        .fetch_all(&st.db)
        .await?;
        for t in ids {
            match crate::trash::purge(&st, t).await {
                Ok(()) | Err(ApiError::NotFound) => {}
                // Storage gone, offline or failing: the connection goes
                // anyway (asked for), so drop the row and remove what can be
                // removed; nothing is left half-deleted.
                Err(e) => {
                    tracing::warn!("purging trash item {t} for a connection delete failed: {e}");
                    crate::trash::forget(&st, t).await?;
                }
            }
        }
    }
    // A local connection's marker goes with it (best effort, below).
    // (Its operator, rooted at the folder, built before the row goes.)
    let local_op = match st.connection_config(id).await {
        Ok(c) if c.kind() == "local" => st.operator_unchecked(id).await.ok(),
        _ => None,
    };
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM volumes WHERE connection_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM connections WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    st.forget_connection(id);
    if let Some(op) = local_op {
        crate::state::LocalRoot::remove_marker(&op, id).await;
    }
    Ok(Json(json!({})))
}

pub async fn probe(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ConnectionView>> {
    check_access(&st, &u, id).await?;
    crate::health::probe_soon(&st, id).await;
    get_view(&st, &u, id).await.map(Json)
}

#[derive(Deserialize, utoipa::ToSchema)]
#[schema(as = ConnectionTest)]
pub struct TestReq {
    pub config: ConnectionConfig,
    /// When editing, blank secrets are taken from this connection.
    pub id: Option<i64>,
}

pub async fn test(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(mut req): Json<TestReq>,
) -> ApiResult<Json<serde_json::Value>> {
    validate(&u, "test", &req.config)?;
    if let Some(id) = req.id {
        check_access(&st, &u, id).await?;
        req.config.merge_secrets(&st.connection_config(id).await?);
    }
    own_keys(&req.config)?;
    // A new local folder is made when the connection is saved; testing
    // must not make it (or fail because it isn't there yet). An existing
    // connection's unchanged folder isn't made by saving, so it is tested.
    let unchanged = match req.id {
        Some(id) => st
            .connection_config(id)
            .await
            .is_ok_and(|c| c == req.config),
        None => false,
    };
    // A saved local folder is checked like the probes do (marker included):
    // an empty mount point left by an unmounted disk isn't "reachable".
    if unchanged
        && let Some(id) = req.id
        && let Some(root) = st.local_root(id, &req.config).await?
        && let Err(detail) = st
            .check_root(id, root, std::time::Duration::from_secs(20))
            .await
    {
        return Ok(Json(json!({
            "ok": false,
            "reason": "not found",
            "error": if u.is_admin { Some(detail) } else { None },
        })));
    }
    if let Some(dir) = req
        .config
        .local_dir(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?
        && !dir.exists()
    {
        return Ok(Json(if unchanged {
            // Saving won't make it: report it like a probe would.
            json!({
                "ok": false,
                "reason": "not found",
                "error": if u.is_admin { Some("the connection's folder is missing") } else { None },
            })
        } else {
            json!({ "ok": true, "error": null, "reason": null, "note": "the folder will be created" })
        }));
    }
    let op = req
        .config
        .operator(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    let secrets = crate::health::secrets(&req.config);
    let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let root = req.config.local_dir(&st.cfg.local_base).ok().flatten();
    // Read only: this config isn't saved yet, so nothing is written to it.
    let p = strata_storage::probe::probe(&op, root.as_deref(), false, &secrets).await;
    Ok(Json(match p.outcome {
        strata_storage::probe::Outcome::Ok => json!({ "ok": true, "error": null, "reason": null }),
        strata_storage::probe::Outcome::Down(f) | strata_storage::probe::Outcome::ReadOnly(f) => {
            json!({
                "ok": false,
                "reason": f.reason.as_str(),
                // The backend's own message names hosts: admins only.
                "error": if u.is_admin { Some(f.detail) } else { None },
            })
        }
    }))
}

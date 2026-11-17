//! Connections CRUD and health probes.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use opendal::Operator;
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_storage::{Capabilities, ConnectionConfig, paths};

use crate::auth::{AuthUser, User};
use crate::error::{ApiError, ApiResult, bad, conflict};
use crate::state::AppState;

#[derive(Serialize)]
pub struct ConnectionView {
    pub id: i64,
    pub owner_id: i64,
    pub name: String,
    pub kind: String,
    pub config: ConnectionConfig,
    pub capabilities: serde_json::Value,
    pub is_default: bool,
    pub health: String,
    pub health_error: Option<String>,
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
    health_error: Option<String>,
    health_checked_at: Option<DateTime<Utc>>,
}

const COLS: &str = "id, owner_id, name, kind, config_enc, capabilities, is_default, health, health_error, health_checked_at";

impl Row {
    fn view(self, st: &AppState) -> ApiResult<ConnectionView> {
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
            health_error: self.health_error,
            health_checked_at: self.health_checked_at,
        })
    }
}

/// Compute and store a connection's storage location (see migration 0006).
/// When none can be computed (undecryptable config, invalid path) the stored
/// one is cleared rather than trusted.
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
    let loc = st
        .decrypt_config(&enc)
        .and_then(|cfg| cfg.location(&st.cfg.local_base).map_err(ApiError::Internal));
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
/// older versions (missing, or in an older format) always match today's rule.
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
            .map(|r| r.view(&st))
            .collect::<ApiResult<_>>()?,
    ))
}

#[derive(Deserialize)]
pub struct UpsertReq {
    pub name: String,
    pub config: ConnectionConfig,
}

/// Probe a backend: list its root, with a timeout.
pub async fn probe_operator(op: &Operator) -> Result<(), String> {
    match tokio::time::timeout(Duration::from_secs(20), op.list("/")).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out after 20s".into()),
    }
}

async fn store_probe(st: &AppState, id: i64, res: &Result<(), String>) -> ApiResult<()> {
    sqlx::query(
        "UPDATE connections SET health = $2, health_error = $3, health_checked_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(if res.is_ok() { "ok" } else { "error" })
    .bind(res.as_ref().err())
    .execute(&st.db)
    .await?;
    Ok(())
}

pub async fn create(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(req): Json<UpsertReq>,
) -> ApiResult<Json<ConnectionView>> {
    validate(&u, &req.name, &req.config)?;
    let op = req
        .config
        .operator(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    let caps = Capabilities::detect(&req.config, &op, &st.cfg.local_base);
    let enc = st
        .secrets
        .encrypt(&serde_json::to_vec(&req.config).unwrap())?;
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
    .bind(sqlx::types::Json(caps))
    .bind(sid)
    .bind(sfolder)
    .fetch_one(&st.db)
    .await?;
    let res = probe_operator(&op).await;
    store_probe(&st, id, &res).await?;
    get_view(&st, id).await.map(Json)
}

async fn get_view(st: &AppState, id: i64) -> ApiResult<ConnectionView> {
    let r: Row = sqlx::query_as(&format!("SELECT {COLS} FROM connections WHERE id = $1"))
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    r.view(st)
}

pub async fn get(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ConnectionView>> {
    check_access(&st, &u, id).await?;
    get_view(&st, id).await.map(Json)
}

pub async fn update(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
    Json(mut req): Json<UpsertReq>,
) -> ApiResult<Json<ConnectionView>> {
    check_access(&st, &u, id).await?;
    validate(&u, &req.name, &req.config)?;
    let old = st.connection_config(id).await?;
    if old.kind() != req.config.kind() {
        return Err(bad("the backend type of a connection cannot change"));
    }
    req.config.merge_secrets(&old);
    let op = req
        .config
        .operator(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    let caps = Capabilities::detect(&req.config, &op, &st.cfg.local_base);
    // Re-pointing a connection must not make its volumes overlap volumes of
    // the opposite mode (a raw view over a hashed store).
    let (sid, sfolder) = req
        .config
        .location(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    let volumes: Vec<(String, String)> =
        sqlx::query_as("SELECT base_path, mode FROM volumes WHERE connection_id = $1")
            .bind(id)
            .fetch_all(&st.db)
            .await?;
    for (base, mode) in &volumes {
        crate::mounts::check_overlap_at(&st, &sid, &paths::join(&sfolder, base), mode, Some(id))
            .await?;
    }
    let enc = st
        .secrets
        .encrypt(&serde_json::to_vec(&req.config).unwrap())?;
    // Config and location change together, in one statement.
    sqlx::query(
        "UPDATE connections SET name = $2, config_enc = $3, capabilities = $4,
                storage_id = $5, storage_folder = $6 WHERE id = $1",
    )
    .bind(id)
    .bind(req.name.trim())
    .bind(enc)
    .bind(sqlx::types::Json(caps))
    .bind(&sid)
    .bind(&sfolder)
    .execute(&st.db)
    .await?;
    st.forget_operator(id);
    let res = probe_operator(&op).await;
    store_probe(&st, id, &res).await?;
    get_view(&st, id).await.map(Json)
}

pub async fn delete(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
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
    st.forget_operator(id);
    Ok(Json(json!({})))
}

pub async fn probe(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ConnectionView>> {
    check_access(&st, &u, id).await?;
    let res = match st.operator(id).await {
        Ok(op) => probe_operator(&op).await,
        Err(e) => Err(e.to_string()),
    };
    store_probe(&st, id, &res).await?;
    get_view(&st, id).await.map(Json)
}

#[derive(Deserialize)]
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
    let op = req
        .config
        .operator(&st.cfg.local_base)
        .map_err(|e| bad(format!("invalid config: {e}")))?;
    let res = probe_operator(&op).await;
    Ok(Json(json!({ "ok": res.is_ok(), "error": res.err() })))
}

/// Background: probe every connection.
pub async fn probe_all(st: &AppState) {
    let ids: Vec<i64> = match sqlx::query_scalar("SELECT id FROM connections")
        .fetch_all(&st.db)
        .await
    {
        Ok(v) => v,
        Err(e) => return tracing::warn!("probe: {e}"),
    };
    for id in ids {
        let res = match st.operator(id).await {
            Ok(op) => probe_operator(&op).await,
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = &res {
            tracing::warn!("connection {id} unhealthy: {e}");
        }
        let _ = store_probe(st, id, &res).await;
    }
}

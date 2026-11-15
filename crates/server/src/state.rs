use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use opendal::Operator;
use sqlx::PgPool;
use strata_storage::ConnectionConfig;
use strata_storage::crypto::SecretBox;
use tokio::sync::broadcast;

use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::mounts::Mount;

/// A new journal entry, pushed to websocket listeners.
#[derive(Clone, Debug)]
pub struct ChangeEvent {
    /// Set for changes inside a volume.
    pub volume_id: Option<i64>,
    /// Set for changes to one user's tree (mounts).
    pub user_id: Option<i64>,
}

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub db: PgPool,
    pub cfg: Config,
    pub secrets: SecretBox,
    /// connection id -> operator
    pub ops: DashMap<i64, Operator>,
    /// user id -> that user's mounts, longest path first
    pub mounts: DashMap<i64, Arc<Vec<Mount>>>,
    pub events: broadcast::Sender<ChangeEvent>,
    /// volume id -> in-process queue in front of the volume's advisory lock
    pub volume_locks: DashMap<i64, Arc<tokio::sync::Mutex<()>>>,
    /// Max concurrent volume-lock holders (each pins one pool connection).
    pub lock_permits: Arc<tokio::sync::Semaphore>,
    /// Server-local folder for chunks of uploads into raw volumes
    /// (`<staging>/<upload id>/<chunk hash>`).
    pub staging: std::path::PathBuf,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    pub async fn new(cfg: Config) -> Result<Self> {
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(32)
            .connect(&cfg.database_url)
            .await?;
        Self::with_pool(cfg, db).await
    }

    pub async fn with_pool(cfg: Config, db: PgPool) -> Result<Self> {
        sqlx::migrate!("./migrations").run(&db).await?;
        std::fs::create_dir_all(&cfg.data_dir)?;
        std::fs::create_dir_all(&cfg.local_base)?;
        let staging = cfg
            .data_dir
            .join(strata_storage::backend::INTERNAL_DIR)
            .join("staging");
        std::fs::create_dir_all(&staging)?;
        let (events, _) = broadcast::channel(1024);
        Ok(AppState(Arc::new(Inner {
            secrets: SecretBox::from_secret(&cfg.secret_key),
            db,
            cfg,
            ops: DashMap::new(),
            mounts: DashMap::new(),
            events,
            volume_locks: DashMap::new(),
            lock_permits: Arc::new(tokio::sync::Semaphore::new(8)),
            staging,
        })))
    }

    /// Decrypt a connection's config.
    pub async fn connection_config(&self, connection_id: i64) -> ApiResult<ConnectionConfig> {
        let enc: Vec<u8> = sqlx::query_scalar("SELECT config_enc FROM connections WHERE id = $1")
            .bind(connection_id)
            .fetch_one(&self.db)
            .await?;
        let plain = self.secrets.decrypt(&enc)?;
        serde_json::from_slice(&plain).map_err(|e| ApiError::Internal(e.into()))
    }

    /// Cached OpenDAL operator for a connection.
    pub async fn operator(&self, connection_id: i64) -> ApiResult<Operator> {
        if let Some(op) = self.ops.get(&connection_id) {
            return Ok(op.clone());
        }
        let cfg = self.connection_config(connection_id).await?;
        let op = cfg.operator(&self.cfg.local_base)?;
        self.ops.insert(connection_id, op.clone());
        Ok(op)
    }

    pub fn forget_operator(&self, connection_id: i64) {
        self.ops.remove(&connection_id);
    }
}

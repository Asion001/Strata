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
    /// Set when a connection's health state changed.
    pub connection_id: Option<i64>,
}

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub db: PgPool,
    pub cfg: Config,
    pub secrets: SecretBox,
    /// connection id -> operator
    pub ops: DashMap<i64, Operator>,
    /// connection id -> times its operator was forgotten (a config change),
    /// so an operator built from the old config isn't cached afterwards
    pub op_gens: DashMap<i64, u64>,
    /// Connections the health probes found offline: their storage isn't
    /// touched (reads and writes get 503) until a probe succeeds again.
    pub offline: DashMap<i64, ()>,
    /// One health probe per connection at a time.
    pub probe_locks: DashMap<i64, Arc<tokio::sync::Mutex<()>>>,
    /// user id -> that user's mounts, longest path first
    pub mounts: DashMap<i64, Arc<Vec<Mount>>>,
    pub events: broadcast::Sender<ChangeEvent>,
    /// Cache of file classifications (see `files::serve`).
    pub decisions: quick_cache::sync::Cache<String, crate::safe::Class>,
    /// volume id -> in-process queue in front of the volume's advisory lock
    pub volume_locks: DashMap<i64, Arc<tokio::sync::Mutex<()>>>,
    /// In-process queue in front of the layout lock (`mounts::lock_layout`).
    pub layout_lock: Arc<tokio::sync::Mutex<()>>,
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
        // Now that both exist, compare real paths (symlinks, `..`) too.
        let (d, l) = (cfg.data_dir.canonicalize()?, cfg.local_base.canonicalize()?);
        if d.starts_with(&l) {
            anyhow::bail!(
                "STRATA_DATA_DIR resolves inside STRATA_LOCAL_BASE ({})",
                l.display()
            );
        }
        let staging = cfg.data_dir.join("staging");
        std::fs::create_dir_all(&staging)?;
        let (events, _) = broadcast::channel(1024);
        let st = AppState(Arc::new(Inner {
            secrets: SecretBox::from_secret(&cfg.secret_key),
            db,
            cfg,
            ops: DashMap::new(),
            op_gens: DashMap::new(),
            offline: DashMap::new(),
            probe_locks: DashMap::new(),
            mounts: DashMap::new(),
            events,
            decisions: quick_cache::sync::Cache::new(100_000),
            volume_locks: DashMap::new(),
            layout_lock: Arc::new(tokio::sync::Mutex::new(())),
            lock_permits: Arc::new(tokio::sync::Semaphore::new(8)),
            staging,
        }));
        crate::health::load(&st).await?;
        Ok(st)
    }

    /// Decrypt a connection's config.
    pub async fn connection_config(&self, connection_id: i64) -> ApiResult<ConnectionConfig> {
        let enc: Vec<u8> = sqlx::query_scalar("SELECT config_enc FROM connections WHERE id = $1")
            .bind(connection_id)
            .fetch_one(&self.db)
            .await?;
        self.decrypt_config(&enc)
    }

    /// Decrypt and parse a stored `config_enc`.
    pub fn decrypt_config(&self, enc: &[u8]) -> ApiResult<ConnectionConfig> {
        let plain = self.secrets.decrypt(enc)?;
        serde_json::from_slice(&plain).map_err(|e| ApiError::Internal(e.into()))
    }

    /// Serialise and encrypt a config for `config_enc`.
    pub fn encrypt_config(&self, cfg: &ConnectionConfig) -> ApiResult<Vec<u8>> {
        let plain = serde_json::to_vec(cfg).map_err(|e| ApiError::Internal(e.into()))?;
        Ok(self.secrets.encrypt(&plain)?)
    }

    /// Cached OpenDAL operator for a connection, refused (503) while the
    /// connection is offline. Health probes use `operator_unchecked`.
    pub async fn operator(&self, connection_id: i64) -> ApiResult<Operator> {
        self.ensure_online(connection_id)?;
        self.operator_unchecked(connection_id).await
    }

    /// 503 while a connection is offline.
    pub fn ensure_online(&self, connection_id: i64) -> ApiResult<()> {
        if self.offline.contains_key(&connection_id) {
            return Err(ApiError::Unavailable(
                "this storage is offline right now; try again later".into(),
            ));
        }
        Ok(())
    }

    pub fn is_offline(&self, connection_id: i64) -> bool {
        self.offline.contains_key(&connection_id)
    }

    /// The operator whatever the connection's health (for probes).
    pub async fn operator_unchecked(&self, connection_id: i64) -> ApiResult<Operator> {
        if let Some(op) = self.ops.get(&connection_id) {
            return Ok(op.clone());
        }
        let seen = self.op_gens.get(&connection_id).map(|g| *g).unwrap_or(0);
        let cfg = self.connection_config(connection_id).await?;
        let op = cfg.operator(&self.cfg.local_base)?;
        // Cached only if no config change landed meanwhile (the entry is
        // held across the insert, so a concurrent forget waits for it).
        let g = self.op_gens.entry(connection_id).or_insert(0);
        if *g == seen {
            self.ops.insert(connection_id, op.clone());
        }
        drop(g);
        Ok(op)
    }

    pub fn forget_operator(&self, connection_id: i64) {
        let mut g = self.op_gens.entry(connection_id).or_insert(0);
        *g += 1;
        self.ops.remove(&connection_id);
    }
}

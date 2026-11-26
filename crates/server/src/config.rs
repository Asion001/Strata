use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind: String,
    /// Server state: upload staging and thumbnail cache. Never inside
    /// `local_base`, so no connection can reach it.
    pub data_dir: PathBuf,
    /// Local connections live below this folder.
    pub local_base: PathBuf,
    /// Folder (below `local_base`) of the default connection that holds every
    /// user's private hashed root (`<store>/users/<id>`).
    pub default_store: String,
    /// Folder (below `local_base`) offered as a plain-files connection.
    pub files_dir: String,
    /// Built SvelteKit app (adapter-static output).
    pub web_dir: PathBuf,
    /// Key material for credential encryption and share-link keys.
    pub secret_key: String,
    pub admin_user: Option<String>,
    pub admin_password: Option<String>,
    pub cookie_secure: bool,
    pub session_days: i64,
    pub gc_grace_hours: i64,
    /// How long a raw folder listing is trusted before rescanning the backend.
    pub raw_ttl_secs: i64,
    /// Seconds between health probes of each connection (jittered).
    pub probe_secs: u64,
    /// Days deleted items stay in the trash; 0 turns the trash off (deletes
    /// are permanent).
    pub trash_days: i64,
    /// How long an operation waits for another one writing the same name
    /// (or committing the same upload) before giving up.
    pub busy_wait_ms: u64,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let data_dir = PathBuf::from(env("STRATA_DATA_DIR").unwrap_or_else(|| "./data".into()));
        Self {
            database_url: env("DATABASE_URL").context("DATABASE_URL is required")?,
            bind: env("STRATA_BIND").unwrap_or_else(|| "0.0.0.0:8080".into()),
            local_base: env("STRATA_LOCAL_BASE")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("local")),
            default_store: env("STRATA_DEFAULT_STORE").unwrap_or_else(|| "blobs".into()),
            files_dir: env("STRATA_FILES_DIR").unwrap_or_else(|| "files".into()),
            web_dir: PathBuf::from(env("STRATA_WEB_DIR").unwrap_or_else(|| "./web/build".into())),
            secret_key: env("STRATA_SECRET_KEY")
                .context("STRATA_SECRET_KEY is required (long random string)")?,
            admin_user: env("STRATA_ADMIN_USER"),
            admin_password: env("STRATA_ADMIN_PASSWORD"),
            cookie_secure: env("STRATA_COOKIE_SECURE")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
            session_days: env("STRATA_SESSION_DAYS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            gc_grace_hours: env("STRATA_GC_GRACE_HOURS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(24),
            raw_ttl_secs: env("STRATA_RAW_TTL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30),
            probe_secs: env("STRATA_PROBE_SECS")
                .and_then(|v| v.parse().ok())
                .map(|s: u64| s.max(5))
                .unwrap_or(60),
            trash_days: env("STRATA_TRASH_DAYS")
                .and_then(|v| v.parse().ok())
                .map(|d: i64| d.clamp(0, 3650))
                .unwrap_or(30),
            busy_wait_ms: env("STRATA_BUSY_WAIT_MS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60_000),
            data_dir,
        }
        .checked()
    }

    /// Refuse a state dir inside the storage base: users could reach it
    /// through a local connection.
    pub fn checked(self) -> Result<Self> {
        let abs = |p: &std::path::Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let (d, l) = (abs(&self.data_dir), abs(&self.local_base));
        let store = strata_storage::paths::normalize(&self.default_store)
            .map_err(|e| anyhow::anyhow!("STRATA_DEFAULT_STORE: {e}"))?;
        let files = strata_storage::paths::normalize(&self.files_dir)
            .map_err(|e| anyhow::anyhow!("STRATA_FILES_DIR: {e}"))?;
        let overlap = |a: &str, b: &str| strata_storage::paths::strip_prefix(a, b).is_some();
        if store.is_empty()
            || files.is_empty()
            || overlap(&store, &files)
            || overlap(&files, &store)
        {
            anyhow::bail!(
                "STRATA_DEFAULT_STORE and STRATA_FILES_DIR must be separate, non-empty folders"
            );
        }
        if d.starts_with(&l) {
            anyhow::bail!(
                "STRATA_DATA_DIR ({}) must not be inside STRATA_LOCAL_BASE ({})",
                d.display(),
                l.display()
            );
        }
        Ok(self)
    }
}

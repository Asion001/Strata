use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub bind: String,
    /// Server data dir: staging chunks, thumbnail cache, default local storage.
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
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let data_dir = PathBuf::from(env("STRATA_DATA_DIR").unwrap_or_else(|| "./data".into()));
        Ok(Self {
            database_url: env("DATABASE_URL").context("DATABASE_URL is required")?,
            bind: env("STRATA_BIND").unwrap_or_else(|| "0.0.0.0:8080".into()),
            local_base: env("STRATA_LOCAL_BASE")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.clone()),
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
            data_dir,
        })
    }
}

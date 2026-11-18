//! Connections: a storage backend plus its credentials, turned into an OpenDAL
//! [`Operator`].

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use opendal::layers::{RetryLayer, TimeoutLayer};
use opendal::{Operator, services};
use serde::{Deserialize, Serialize};

/// Folder inside each local connection used for atomic writes; never listed.
pub const ATOMIC_TMP_DIR: &str = ".strata-tmp";

/// Folder at a connection's root reserved for health probes (see `probe`).
/// Nothing else is ever written there and it is hidden from listings.
pub const PROBE_DIR: &str = ".strata-probe";

/// Is a `/`-separated path (from the connection's root) inside the probe
/// folder?
pub fn is_probe_path(path: &str) -> bool {
    path.trim_start_matches('/').split('/').next() == Some(PROBE_DIR)
}

/// Does a `/`-separated path contain the reserved atomic-write folder?
pub fn has_reserved_component(path: &str) -> bool {
    path.split('/').any(|c| c == ATOMIC_TMP_DIR)
}

/// Backend configuration. Serialised to JSON and encrypted at rest.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ConnectionConfig {
    /// A folder on the server. `path` is relative to the server's
    /// `STRATA_LOCAL_BASE`, so users cannot reach arbitrary server paths.
    Local { path: String },
    S3 {
        bucket: String,
        #[serde(default)]
        region: Option<String>,
        #[serde(default)]
        endpoint: Option<String>,
        access_key_id: String,
        secret_access_key: String,
        #[serde(default)]
        root: Option<String>,
    },
    Ftp {
        /// e.g. `ftp://host:21` or `ftps://host`
        endpoint: String,
        #[serde(default)]
        user: Option<String>,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        root: Option<String>,
    },
}

impl ConnectionConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local",
            Self::S3 { .. } => "s3",
            Self::Ftp { .. } => "ftp",
        }
    }

    /// Where this connection's files physically live, as (storage identity,
    /// folder inside it). Two connections with the same identity reach the
    /// same files, so their folders can overlap (e.g. two users' WebDAV or S3
    /// connections to one share or bucket). Credentials are not part of it.
    pub fn location(&self, local_base: &Path) -> Result<(String, String)> {
        Ok(match self {
            // Relative to the storage base (every local connection lives below
            // it), so moving the base doesn't make stored locations disagree.
            Self::Local { path } => {
                self.local_dir(local_base)?; // validates the path
                ("local".into(), canonical_folder(path)?)
            }
            Self::S3 {
                bucket,
                endpoint,
                root,
                ..
            } => {
                let host = match endpoint.as_deref() {
                    None | Some("") => "s3.amazonaws.com".to_string(),
                    Some(e) => endpoint_host(e, &[80, 443]),
                };
                // Regional AWS endpoints name the same bucket.
                let host = if host.ends_with(".amazonaws.com") {
                    "s3.amazonaws.com".into()
                } else {
                    host
                };
                (format!("s3:{host}|{bucket}"), remote_root(root)?)
            }
            Self::Ftp { endpoint, root, .. } => (
                format!("ftp:{}", endpoint_host(endpoint, &[21, 990])),
                remote_root(root)?,
            ),
        })
    }

    /// A copy with secrets blanked, safe to send to the UI.
    pub fn redacted(&self) -> Self {
        let mut c = self.clone();
        match &mut c {
            Self::Local { .. } => {}
            Self::S3 {
                secret_access_key, ..
            } => *secret_access_key = String::new(),
            Self::Ftp { password, .. } => {
                if password.is_some() {
                    *password = Some(String::new())
                }
            }
        }
        c
    }

    /// Keep secrets from `old` when the UI sends a blank (redacted) value.
    pub fn merge_secrets(&mut self, old: &Self) {
        match (self, old) {
            (
                Self::S3 {
                    secret_access_key, ..
                },
                Self::S3 {
                    secret_access_key: o,
                    ..
                },
            ) if secret_access_key.is_empty() => *secret_access_key = o.clone(),
            (Self::Ftp { password, .. }, Self::Ftp { password: o, .. })
                if password.as_deref() == Some("") =>
            {
                *password = o.clone()
            }
            _ => {}
        }
    }

    /// Resolve a local connection's directory under `local_base`.
    pub fn local_dir(&self, local_base: &Path) -> Result<Option<PathBuf>> {
        let Self::Local { path } = self else {
            return Ok(None);
        };
        let rel = Path::new(path.trim_start_matches('/'));
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            bail!("local path must stay inside the storage base");
        }
        let slashed = path.replace('\\', "/");
        if has_reserved_component(&slashed) {
            bail!("{ATOMIC_TMP_DIR} is reserved");
        }
        if slashed.split('/').any(|c| c == PROBE_DIR) {
            bail!("{PROBE_DIR} is reserved");
        }
        Ok(Some(local_base.join(rel)))
    }

    /// Create a local connection's folder (when the connection is added or
    /// re-pointed). Nothing for other kinds.
    pub fn create_local_root(&self, local_base: &Path) -> Result<()> {
        if let Some(dir) = self.local_dir(local_base)? {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Build an OpenDAL operator rooted at this connection.
    pub fn operator(&self, local_base: &Path) -> Result<Operator> {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(opendal::install_default);
        let op = match self {
            Self::Local { .. } => {
                let dir = self.local_dir(local_base)?.unwrap();
                // Writes land in a temp file and are renamed into place, so an
                // interrupted upload never leaves a partial file at the target.
                // Inside the connection's own folder: same filesystem, so the
                // final rename never crosses devices.
                let tmp = dir.join(ATOMIC_TMP_DIR);
                // Never (re)create the connection's folder here: if it is
                // missing (an unmounted disk), creating it would put files on
                // the wrong disk. `create_local_root` makes it, once, when the
                // connection is set up; the health probes see it go missing,
                // and until it is back no operator can be built for it.
                // (OpenDAL's Fs backend would create a missing root itself.)
                if !dir.is_dir() {
                    bail!("the connection's folder is missing");
                }
                std::fs::create_dir_all(&tmp)?;
                Operator::new(
                    services::Fs::default()
                        .root(&dir.to_string_lossy())
                        .atomic_write_dir(&tmp.to_string_lossy()),
                )?
            }
            Self::S3 {
                bucket,
                region,
                endpoint,
                access_key_id,
                secret_access_key,
                root,
            } => {
                let mut b = services::S3::default()
                    .bucket(bucket)
                    .access_key_id(access_key_id)
                    .secret_access_key(secret_access_key)
                    .region(region.as_deref().unwrap_or("us-east-1"));
                if let Some(e) = endpoint.as_deref().filter(|e| !e.is_empty()) {
                    b = b.endpoint(e);
                }
                // The canonical root, exactly what `location` records.
                let r = remote_root(root)?;
                if !r.is_empty() {
                    b = b.root(&format!("/{r}"));
                }
                Operator::new(b)?
            }
            Self::Ftp {
                endpoint,
                user,
                password,
                root,
            } => {
                let mut b = services::Ftp::default().endpoint(endpoint);
                if let Some(u) = user.as_deref().filter(|u| !u.is_empty()) {
                    b = b.user(u);
                }
                if let Some(p) = password.as_deref().filter(|p| !p.is_empty()) {
                    b = b.password(p);
                }
                // The canonical root, exactly what `location` records.
                let r = remote_root(root)?;
                if !r.is_empty() {
                    b = b.root(&format!("/{r}"));
                }
                Operator::new(b)?
            }
        };
        Ok(op
            .layer(TimeoutLayer::new().with_timeout(Duration::from_secs(60)))
            .layer(RetryLayer::new().with_max_times(3)))
    }
}

/// The canonical root folder of a remote (S3/FTP) connection: the one value
/// both `operator` and `location` use.
fn remote_root(root: &Option<String>) -> Result<String> {
    let r = canonical_folder(root.as_deref().unwrap_or(""))?;
    if r.split('/').any(|c| c == PROBE_DIR) {
        bail!("{PROBE_DIR} is reserved");
    }
    Ok(r)
}

/// The one canonical form of a folder inside a storage, for every backend:
/// `a//b`, `/a/./b/` and `a/b` are the same place. `..` is refused (FTP
/// servers resolve it, S3 keys don't), here and when building an operator,
/// so a config that has one can't reach any storage at all.
pub fn canonical_folder(p: &str) -> Result<String> {
    let parts: Vec<&str> = p
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if parts.contains(&"..") {
        bail!("folder paths may not contain '..'");
    }
    Ok(parts.join("/"))
}

/// `host[:port]` of an endpoint URL, lowercased, without scheme, path or a
/// default port, so equivalent spellings of one server compare equal.
fn endpoint_host(url: &str, default_ports: &[u16]) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("").to_ascii_lowercase();
    match host.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok_and(|p| default_ports.contains(&p)) => h.to_string(),
        _ => host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_path_cannot_escape() {
        let c = ConnectionConfig::Local {
            path: "../etc".into(),
        };
        assert!(c.local_dir(Path::new("/data")).is_err());
        let c = ConnectionConfig::Local {
            path: "/users/a".into(),
        };
        assert_eq!(
            c.local_dir(Path::new("/data")).unwrap().unwrap(),
            Path::new("/data/users/a")
        );
    }

    #[test]
    fn redact_and_merge() {
        let old = ConnectionConfig::S3 {
            bucket: "b".into(),
            region: None,
            endpoint: None,
            access_key_id: "a".into(),
            secret_access_key: "s".into(),
            root: None,
        };
        let mut new = old.redacted();
        new.merge_secrets(&old);
        assert_eq!(new, old);
        let ftp = |password: Option<&str>| ConnectionConfig::Ftp {
            endpoint: "ftp://h".into(),
            user: None,
            password: password.map(Into::into),
            root: None,
        };
        let mut new = ftp(Some("p")).redacted();
        assert_eq!(new, ftp(Some("")));
        new.merge_secrets(&ftp(Some("p")));
        assert_eq!(new, ftp(Some("p")));
        // No password stays none.
        let mut new = ftp(None).redacted();
        new.merge_secrets(&ftp(Some("p")));
        assert_eq!(new, ftp(None));
    }

    #[test]
    fn same_remote_storage_has_the_same_location() {
        let s3 = |key: &str, root: Option<&str>| ConnectionConfig::S3 {
            bucket: "b".into(),
            region: None,
            endpoint: Some("https://S3.example.com/".into()),
            access_key_id: key.into(),
            secret_access_key: "s".into(),
            root: root.map(str::to_string),
        };
        let base = Path::new("/data");
        assert_eq!(
            s3("alice", Some("/x/")).location(base).unwrap(),
            s3("bob", Some("x")).location(base).unwrap()
        );
        assert_ne!(
            s3("a", None).location(base).unwrap().1,
            s3("a", Some("y")).location(base).unwrap().1
        );
        let ftp = |e: &str| ConnectionConfig::Ftp {
            endpoint: e.into(),
            user: Some("u".into()),
            password: None,
            root: None,
        };
        assert_eq!(
            ftp("ftp://NAS:21").location(base).unwrap(),
            ("ftp:nas".to_string(), String::new())
        );
        assert_eq!(
            ftp("ftp://nas").location(base).unwrap(),
            ftp("ftps://nas:990/").location(base).unwrap()
        );
        let aws = |e: Option<&str>| ConnectionConfig::S3 {
            bucket: "b".into(),
            region: None,
            endpoint: e.map(str::to_string),
            access_key_id: "k".into(),
            secret_access_key: "s".into(),
            root: None,
        };
        assert_eq!(
            aws(None).location(base).unwrap(),
            aws(Some("https://s3.eu-west-1.amazonaws.com"))
                .location(base)
                .unwrap()
        );
        assert_eq!(
            aws(Some("http://minio:9000")).location(base).unwrap(),
            aws(Some("https://MINIO:9000/")).location(base).unwrap()
        );
        let local = |p: &str| {
            ConnectionConfig::Local { path: p.into() }
                .location(base)
                .unwrap()
        };
        assert_eq!(local("store//users/5"), local("store/users/5"));
        assert_eq!(local("store/users/5/"), local("/store/users/5"));
        assert_eq!(
            s3("a", Some("data//store/")).location(base).unwrap(),
            s3("a", Some("/data/store")).location(base).unwrap()
        );
        assert!(s3("a", Some("pub/../store")).location(base).is_err());
        assert!(s3("a", Some("pub/../store")).operator(base).is_err());
        assert_eq!(
            s3("a", Some("./data/.")).location(base).unwrap(),
            s3("a", Some("data")).location(base).unwrap()
        );
    }
}

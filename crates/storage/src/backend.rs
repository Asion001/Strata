//! Connections: a storage backend plus its credentials, turned into an OpenDAL
//! [`Operator`].

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use opendal::layers::{RetryLayer, TimeoutLayer};
use opendal::{Operator, services};
use serde::{Deserialize, Serialize};

use crate::readonly::ReadOnlyLayer;

/// Folder inside each local connection used for atomic writes; never listed.
pub const ATOMIC_TMP_DIR: &str = ".strata-tmp";

/// Does a `/`-separated path contain the reserved atomic-write folder?
pub fn has_reserved_component(path: &str) -> bool {
    path.split('/').any(|c| c == ATOMIC_TMP_DIR)
}

/// Errors that mean "this folder exists but we may not write to it".
fn is_read_only_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ReadOnlyFilesystem | std::io::ErrorKind::PermissionDenied
    )
}

/// Is the filesystem holding `p` mounted read-only (statvfs ST_RDONLY)?
/// Unlike permission checks this does not depend on who we run as, so it
/// cannot misjudge a folder we can in fact write.
#[cfg(unix)]
fn mounted_read_only(p: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return false;
    };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return false;
    }
    #[allow(clippy::unnecessary_cast)]
    let ro = (s.f_flag as u64) & (libc::ST_RDONLY as u64) != 0;
    ro
}

#[cfg(not(unix))]
fn mounted_read_only(_: &Path) -> bool {
    false
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
        if has_reserved_component(&path.replace('\\', "/")) {
            bail!("{ATOMIC_TMP_DIR} is reserved");
        }
        Ok(Some(local_base.join(rel)))
    }

    /// Build an OpenDAL operator rooted at this connection.
    pub fn operator(&self, local_base: &Path) -> Result<Operator> {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(opendal::install_default);
        let op = match self {
            Self::Local { .. } => {
                let dir = self.local_dir(local_base)?.unwrap();
                std::fs::create_dir_all(&dir)?;
                // Writes land in a temp file and are renamed into place, so an
                // interrupted upload never leaves a partial file at the target.
                // Inside the connection's own folder: same filesystem, so the
                // final rename never crosses devices.
                let tmp = dir.join(ATOMIC_TMP_DIR);
                // `create_dir_all` succeeds without writing when `.strata-tmp`
                // is left over from a time the folder was writable, so also
                // check whether the folder's filesystem is mounted read-only.
                let writable = std::fs::create_dir_all(&tmp).and_then(|()| {
                    if mounted_read_only(&dir) {
                        Err(std::io::ErrorKind::ReadOnlyFilesystem.into())
                    } else {
                        Ok(())
                    }
                });
                match writable {
                    Ok(()) => Operator::new(
                        services::Fs::default()
                            .root(&dir.to_string_lossy())
                            .atomic_write_dir(&tmp.to_string_lossy()),
                    )?,
                    // A folder we may read but not write (e.g. bind-mounted
                    // read-only): offer it for browsing and downloads only.
                    // Without an atomic write folder a write would go straight
                    // to the target, so the read-only layer refuses them all.
                    Err(e) if is_read_only_error(&e) && dir.is_dir() => {
                        Operator::new(services::Fs::default().root(&dir.to_string_lossy()))?
                            .layer(ReadOnlyLayer)
                    }
                    Err(e) => return Err(e.into()),
                }
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
    canonical_folder(root.as_deref().unwrap_or(""))
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

/// Native snapshot support a backend offers (used from phase 2 on).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotKind {
    #[default]
    None,
    Btrfs,
    Zfs,
    S3Versioning,
    Smb,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub snapshots: SnapshotKind,
    /// Whether Strata can create snapshots itself (vs. only read existing ones).
    pub snapshots_create: bool,
    pub rename: bool,
    pub range_read: bool,
    /// Files can be listed and read but nothing can be written (e.g. a local
    /// folder mounted read-only on the server).
    #[serde(default)]
    pub read_only: bool,
    /// Filesystem type name for local connections (e.g. "ext4", "btrfs").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_type: Option<String>,
}

impl Capabilities {
    /// Detect what a connection can do. For local folders this inspects the
    /// filesystem with statfs(2) to spot btrfs and ZFS.
    pub fn detect(cfg: &ConnectionConfig, op: &Operator, local_base: &Path) -> Self {
        let cap = op.info().capability();
        let mut c = Capabilities {
            rename: cap.rename,
            range_read: cap.read,
            read_only: !cap.write,
            ..Default::default()
        };
        if let Ok(Some(dir)) = cfg.local_dir(local_base) {
            let fs = fs_type(&dir);
            c.snapshots = match fs.as_deref() {
                Some("btrfs") => SnapshotKind::Btrfs,
                Some("zfs") => SnapshotKind::Zfs,
                _ => SnapshotKind::None,
            };
            // Creating btrfs/ZFS snapshots needs privileges we do not assume yet.
            c.snapshots_create = false;
            c.fs_type = fs;
        }
        c
    }
}

#[cfg(target_os = "linux")]
pub fn fs_type(dir: &Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(
        match s.f_type as i64 {
            0x9123_683E => "btrfs",
            0x2FC1_2FC1 => "zfs",
            0xEF53 => "ext4",
            0x5846_5342 => "xfs",
            0x0102_1994 => "tmpfs",
            0x794c_7630 => "overlayfs",
            other => return Some(format!("0x{other:x}")),
        }
        .to_string(),
    )
}

#[cfg(target_os = "macos")]
pub fn fs_type(dir: &Path) -> Option<String> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let name = unsafe { CStr::from_ptr(s.f_fstypename.as_ptr()) };
    Some(name.to_string_lossy().into_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn fs_type(_dir: &Path) -> Option<String> {
    None
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

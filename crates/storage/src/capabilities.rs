//! What a connection can do. Detected when a connection is created and again
//! on every health probe (a backend can change: a bucket gets versioning, a
//! folder moves to another filesystem).

use std::path::{Path, PathBuf};
use std::time::Duration;

use opendal::Operator;
use serde::{Deserialize, Serialize};

use crate::backend::{ConnectionConfig, PROBE_DIR};

/// Native snapshot support a backend offers.
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Snapshots {
    pub kind: SnapshotKind,
    /// Whether Strata can create snapshots itself (vs. only read existing
    /// ones). Only set after a real test snapshot (btrfs) or permission
    /// check (ZFS) succeeded.
    pub create: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Capabilities {
    pub snapshots: Snapshots,
    pub rename: bool,
    pub range_read: bool,
    /// A write becomes visible all at once (never half-written).
    pub atomic_write: bool,
    /// Listing a folder is cheap (one request, not one per entry).
    pub list_fast: bool,
    /// Filesystem type of a local connection (e.g. "ext4", "btrfs", "apfs").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fs_type: Option<String>,
}

impl Capabilities {
    pub async fn detect(cfg: &ConnectionConfig, op: &Operator, local_base: &Path) -> Self {
        let cap = op.info().capability();
        let mut c = Capabilities {
            rename: cap.rename,
            range_read: cap.read,
            ..Default::default()
        };
        match cfg {
            ConnectionConfig::Local { .. } => {
                // Local writes go to a temp file renamed into place.
                c.atomic_write = true;
                c.list_fast = true;
                if let Ok(Some(dir)) = cfg.local_dir(local_base) {
                    let fs = fs_type(&dir);
                    c.snapshots = match fs.as_deref() {
                        Some("btrfs") => Snapshots {
                            kind: SnapshotKind::Btrfs,
                            create: btrfs_can_snapshot(&dir).await,
                        },
                        Some("zfs") => Snapshots {
                            kind: SnapshotKind::Zfs,
                            create: zfs_can_snapshot(&dir).await,
                        },
                        _ => Snapshots::default(),
                    };
                    c.fs_type = fs;
                }
            }
            ConnectionConfig::S3 { .. } => {
                // A PUT replaces the whole object at once.
                c.atomic_write = true;
                c.list_fast = true;
                if s3_versioning_enabled(cfg).await {
                    c.snapshots.kind = SnapshotKind::S3Versioning;
                }
            }
            ConnectionConfig::Ftp { .. } => {}
        }
        c
    }
}

/// Filesystem type of a folder, from statfs(2): `f_type` on Linux (btrfs is
/// 0x9123683E, ZFS 0x2FC12FC1), `f_fstypename` on macOS.
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
    Some(fs_name(s.f_type as i64))
}

/// Name for a Linux statfs `f_type` magic number.
pub fn fs_name(magic: i64) -> String {
    match magic {
        0x9123_683E => "btrfs",
        0x2FC1_2FC1 => "zfs",
        0xEF53 => "ext4",
        0x5846_5342 => "xfs",
        0x0102_1994 => "tmpfs",
        0x794c_7630 => "overlayfs",
        other => return format!("0x{other:x}"),
    }
    .to_string()
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

async fn run(cmd: &str, args: &[&std::ffi::OsStr]) -> Option<String> {
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(cmd)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Can this process create btrfs snapshots here? Only a real one proves it:
/// a scratch subvolume and a read-only snapshot of it, both removed again,
/// inside the hidden probe folder.
async fn btrfs_can_snapshot(dir: &Path) -> bool {
    let probe = dir.join(PROBE_DIR);
    // Leftovers of an earlier, interrupted test go first.
    sweep_btrfs_scratch(&probe).await;
    let made_probe_dir = !probe.exists();
    if tokio::fs::create_dir_all(&probe).await.is_err() {
        return false;
    }
    let tag: u64 = rand::random();
    let sub = probe.join(format!("snap-{tag:016x}"));
    let snap = probe.join(format!("snap-{tag:016x}-s"));
    let ok = run(
        "btrfs",
        &["subvolume".as_ref(), "create".as_ref(), sub.as_os_str()],
    )
    .await
    .is_some()
        && run(
            "btrfs",
            &[
                "subvolume".as_ref(),
                "snapshot".as_ref(),
                "-r".as_ref(),
                sub.as_os_str(),
                snap.as_os_str(),
            ],
        )
        .await
        .is_some();
    let cleaned = remove_subvolume(&snap).await & remove_subvolume(&sub).await;
    if made_probe_dir {
        let _ = tokio::fs::remove_dir(&probe).await;
    }
    ok && cleaned
}

/// Remove an (empty) scratch subvolume: `btrfs subvolume delete` where
/// allowed, else rmdir (which removes an empty subvolume its owner made on
/// current kernels). True once it is gone.
async fn remove_subvolume(p: &PathBuf) -> bool {
    if !p.exists() {
        return true;
    }
    if run(
        "btrfs",
        &["subvolume".as_ref(), "delete".as_ref(), p.as_os_str()],
    )
    .await
    .is_some()
    {
        return true;
    }
    tokio::fs::remove_dir(p).await.is_ok() || !p.exists()
}

async fn sweep_btrfs_scratch(probe: &Path) {
    let Ok(mut rd) = tokio::fs::read_dir(probe).await else {
        return;
    };
    while let Ok(Some(e)) = rd.next_entry().await {
        if e.file_name().to_string_lossy().starts_with("snap-") {
            remove_subvolume(&e.path()).await;
        }
    }
}

/// Can this process create ZFS snapshots of the dataset holding `dir`?
/// Root can; anyone else needs the `snapshot` permission from `zfs allow`.
async fn zfs_can_snapshot(dir: &Path) -> bool {
    let Some(ds) = run(
        "zfs",
        &[
            "list".as_ref(),
            "-H".as_ref(),
            "-o".as_ref(),
            "name".as_ref(),
            dir.as_os_str(),
        ],
    )
    .await
    else {
        return false;
    };
    let ds = ds.trim();
    if ds.is_empty() {
        return false;
    }
    if unsafe { libc::geteuid() } == 0 {
        return true;
    }
    let Some(allow) = run("zfs", &["allow".as_ref(), ds.as_ref()]).await else {
        return false;
    };
    let user = std::env::var("USER").unwrap_or_default();
    zfs_allows_snapshot(&allow, &user)
}

/// Does `zfs allow` output grant `snapshot` to `user` (or everyone)? Group
/// grants aren't resolved: they count as no.
pub fn zfs_allows_snapshot(output: &str, user: &str) -> bool {
    output.lines().any(|l| {
        let mut w = l.split_whitespace();
        let who = match (w.next(), w.next()) {
            (Some("user"), Some(u)) => !user.is_empty() && u == user,
            (Some("everyone"), _) => true,
            _ => return false,
        };
        let perms = if l.trim_start().starts_with("everyone") {
            l.trim_start().trim_start_matches("everyone")
        } else {
            l.trim_start()
                .splitn(3, char::is_whitespace)
                .nth(2)
                .unwrap_or("")
        };
        who && perms.split(',').any(|p| p.trim() == "snapshot")
    })
}

/// GetBucketVersioning: is versioning on for the connection's bucket? Any
/// failure (no permission, not S3) counts as no.
async fn s3_versioning_enabled(cfg: &ConnectionConfig) -> bool {
    let ConnectionConfig::S3 {
        bucket,
        region,
        endpoint,
        access_key_id,
        secret_access_key,
        ..
    } = cfg
    else {
        return false;
    };
    let region = region
        .as_deref()
        .filter(|r| !r.is_empty())
        .unwrap_or("us-east-1");
    let base = match endpoint.as_deref().filter(|e| !e.is_empty()) {
        Some(e) => e.trim_end_matches('/').to_string(),
        None => format!("https://s3.{region}.amazonaws.com"),
    };
    let Ok(url) = reqwest::Url::parse(&format!("{base}/{bucket}?versioning")) else {
        return false;
    };
    let now = chrono::Utc::now();
    let headers = crate::sigv4::sign_get(&url, region, access_key_id, secret_access_key, now);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => r
            .text()
            .await
            .map(|t| versioning_status_enabled(&t))
            .unwrap_or(false),
        _ => false,
    }
}

/// `<VersioningConfiguration><Status>Enabled</Status>…`: only "Enabled"
/// counts (a suspended or never-enabled bucket keeps no new versions).
pub fn versioning_status_enabled(xml: &str) -> bool {
    xml.split("<Status>")
        .nth(1)
        .and_then(|s| s.split("</Status>").next())
        .is_some_and(|s| s.trim() == "Enabled")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zfs_allow_parsing() {
        let out = "---- Permissions on tank/data ---------------------------------------\n\
                   Local+Descendent permissions:\n\
                   \tuser strata create,mount,snapshot\n\
                   \tuser other destroy\n";
        assert!(zfs_allows_snapshot(out, "strata"));
        assert!(!zfs_allows_snapshot(out, "other"));
        assert!(!zfs_allows_snapshot(out, ""));
        assert!(zfs_allows_snapshot(
            "Local permissions:\n\teveryone snapshot,send\n",
            "x"
        ));
        assert!(!zfs_allows_snapshot(
            "Local permissions:\n\tgroup staff snapshot\n",
            "x"
        ));
        assert!(!zfs_allows_snapshot("\tuser strata snapshotx\n", "strata"));
    }

    #[test]
    fn versioning_status() {
        let on = r#"<?xml version="1.0"?><VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>Enabled</Status></VersioningConfiguration>"#;
        assert!(versioning_status_enabled(on));
        assert!(!versioning_status_enabled(
            &on.replace("Enabled", "Suspended")
        ));
        assert!(!versioning_status_enabled(
            r#"<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"/>"#
        ));
    }

    #[test]
    fn fs_names() {
        assert_eq!(fs_name(0x9123683E), "btrfs");
        assert_eq!(fs_name(0x2FC12FC1), "zfs");
        assert_eq!(fs_name(0xEF53), "ext4");
    }

    #[tokio::test]
    async fn local_detection() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ConnectionConfig::Local { path: "".into() };
        let op = cfg.operator(dir.path()).unwrap();
        let c = Capabilities::detect(&cfg, &op, dir.path()).await;
        assert!(c.rename && c.range_read && c.atomic_write && c.list_fast);
        let fs = c.fs_type.clone().unwrap();
        #[cfg(target_os = "macos")]
        assert_eq!(fs, "apfs");
        #[cfg(target_os = "linux")]
        assert!(!fs.is_empty());
        if fs != "btrfs" && fs != "zfs" {
            assert_eq!(c.snapshots, Snapshots::default());
        }
        // Nothing left behind.
        assert!(!dir.path().join(PROBE_DIR).exists());
    }

    /// btrfs on a loop device: CI (Linux) sets STRATA_TEST_BTRFS_DIR to a
    /// writable folder on a btrfs mount.
    #[tokio::test]
    async fn btrfs_detection() {
        let Ok(dir) = std::env::var("STRATA_TEST_BTRFS_DIR") else {
            eprintln!("skipped: set STRATA_TEST_BTRFS_DIR to a folder on btrfs (Linux CI does)");
            return;
        };
        let dir = PathBuf::from(dir);
        let cfg = ConnectionConfig::Local { path: "".into() };
        let op = cfg.operator(&dir).unwrap();
        let c = Capabilities::detect(&cfg, &op, &dir).await;
        assert_eq!(c.fs_type.as_deref(), Some("btrfs"));
        assert_eq!(c.snapshots.kind, SnapshotKind::Btrfs);
        assert!(c.snapshots.create, "the test snapshot should succeed");
        assert!(
            !dir.join(PROBE_DIR).exists(),
            "scratch subvolumes left behind"
        );
    }
}

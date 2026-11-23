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
    /// Files can be listed and read but nothing can be written (e.g. a local
    /// folder mounted read-only on the server).
    pub read_only: bool,
    /// Filesystem type of a local connection (e.g. "ext4", "btrfs", "apfs").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fs_type: Option<String>,
    /// Whether deleting a file leaves nothing behind (no object versions or
    /// delete markers). False when that can't be confirmed; health probes
    /// only write test files where it is true.
    #[serde(skip)]
    pub clean_deletes: bool,
}

/// A bucket's versioning state from GetBucketVersioning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Versioning {
    /// Never enabled: a delete removes the object for good.
    Off,
    Enabled,
    /// Was enabled: deletes still leave delete markers.
    Suspended,
    /// The request failed (no permission, timeout).
    Unknown,
}

impl Capabilities {
    /// Detect what a connection supports. `prev` is the last detection for
    /// the same config: a snapshot test that already ran on the same
    /// filesystem isn't repeated, and an S3 versioning state that can't be
    /// read this time (throttled, timed out) keeps its last known value.
    pub async fn detect(
        cfg: &ConnectionConfig,
        op: &Operator,
        local_base: &Path,
        prev: Option<&Capabilities>,
    ) -> Self {
        let cap = op.info().capability();
        let mut c = Capabilities {
            rename: cap.rename,
            range_read: cap.read,
            read_only: !cap.write,
            ..Default::default()
        };
        match cfg {
            ConnectionConfig::Local { .. } => {
                // Local writes go to a temp file renamed into place.
                c.atomic_write = true;
                c.list_fast = true;
                c.clean_deletes = true;
                if let Ok(Some(dir)) = cfg.local_dir(local_base) {
                    let fs = fs_type_bounded(&dir).await;
                    // A test that worked isn't repeated on the same
                    // filesystem; one that failed (maybe transiently: tools
                    // not installed yet, a timeout) is tried again.
                    let known = prev
                        .filter(|p| p.fs_type.is_some() && p.fs_type == fs && p.snapshots.create);
                    c.snapshots = match fs.as_deref() {
                        // Same filesystem as last time: no new test snapshot.
                        Some("btrfs" | "zfs") if known.is_some() => known.unwrap().snapshots,
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
                let v = s3_versioning(cfg).await;
                c.snapshots.kind = match v {
                    Versioning::Enabled => SnapshotKind::S3Versioning,
                    Versioning::Unknown => prev.map(|p| p.snapshots.kind).unwrap_or_default(),
                    Versioning::Off | Versioning::Suspended => SnapshotKind::None,
                };
                c.clean_deletes = v == Versioning::Off;
            }
            ConnectionConfig::Ftp { .. } => c.clean_deletes = true,
        }
        // Nothing is written to a read-only connection, tests included.
        if c.read_only {
            c.clean_deletes = false;
        }
        c
    }
}

/// `fs_type` off the async threads, with a time limit (a hung network mount
/// must not block the server).
async fn fs_type_bounded(dir: &Path) -> Option<String> {
    let dir = dir.to_path_buf();
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || fs_type(&dir)),
    )
    .await
    .ok()?
    .ok()?
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
/// a scratch subvolume and a snapshot of it, both removed again, inside the
/// hidden probe folder. The snapshot is writable, so an empty one can be
/// removed with rmdir even where `btrfs subvolume delete` isn't allowed.
async fn btrfs_can_snapshot(dir: &Path) -> bool {
    let probe = dir.join(PROBE_DIR);
    // Leftovers of an earlier, interrupted test go first. If one can't be
    // removed, no new test runs (it would only add another).
    if !sweep_btrfs_scratch(&probe).await {
        return false;
    }
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

/// Remove scratch subvolumes earlier tests left. True when none remain.
async fn sweep_btrfs_scratch(probe: &Path) -> bool {
    let Ok(mut rd) = tokio::fs::read_dir(probe).await else {
        return true; // no probe folder: nothing left over
    };
    let mut clean = true;
    while let Ok(Some(e)) = rd.next_entry().await {
        if e.file_name().to_string_lossy().starts_with("snap-") {
            clean &= remove_subvolume(&e.path()).await;
        }
    }
    clean
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
    zfs_allows_snapshot(&allow, ds, &effective_user().unwrap_or_default())
}

/// The name of the effective user (not `$USER`, which may be unset or
/// belong to someone else after su/sudo).
fn effective_user() -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut pw,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut out,
        )
    };
    if rc != 0 || out.is_null() {
        return None;
    }
    Some(
        unsafe { std::ffi::CStr::from_ptr(pw.pw_name) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Does `zfs allow` output grant `snapshot` and `mount` (ZFS needs both to
/// create a snapshot) to `user` (or everyone) on dataset `ds`? Only grants
/// that apply to `ds` count: its own "Local" and "Local+Descendent"
/// sections, and an ancestor's "Descendent" and "Local+Descendent" ones.
/// Group grants aren't resolved: they count as no.
pub fn zfs_allows_snapshot(output: &str, ds: &str, user: &str) -> bool {
    let mut perms: Vec<&str> = Vec::new();
    let mut on: Option<&str> = None; // dataset of the current block
    let mut applies = false;
    for l in output.lines() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("---- Permissions on ") {
            on = rest.split_whitespace().next();
            applies = false;
            continue;
        }
        if t.ends_with("permissions:") {
            let own = on == Some(ds);
            let ancestor = on.is_some_and(|o| ds.starts_with(&format!("{o}/")));
            applies = match t {
                "Local permissions:" => own,
                "Descendent permissions:" => ancestor,
                "Local+Descendent permissions:" => own || ancestor,
                _ => false,
            };
            continue;
        }
        if !applies {
            continue;
        }
        let rest = if let Some(r) = t.strip_prefix("everyone") {
            r
        } else if let Some(r) = t.strip_prefix("user ") {
            let mut parts = r.trim_start().splitn(2, char::is_whitespace);
            match (parts.next(), parts.next()) {
                (Some(u), Some(p)) if !user.is_empty() && u == user => p,
                _ => continue,
            }
        } else {
            continue;
        };
        perms.extend(rest.split(',').map(str::trim));
    }
    perms.contains(&"snapshot") && perms.contains(&"mount")
}

/// GetBucketVersioning for the connection's bucket.
async fn s3_versioning(cfg: &ConnectionConfig) -> Versioning {
    let ConnectionConfig::S3 {
        bucket,
        region,
        endpoint,
        access_key_id,
        secret_access_key,
        ..
    } = cfg
    else {
        return Versioning::Unknown;
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
        return Versioning::Unknown;
    };
    let now = chrono::Utc::now();
    let headers = crate::sigv4::sign_get(&url, region, access_key_id, secret_access_key, now);
    // One client for all probes (TLS setup once).
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    let client = match CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .ok()
        })
        .clone()
    {
        Some(c) => c,
        None => return Versioning::Unknown,
    };
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(r) if r.status().is_success() => match r.text().await {
            Ok(t) => versioning_status(&t),
            Err(_) => Versioning::Unknown,
        },
        _ => Versioning::Unknown,
    }
}

/// `<VersioningConfiguration><Status>Enabled</Status>…`; no Status at all
/// means versioning was never turned on.
pub fn versioning_status(xml: &str) -> Versioning {
    if !xml.contains("VersioningConfiguration") {
        return Versioning::Unknown;
    }
    match xml
        .split("<Status>")
        .nth(1)
        .and_then(|s| s.split("</Status>").next())
        .map(str::trim)
    {
        None => Versioning::Off,
        Some("Enabled") => Versioning::Enabled,
        Some("Suspended") => Versioning::Suspended,
        Some(_) => Versioning::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zfs_allow_parsing() {
        let own = "---- Permissions on tank/data ---------------------------------------\n\
                   Local+Descendent permissions:\n\
                   \tuser strata create,mount,snapshot\n\
                   \tuser other destroy\n";
        assert!(zfs_allows_snapshot(own, "tank/data", "strata"));
        assert!(!zfs_allows_snapshot(own, "tank/data", "other"));
        assert!(!zfs_allows_snapshot(own, "tank/data", ""));
        // snapshot without mount isn't enough
        let p =
            |body: &str| format!("---- Permissions on tank/data ----\nLocal permissions:\n{body}");
        assert!(!zfs_allows_snapshot(
            &p("\teveryone snapshot,send\n"),
            "tank/data",
            "x"
        ));
        assert!(zfs_allows_snapshot(
            &p("\teveryone snapshot,send\n\tuser x mount\n"),
            "tank/data",
            "x"
        ));
        assert!(!zfs_allows_snapshot(
            &p("\tgroup staff snapshot,mount\n"),
            "tank/data",
            "x"
        ));
        assert!(!zfs_allows_snapshot(
            &p("\tuser strata snapshotx,mount\n"),
            "tank/data",
            "strata"
        ));
        // an ancestor's grants: only the descendent ones reach the child
        let anc = |kind: &str| {
            format!("---- Permissions on tank ----\n{kind} permissions:\n\tuser x snapshot,mount\n")
        };
        assert!(!zfs_allows_snapshot(&anc("Local"), "tank/data", "x"));
        assert!(zfs_allows_snapshot(&anc("Descendent"), "tank/data", "x"));
        assert!(zfs_allows_snapshot(
            &anc("Local+Descendent"),
            "tank/data",
            "x"
        ));
        // the dataset's own descendent-only grants don't apply to it
        let own_desc = "---- Permissions on tank/data ----\nDescendent permissions:\n\tuser x snapshot,mount\n";
        assert!(!zfs_allows_snapshot(own_desc, "tank/data", "x"));
        // a sibling with a shared prefix isn't an ancestor
        let sib = "---- Permissions on tank/da ----\nLocal+Descendent permissions:\n\tuser x snapshot,mount\n";
        assert!(!zfs_allows_snapshot(sib, "tank/data", "x"));
    }

    #[test]
    fn parses_versioning() {
        let on = r#"<?xml version="1.0"?><VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Status>Enabled</Status></VersioningConfiguration>"#;
        assert_eq!(versioning_status(on), Versioning::Enabled);
        assert_eq!(
            versioning_status(&on.replace("Enabled", "Suspended")),
            Versioning::Suspended
        );
        assert_eq!(
            versioning_status(
                r#"<VersioningConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/"/>"#
            ),
            Versioning::Off
        );
        assert_eq!(
            versioning_status("<Error><Code>AccessDenied</Code></Error>"),
            Versioning::Unknown
        );
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
        let c = Capabilities::detect(&cfg, &op, dir.path(), None).await;
        assert!(c.rename && c.range_read && c.atomic_write && c.list_fast && c.clean_deletes);
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
        let c = Capabilities::detect(&cfg, &op, &dir, None).await;
        assert_eq!(c.fs_type.as_deref(), Some("btrfs"));
        assert_eq!(c.snapshots.kind, SnapshotKind::Btrfs);
        assert!(c.snapshots.create, "the test snapshot should succeed");
        assert!(
            !dir.join(PROBE_DIR).exists(),
            "scratch subvolumes left behind"
        );
    }
}

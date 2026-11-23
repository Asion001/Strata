//! Local folders Strata may read but not write are offered read-only, and the
//! read-only layer refuses every change.
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use opendal::{Operator, services};
use strata_storage::readonly::{ReadOnlyLayer, is_read_only_refusal};
use strata_storage::{Capabilities, ConnectionConfig};

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type().unwrap().is_dir() {
            for (n, b) in snapshot(&e.path()) {
                out.push((format!("{name}/{n}"), b));
            }
            out.push((format!("{name}/"), Vec::new()));
        } else {
            out.push((name, std::fs::read(e.path()).unwrap()));
        }
    }
    out.sort();
    out
}

async fn assert_refuses_writes(op: &Operator, dir: &Path) {
    let before = snapshot(dir);
    let refused = |r: opendal::Result<()>| {
        let e = r.expect_err("must be refused");
        assert!(is_read_only_refusal(&e), "{e}");
    };
    refused(op.write("new.txt", vec![1u8]).await.map(|_| ()));
    refused(op.write("a.txt", vec![1u8]).await.map(|_| ()));
    refused(op.writer_with("big.bin").chunk(8).await.map(|_| ()));
    refused(op.create_dir("newdir/").await);
    refused(op.delete("a.txt").await);
    refused(op.delete_with("sub/").recursive(true).await);
    refused(op.rename("a.txt", "b.txt").await);
    refused(op.copy("a.txt", "b.txt").await.map(|_| ()));
    assert_eq!(snapshot(dir), before, "nothing may change on disk");

    // reading and listing still work
    assert_eq!(op.read("a.txt").await.unwrap().to_vec(), b"hello");
    assert_eq!(
        op.read_with("a.txt").range(1..3).await.unwrap().to_vec(),
        b"el"
    );
    assert_eq!(op.stat("sub/b.txt").await.unwrap().content_length(), 3);
    let mut names: Vec<_> = op
        .list("/")
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.path().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["/", "a.txt", "sub/"]);

    let cap = op.info().capability();
    assert!(!cap.write && !cap.delete && !cap.rename && !cap.copy && !cap.create_dir);
    assert!(cap.read && cap.list && cap.stat);
}

fn fill(dir: &Path) {
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), b"hello").unwrap();
    std::fs::write(dir.join("sub/b.txt"), b"abc").unwrap();
}

#[tokio::test]
async fn layer_refuses_every_change_even_on_a_writable_folder() {
    let d = tempfile::tempdir().unwrap();
    fill(d.path());
    let op = Operator::new(services::Fs::default().root(&d.path().to_string_lossy()))
        .unwrap()
        .layer(ReadOnlyLayer);
    assert_refuses_writes(&op, d.path()).await;
}

#[tokio::test]
async fn other_errors_are_not_read_only_refusals() {
    let d = tempfile::tempdir().unwrap();
    let op = Operator::new(services::Fs::default().root(&d.path().to_string_lossy())).unwrap();
    let e = op.read("missing").await.unwrap_err();
    assert!(!is_read_only_refusal(&e));
}

#[tokio::test]
async fn writable_local_folder_is_not_read_only() {
    let base = tempfile::tempdir().unwrap();
    let cfg = ConnectionConfig::Local { path: "rw".into() };
    cfg.create_local_root(base.path()).unwrap();
    let op = cfg.operator(base.path()).unwrap();
    assert!(base.path().join("rw/.strata-tmp").is_dir());
    assert!(
        !Capabilities::detect(&cfg, &op, base.path(), None)
            .await
            .read_only
    );
    op.write("x.txt", vec![1u8]).await.unwrap();
}

#[tokio::test]
async fn unwritable_local_folder_opens_read_only() {
    // root ignores permission bits, so this needs an ordinary user (as in CI)
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: running as root");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("ro");
    fill(&dir);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let cfg = ConnectionConfig::Local { path: "ro".into() };
    let op = cfg
        .operator(base.path())
        .expect("a read-only folder still opens");
    assert!(!dir.join(".strata-tmp").exists());
    let caps = Capabilities::detect(&cfg, &op, base.path(), None).await;
    assert!(caps.read_only);
    assert!(!caps.rename);
    assert_refuses_writes(&op, &dir).await;

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn missing_folder_under_unwritable_base_is_an_error() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: running as root");
        return;
    }
    let base = tempfile::tempdir().unwrap();
    std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let cfg = ConnectionConfig::Local {
        path: "nope".into(),
    };
    assert!(cfg.operator(base.path()).is_err());
    std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
}

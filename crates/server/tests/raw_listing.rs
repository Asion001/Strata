//! Raw folder listings: rescans write only what changed, and a stale listing
//! is answered from the cache while the folder is rescanned in the background.
mod common;

use common::*;
use serde_json::json;
use std::collections::HashMap;
use std::time::{Duration, Instant};

async fn raw_mount(t: &TestApp) -> std::path::PathBuf {
    let v = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "Disk", "config": {"kind": "local", "path": "disk"}}),
        )
        .await;
    let cid = v["id"].as_i64().unwrap();
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(disk.join("dl/sub")).unwrap();
    std::fs::write(disk.join("dl/a.txt"), b"a").unwrap();
    std::fs::write(disk.join("dl/b.txt"), b"b").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "D", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    disk
}

/// name -> the transaction id that last wrote the row
async fn row_versions(t: &TestApp) -> HashMap<String, String> {
    sqlx::query_as::<_, (String, String)>("SELECT name, xmin::text FROM nodes WHERE parent = 'dl'")
        .fetch_all(&t.st.db)
        .await
        .unwrap()
        .into_iter()
        .collect()
}

#[tokio::test]
async fn rescan_rewrites_only_changed_rows() {
    let Some(t) = app().await else { return };
    let disk = raw_mount(&t).await;
    assert_eq!(t.names("D/dl").await, vec!["sub", "a.txt", "b.txt"]);
    let before = row_versions(&t).await;

    // Nothing changed: TTL 0 rescans, and no row is written again.
    assert_eq!(t.names("D/dl").await, vec!["sub", "a.txt", "b.txt"]);
    assert_eq!(row_versions(&t).await, before);

    // One file changed, one added, one removed.
    std::fs::write(disk.join("dl/b.txt"), b"bigger").unwrap();
    std::fs::write(disk.join("dl/c.txt"), b"c").unwrap();
    std::fs::remove_file(disk.join("dl/a.txt")).unwrap();
    assert_eq!(t.names("D/dl").await, vec!["sub", "b.txt", "c.txt"]);
    let after = row_versions(&t).await;
    assert_eq!(after["sub"], before["sub"]);
    assert_ne!(after["b.txt"], before["b.txt"]);
    let (_, v) = t.get("/api/files/list?path=D%2Fdl").await;
    let b = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "b.txt")
        .unwrap()
        .clone();
    assert_eq!(b["size"], 6);

    // A file that became a folder is rewritten too.
    std::fs::remove_file(disk.join("dl/c.txt")).unwrap();
    std::fs::create_dir(disk.join("dl/c.txt")).unwrap();
    let (_, v) = t.get("/api/files/list?path=D%2Fdl").await;
    let c = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "c.txt")
        .unwrap()
        .clone();
    assert_eq!(c["is_dir"], true);
}

#[tokio::test]
async fn stale_listing_is_served_from_cache_and_rescanned_in_background() {
    let Some(t) = app_with(|c| c.raw_background_rescan = true).await else {
        return;
    };
    let disk = raw_mount(&t).await;
    let mut events = t.st.events.subscribe();
    // Never scanned: the first open waits for the scan.
    assert_eq!(t.names("D/dl").await, vec!["sub", "a.txt", "b.txt"]);

    std::fs::write(disk.join("dl/c.txt"), b"c").unwrap();
    // Stale (TTL 0): answered at once from the cache ...
    let first = t.names("D/dl").await;
    assert!(
        first == vec!["sub", "a.txt", "b.txt"] || first == vec!["sub", "a.txt", "b.txt", "c.txt"],
        "{first:?}"
    );
    // ... and the background rescan catches up and tells clients.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if t.names("D/dl").await == vec!["sub", "a.txt", "b.txt", "c.txt"] {
            break;
        }
        assert!(Instant::now() < deadline, "background rescan never landed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let told = tokio::time::timeout(Duration::from_secs(2), events.recv()).await;
    assert!(matches!(told, Ok(Ok(_))), "no change event for the rescan");
    // Rescans finish and clear their running mark.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !t.st.rescans.is_empty() {
        assert!(Instant::now() < deadline, "rescan mark left behind");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // A folder that never was scanned and does not exist is still an error.
    let (s, _) = t.get("/api/files/list?path=D%2Fnope").await;
    assert_eq!(s, axum::http::StatusCode::NOT_FOUND);
}

//! Trash: deletes go to a per-user trash that can be restored, purged and
//! emptied, on hashed volumes and raw local folders; crashes mid-way lose
//! nothing (see `trash::reconcile`).

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

async fn local_conn(t: &TestApp, name: &str, path: &str) -> i64 {
    let v = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": name, "config": {"kind": "local", "path": path}}),
        )
        .await;
    v["id"].as_i64().unwrap()
}

async fn items(t: &TestApp) -> Vec<Value> {
    let (s, v) = t.get("/api/trash").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["items"].as_array().unwrap().clone()
}

fn id_of(items: &[Value], original: &str) -> i64 {
    items
        .iter()
        .find(|i| i["original_path"] == original)
        .unwrap_or_else(|| panic!("{original} not in trash: {items:?}"))["id"]
        .as_i64()
        .unwrap()
}

async fn delete(t: &TestApp, paths: &[&str]) -> Value {
    t.ok("POST", "/api/files/delete", json!({ "paths": paths }))
        .await
}

async fn restore(t: &TestApp, id: i64) -> String {
    let v = t
        .ok("POST", &format!("/api/trash/{id}/restore"), json!({}))
        .await;
    v["path"].as_str().unwrap().to_string()
}

async fn count(t: &TestApp, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(&t.st.db).await.unwrap()
}

#[tokio::test]
async fn hashed_delete_goes_to_trash_and_restores_intact() {
    let Some(t) = app().await else { return };
    let big = pseudo(9 * 1024 * 1024, 7);
    t.upload("Docs/a.txt", b"alpha").await;
    t.upload("Docs/sub/big.bin", &big).await;
    t.upload("note.md", b"note").await;

    let v = delete(&t, &["Docs", "note.md"]).await;
    assert_eq!(v["trashed"], 2);
    assert_eq!(t.names("").await, Vec::<String>::new());
    let (_, hits) = t.get("/api/search?q=big").await;
    assert_eq!(
        hits["hits"].as_array().unwrap().len(),
        0,
        "trash is not searchable"
    );

    let list = items(&t).await;
    assert_eq!(list.len(), 2);
    let docs = list.iter().find(|i| i["original_path"] == "Docs").unwrap();
    assert_eq!(docs["is_dir"], true);
    assert_eq!(docs["size"], (5 + big.len()) as i64);
    assert_eq!(docs["name"], "Docs");

    // GC keeps the blobs of trashed files.
    strata_server::background::gc(&t.st, 0).await.unwrap();

    assert_eq!(restore(&t, id_of(&list, "Docs")).await, "Docs");
    assert_eq!(t.download("Docs/a.txt").await, b"alpha");
    assert_eq!(t.download("Docs/sub/big.bin").await, big);
    assert_eq!(items(&t).await.len(), 1);
    assert_eq!(count(&t, "SELECT count(*) FROM trash_nodes").await, 1);

    // The change feed says delete, then create; never a hidden path.
    let (_, c) = t.get("/api/changes").await;
    let cursor = c["cursor"].as_str().unwrap().to_string();
    delete(&t, &["Docs/a.txt"]).await;
    let id = id_of(&items(&t).await, "Docs/a.txt");
    restore(&t, id).await;
    let (_, c) = t
        .get(&format!("/api/changes?cursor={}", enc(&cursor)))
        .await;
    let ops: Vec<(String, String)> = c["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["op"].as_str().unwrap().to_string(),
                x["path"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        ops,
        vec![
            ("delete".into(), "Docs/a.txt".into()),
            ("create".into(), "Docs/a.txt".into())
        ]
    );
}

#[tokio::test]
async fn restore_takes_a_free_name_and_recreates_parents() {
    let Some(t) = app().await else { return };
    t.upload("A/B/c.txt", b"old").await;
    delete(&t, &["A/B/c.txt"]).await;
    t.upload("A/B/c.txt", b"new").await;
    let id = id_of(&items(&t).await, "A/B/c.txt");
    assert_eq!(restore(&t, id).await, "A/B/c (restored).txt");
    assert_eq!(t.download("A/B/c.txt").await, b"new");
    assert_eq!(t.download("A/B/c (restored).txt").await, b"old");

    // A second collision gets the next number.
    delete(&t, &["A/B/c (restored).txt"]).await;
    t.upload("A/B/c (restored).txt", b"other").await;
    let id = id_of(&items(&t).await, "A/B/c (restored).txt");
    assert_eq!(restore(&t, id).await, "A/B/c (restored) (restored).txt");

    // The parent was deleted too (permanently): it comes back.
    t.upload("X/Y/z.txt", b"z").await;
    delete(&t, &["X/Y/z.txt"]).await;
    t.ok(
        "POST",
        "/api/files/delete",
        json!({"paths": ["X"], "permanent": true}),
    )
    .await;
    assert_eq!(t.names("").await, vec!["A"]);
    let id = id_of(&items(&t).await, "X/Y/z.txt");
    assert_eq!(restore(&t, id).await, "X/Y/z.txt");
    assert_eq!(t.download("X/Y/z.txt").await, b"z");

    // A folder restored under a new name keeps its whole subtree.
    t.upload("F/1/2.txt", b"two").await;
    delete(&t, &["F"]).await;
    t.ok("POST", "/api/files/mkdir", json!({"path": "F"})).await;
    let id = id_of(&items(&t).await, "F");
    assert_eq!(restore(&t, id).await, "F (restored)");
    assert_eq!(t.download("F (restored)/1/2.txt").await, b"two");
    // Folder names keep their dots.
    t.ok("POST", "/api/files/mkdir", json!({"path": "v1.2"}))
        .await;
    delete(&t, &["v1.2"]).await;
    t.ok("POST", "/api/files/mkdir", json!({"path": "v1.2"}))
        .await;
    let id = id_of(&items(&t).await, "v1.2");
    assert_eq!(restore(&t, id).await, "v1.2 (restored)");
}

#[tokio::test]
async fn purge_and_empty_free_blobs_and_permanent_skips_trash() {
    let Some(t) = app().await else { return };
    t.upload("a.bin", &pseudo(300_000, 1)).await;
    t.upload("b.bin", &pseudo(300_000, 2)).await;
    t.upload("c.bin", &pseudo(300_000, 3)).await;
    let v = t
        .ok(
            "POST",
            "/api/files/delete",
            json!({"paths": ["c.bin"], "permanent": true}),
        )
        .await;
    assert_eq!(v["trashed"], 0);
    delete(&t, &["a.bin", "b.bin"]).await;
    let list = items(&t).await;
    assert_eq!(list.len(), 2, "permanent deletes skip the trash");

    let (s, _) = t
        .json(
            "DELETE",
            &format!("/api/trash/{}", id_of(&list, "a.bin")),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(items(&t).await.len(), 1);
    let v = t.ok("POST", "/api/trash/empty", json!({})).await;
    assert_eq!(v["purged"], 1);
    assert!(items(&t).await.is_empty());
    assert_eq!(count(&t, "SELECT count(*) FROM trash_nodes").await, 0);
    strata_server::background::gc(&t.st, 0).await.unwrap();
    assert_eq!(count(&t, "SELECT count(*) FROM blobs").await, 0);
}

#[tokio::test]
async fn expired_items_are_purged() {
    let Some(t) = app().await else { return };
    t.upload("old.txt", b"old").await;
    t.upload("fresh.txt", b"fresh").await;
    delete(&t, &["old.txt", "fresh.txt"]).await;
    sqlx::query(
        "UPDATE trash SET purge_after = now() - interval '1 minute' WHERE name = 'old.txt'",
    )
    .execute(&t.st.db)
    .await
    .unwrap();
    let n = strata_server::trash::purge_expired(&t.st).await.unwrap();
    assert_eq!(n, 1);
    let list = items(&t).await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "fresh.txt");
    // Retention is reported for the UI.
    let (_, v) = t.get("/api/trash").await;
    assert_eq!(v["retention_days"], 30);
}

#[tokio::test]
async fn raw_local_trash_moves_on_disk_and_back() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(disk.join("photos/2024")).unwrap();
    std::fs::write(disk.join("photos/2024/a.jpg"), b"jpeg").unwrap();
    std::fs::write(disk.join("readme.txt"), b"hello").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let (_, l) = t.get("/api/files/list?path=Disk").await;
    assert_eq!(l["mount"]["trash"], true);

    let v = delete(&t, &["Disk/photos", "Disk/readme.txt"]).await;
    assert_eq!(v["trashed"], 2);
    assert!(!disk.join("photos").exists());
    assert!(!disk.join("readme.txt").exists());
    let list = items(&t).await;
    let pid = id_of(&list, "Disk/photos");
    let rid = id_of(&list, "Disk/readme.txt");
    let tdir = disk.join(".strata-trash");
    assert_eq!(
        std::fs::read(tdir.join(format!("{pid}/photos/2024/a.jpg"))).unwrap(),
        b"jpeg"
    );
    // The trash folder never shows up, nor can it be reached by path.
    assert_eq!(t.names("Disk").await, Vec::<String>::new());
    assert_eq!(
        t.get("/api/files/list?path=Disk/.strata-trash").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        t.get(&format!(
            "/api/files/content?path={}",
            enc(&format!("Disk/.strata-trash/{rid}/readme.txt"))
        ))
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Restore: byte-identical, in place, and the item folder is cleaned up.
    assert_eq!(restore(&t, pid).await, "Disk/photos");
    assert_eq!(
        std::fs::read(disk.join("photos/2024/a.jpg")).unwrap(),
        b"jpeg"
    );
    assert!(!tdir.join(pid.to_string()).exists());
    assert_eq!(t.names("Disk").await, vec!["photos"]);
    assert_eq!(t.download("Disk/photos/2024/a.jpg").await, b"jpeg");

    // A file created outside Strata meanwhile is never replaced.
    std::fs::write(disk.join("readme.txt"), b"newer").unwrap();
    assert_eq!(restore(&t, rid).await, "Disk/readme (restored).txt");
    assert_eq!(std::fs::read(disk.join("readme.txt")).unwrap(), b"newer");
    assert_eq!(
        std::fs::read(disk.join("readme (restored).txt")).unwrap(),
        b"hello"
    );

    // Purge removes it from disk.
    delete(&t, &["Disk/photos"]).await;
    let pid = id_of(&items(&t).await, "Disk/photos");
    assert!(tdir.join(pid.to_string()).exists());
    t.ok("POST", "/api/trash/empty", json!({})).await;
    assert!(!tdir.join(pid.to_string()).exists());
    assert!(items(&t).await.is_empty());
}

#[tokio::test]
async fn crashes_mid_trash_lose_nothing() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(&disk).unwrap();
    std::fs::write(disk.join("kept.txt"), b"kept").unwrap();
    std::fs::write(disk.join("moved.txt"), b"moved").unwrap();
    std::fs::write(disk.join("gone.txt"), b"gone").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let uid: i64 = count(&t, "SELECT id FROM users WHERE username = 'admin'").await;
    let vid: i64 = count(&t, "SELECT volume_id FROM mounts WHERE path = 'Disk'").await;
    let row = |name: &'static str, ready: bool| {
        let st = t.st.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO trash (owner_id, volume_id, original_path, rel_path, name, is_dir, ready, purge_after)
                 VALUES ($1, $2, 'Disk/' || $3, $3, $3, FALSE, $4, now() + interval '30 days') RETURNING id",
            )
            .bind(uid)
            .bind(vid)
            .bind(name)
            .bind(ready)
            .fetch_one(&st.db)
            .await
            .unwrap()
        }
    };
    // 1. Row written, crash before the rename: the file is still in place.
    let a = row("kept.txt", false).await;
    // 2. Crash after the rename, before the row was marked ready.
    let b = row("moved.txt", false).await;
    let bdir = disk.join(format!(".strata-trash/{b}"));
    std::fs::create_dir_all(&bdir).unwrap();
    std::fs::rename(disk.join("moved.txt"), bdir.join("moved.txt")).unwrap();
    // 3. A ready row whose item was removed outside Strata (or a purge that
    //    crashed after removing the files).
    let c = row("gone.txt", true).await;
    std::fs::remove_file(disk.join("gone.txt")).unwrap();

    assert_eq!(items(&t).await.len(), 1, "unready rows are not listed");
    let fixed = strata_server::trash::reconcile(&t.st).await.unwrap();
    assert_eq!(fixed, 3);
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM trash ORDER BY id")
        .fetch_all(&t.st.db)
        .await
        .unwrap();
    assert_eq!(ids, vec![b]);
    assert!(ids.iter().all(|i| *i != a && *i != c));
    assert_eq!(std::fs::read(disk.join("kept.txt")).unwrap(), b"kept");
    assert_eq!(restore(&t, b).await, "Disk/moved.txt");
    assert_eq!(std::fs::read(disk.join("moved.txt")).unwrap(), b"moved");
    // Nothing left to fix.
    assert_eq!(strata_server::trash::reconcile(&t.st).await.unwrap(), 0);
}

#[tokio::test]
async fn trash_is_private_and_refuses_what_it_cannot_do() {
    let Some(t) = app().await else { return };
    t.upload("secret.txt", b"mine").await;
    delete(&t, &["secret.txt"]).await;
    let id = id_of(&items(&t).await, "secret.txt");
    t.ok(
        "POST",
        "/api/admin/users",
        json!({"username": "eve", "password": "evepassword"}),
    )
    .await;
    let mut eve = TestApp {
        router: t.router.clone(),
        st: t.st.clone(),
        cookie: None,
        dir: tempfile::tempdir().unwrap(),
    };
    eve.login("eve", "evepassword").await;
    assert!(items(&eve).await.is_empty());
    let (s, _) = eve
        .json("POST", &format!("/api/trash/{id}/restore"), json!({}))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = eve
        .json("DELETE", &format!("/api/trash/{id}"), json!({}))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    eve.ok("POST", "/api/trash/empty", json!({})).await;
    assert_eq!(items(&t).await.len(), 1, "eve's empty leaves admin's trash");
    let (s, _) = t.json("POST", "/api/trash/999999/restore", json!({})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Mount points and the root are still refused, not trashed.
    let cid = local_conn(&t, "Disk", "disk").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "hashed"}),
    )
    .await;
    let (s, _) = t
        .json("POST", "/api/files/delete", json!({"paths": ["Disk"]}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = t
        .json("POST", "/api/files/delete", json!({"paths": [""]}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // An item from a mount that was removed can't be restored (yet stays).
    t.upload("Disk/x.txt", b"x").await;
    delete(&t, &["Disk/x.txt"]).await;
    let xid = id_of(&items(&t).await, "Disk/x.txt");
    let mid: i64 = count(&t, "SELECT id FROM mounts WHERE path = 'Disk'").await;
    t.ok("DELETE", &format!("/api/mounts/{mid}"), json!({}))
        .await;
    let (s, _) = t
        .json("POST", &format!("/api/trash/{xid}/restore"), json!({}))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(items(&t).await.len(), 2);
}

#[tokio::test]
async fn hashed_mount_on_a_local_connection_uses_the_db_trash() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Store", "store").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "S", "connection_id": cid, "mode": "hashed"}),
    )
    .await;
    // In hashed mounts `.strata-trash` is an ordinary name.
    t.upload("S/.strata-trash/mine.txt", b"mine").await;
    assert_eq!(t.download("S/.strata-trash/mine.txt").await, b"mine");
    delete(&t, &["S/.strata-trash"]).await;
    assert!(
        !t.dir.path().join("local/store/.strata-trash").exists(),
        "hashed trash never touches the backend"
    );
    let id = id_of(&items(&t).await, "S/.strata-trash");
    assert_eq!(restore(&t, id).await, "S/.strata-trash");
    assert_eq!(t.download("S/.strata-trash/mine.txt").await, b"mine");
}

#[tokio::test]
async fn trash_off_deletes_for_good() {
    let Some(t) = app_with(|c| c.trash_days = 0).await else {
        return;
    };
    t.upload("a.txt", b"a").await;
    let v = delete(&t, &["a.txt"]).await;
    assert_eq!(v["trashed"], 0);
    assert!(items(&t).await.is_empty());
    let (_, l) = t.get("/api/files/list?path=").await;
    assert_eq!(l["mount"]["trash"], false);
    let (_, v) = t.get("/api/trash").await;
    assert_eq!(v["retention_days"], 0);
}

#[tokio::test]
async fn trash_folder_is_reserved_for_local_connections_and_mounts() {
    let Some(t) = app().await else { return };
    let (s, _) = t
        .json(
            "POST",
            "/api/connections",
            json!({"name": "x", "config": {"kind": "local", "path": "files/.strata-trash"}}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let f = local_conn(&t, "Files", "files").await;
    let (s, _) = t
        .json(
            "POST",
            "/api/mounts",
            json!({"path": "T", "connection_id": f, "base_path": ".strata-trash", "mode": "raw"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unreachable_trash_folder_is_never_read_as_empty() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(&disk).unwrap();
    std::fs::write(disk.join("a.txt"), b"a").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    delete(&t, &["Disk/a.txt"]).await;
    let id = id_of(&items(&t).await, "Disk/a.txt");
    // The disk is "unmounted": its trash folder and the connection's root
    // marker are not there right now (an empty mount point).
    let tdir = disk.join(".strata-trash");
    let probe = disk.join(".strata-probe");
    assert!(probe.exists(), "the first probe writes the root marker");
    let away = t.dir.path().join("away");
    let away_probe = t.dir.path().join("away-probe");
    std::fs::rename(&tdir, &away).unwrap();
    std::fs::rename(&probe, &away_probe).unwrap();
    assert_eq!(strata_server::trash::reconcile(&t.st).await.unwrap(), 0);
    let (s, _) = t
        .json("DELETE", &format!("/api/trash/{id}"), json!({}))
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(items(&t).await.len(), 1, "the row survives");
    let (s, _) = t
        .json("POST", &format!("/api/trash/{id}/restore"), json!({}))
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    // Back again: everything is where it was.
    std::fs::rename(&away, &tdir).unwrap();
    std::fs::rename(&away_probe, &probe).unwrap();
    assert_eq!(restore(&t, id).await, "Disk/a.txt");
    assert_eq!(std::fs::read(disk.join("a.txt")).unwrap(), b"a");

    // With the disk there (marker present), a trash folder removed by hand
    // really is gone: its rows are dropped instead of blocking forever.
    delete(&t, &["Disk/a.txt"]).await;
    std::fs::remove_dir_all(&tdir).unwrap();
    assert_eq!(strata_server::trash::reconcile(&t.st).await.unwrap(), 1);
    assert!(items(&t).await.is_empty());
}

#[tokio::test]
async fn connection_with_trashed_items_cannot_be_deleted() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Store", "store").await;
    let v = t
        .ok(
            "POST",
            "/api/mounts",
            json!({"path": "S", "connection_id": cid, "mode": "hashed"}),
        )
        .await;
    t.upload("S/a.txt", b"a").await;
    delete(&t, &["S/a.txt"]).await;
    let mid = v["id"].as_i64().unwrap();
    t.ok("DELETE", &format!("/api/mounts/{mid}"), json!({}))
        .await;
    let (s, v) = t
        .json("DELETE", &format!("/api/connections/{cid}"), json!({}))
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(items(&t).await.len(), 1, "a refused delete keeps the trash");
    t.ok(
        "DELETE",
        &format!("/api/connections/{cid}?purge_trash=true"),
        json!({}),
    )
    .await;
    assert!(items(&t).await.is_empty());
}

#[tokio::test]
async fn deleting_a_user_removes_their_raw_trash_from_disk() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(&disk).unwrap();
    std::fs::write(disk.join("bob.txt"), b"bob").unwrap();
    let v = t
        .ok(
            "POST",
            "/api/admin/users",
            json!({"username": "bob", "password": "bobpassword"}),
        )
        .await;
    let bob_id = v["id"].as_i64().unwrap();
    // The admin shares the connection's folder with bob through his own mount.
    sqlx::query("UPDATE connections SET owner_id = $1 WHERE id = $2")
        .bind(bob_id)
        .bind(cid)
        .execute(&t.st.db)
        .await
        .unwrap();
    let mut bob = TestApp {
        router: t.router.clone(),
        st: t.st.clone(),
        cookie: None,
        dir: tempfile::tempdir().unwrap(),
    };
    bob.login("bob", "bobpassword").await;
    bob.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    delete(&bob, &["Disk/bob.txt"]).await;
    let id = id_of(&items(&bob).await, "Disk/bob.txt");
    let item = disk.join(format!(".strata-trash/{id}"));
    assert!(item.exists());
    // Bob still owns the connection: the delete is refused, the trash kept.
    let (s, _) = t
        .json("DELETE", &format!("/api/admin/users/{bob_id}"), json!({}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(item.exists(), "a refused delete keeps the trash");
    sqlx::query("UPDATE connections SET owner_id = (SELECT id FROM users WHERE username = 'admin') WHERE id = $1")
        .bind(cid)
        .execute(&t.st.db)
        .await
        .unwrap();
    t.ok("DELETE", &format!("/api/admin/users/{bob_id}"), json!({}))
        .await;
    assert!(!item.exists());
}

#[tokio::test]
async fn item_on_another_disk_is_refused_not_lost() {
    let Some(t) = app().await else { return };
    // A folder on another filesystem (tmpfs), reached through a symlink.
    let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
        return;
    };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(&disk).unwrap();
    std::fs::write(other.path().join("far.txt"), b"far").unwrap();
    std::os::unix::fs::symlink(other.path(), disk.join("other")).unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let (s, v) = t
        .json(
            "POST",
            "/api/files/delete",
            json!({"paths": ["Disk/other/far.txt"]}),
        )
        .await;
    if s == StatusCode::NOT_FOUND {
        return; // this backend does not follow symlinked folders
    }
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(std::fs::read(other.path().join("far.txt")).unwrap(), b"far");
    assert!(items(&t).await.is_empty());
}

#[tokio::test]
async fn share_links_follow_a_restored_item() {
    let Some(t) = app().await else { return };
    let anon = TestApp {
        router: t.router.clone(),
        st: t.st.clone(),
        cookie: None,
        dir: tempfile::tempdir().unwrap(),
    };
    // Hashed: a link to a file inside a trashed folder, and one to the folder.
    t.upload("Pub/doc.txt", b"doc").await;
    let f = t
        .ok("POST", "/api/shares", json!({"path": "Pub/doc.txt"}))
        .await;
    let d = t.ok("POST", "/api/shares", json!({"path": "Pub"})).await;
    let link = |v: &Value| format!("/api/public/{}", v["token"].as_str().unwrap());
    assert_eq!(anon.get(&link(&f)).await.0, StatusCode::OK);
    delete(&t, &["Pub"]).await;
    assert_eq!(anon.get(&link(&f)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(anon.get(&link(&d)).await.0, StatusCode::NOT_FOUND);
    restore(&t, id_of(&items(&t).await, "Pub")).await;
    assert_eq!(anon.get(&link(&f)).await.0, StatusCode::OK);
    assert_eq!(anon.get(&link(&d)).await.0, StatusCode::OK);

    // Restored under another name: the links stay dead.
    delete(&t, &["Pub/doc.txt"]).await;
    t.upload("Pub/doc.txt", b"someone else's").await;
    restore(&t, id_of(&items(&t).await, "Pub/doc.txt")).await;
    assert_eq!(anon.get(&link(&f)).await.0, StatusCode::NOT_FOUND);

    // Raw: a link to the trashed item itself comes back.
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(&disk).unwrap();
    std::fs::write(disk.join("r.txt"), b"raw").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let r = t
        .ok("POST", "/api/shares", json!({"path": "Disk/r.txt"}))
        .await;
    assert_eq!(anon.get(&link(&r)).await.0, StatusCode::OK);
    delete(&t, &["Disk/r.txt"]).await;
    assert_eq!(anon.get(&link(&r)).await.0, StatusCode::NOT_FOUND);
    restore(&t, id_of(&items(&t).await, "Disk/r.txt")).await;
    assert_eq!(anon.get(&link(&r)).await.0, StatusCode::OK);
}

#[tokio::test]
async fn racing_purge_after_restore_says_not_found() {
    let Some(t) = app().await else { return };
    t.upload("a.txt", b"a").await;
    delete(&t, &["a.txt"]).await;
    let id = id_of(&items(&t).await, "a.txt");
    restore(&t, id).await;
    assert!(matches!(
        strata_server::trash::purge(&t.st, id).await,
        Err(strata_server::error::ApiError::NotFound)
    ));
    assert_eq!(t.download("a.txt").await, b"a");
}

#[tokio::test]
async fn restored_folder_never_replaces_one_made_meanwhile() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(disk.join("f")).unwrap();
    std::fs::write(disk.join("f/in.txt"), b"in").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    delete(&t, &["Disk/f"]).await;
    // An empty folder of the same name appears outside Strata.
    std::fs::create_dir(disk.join("f")).unwrap();
    assert_eq!(
        restore(&t, id_of(&items(&t).await, "Disk/f")).await,
        "Disk/f (restored)"
    );
    assert!(disk.join("f").is_dir());
    assert_eq!(
        std::fs::read(disk.join("f (restored)/in.txt")).unwrap(),
        b"in"
    );
}

#[tokio::test]
async fn symlink_to_a_folder_restores_as_a_link() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(disk.join("real")).unwrap();
    std::fs::write(disk.join("real/x.txt"), b"x").unwrap();
    std::os::unix::fs::symlink("real", disk.join("link")).unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    delete(&t, &["Disk/link"]).await;
    assert!(disk.join("real/x.txt").exists(), "only the link moved");
    assert!(std::fs::symlink_metadata(disk.join("link")).is_err());
    assert_eq!(
        restore(&t, id_of(&items(&t).await, "Disk/link")).await,
        "Disk/link"
    );
    let md = std::fs::symlink_metadata(disk.join("link")).unwrap();
    assert!(md.file_type().is_symlink());
    assert_eq!(std::fs::read(disk.join("link/x.txt")).unwrap(), b"x");
}

#[tokio::test]
async fn connection_on_a_vanished_disk_can_still_be_deleted() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(&disk).unwrap();
    std::fs::write(disk.join("a.txt"), b"a").unwrap();
    let v = t
        .ok(
            "POST",
            "/api/mounts",
            json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
        )
        .await;
    delete(&t, &["Disk/a.txt"]).await;
    let mid = v["id"].as_i64().unwrap();
    t.ok("DELETE", &format!("/api/mounts/{mid}"), json!({}))
        .await;
    // The disk is gone for good: trash folder and marker with it.
    std::fs::remove_dir_all(disk.join(".strata-trash")).unwrap();
    std::fs::remove_dir_all(disk.join(".strata-probe")).unwrap();
    t.ok(
        "DELETE",
        &format!("/api/connections/{cid}?purge_trash=true"),
        json!({}),
    )
    .await;
    assert!(items(&t).await.is_empty());
}

#[tokio::test]
async fn restore_never_lands_under_or_on_a_new_mount() {
    let Some(t) = app().await else { return };
    t.upload("Docs/a.txt", b"a").await;
    t.upload("Pics/x.txt", b"x").await;
    delete(&t, &["Docs", "Pics/x.txt"]).await;
    t.ok(
        "POST",
        "/api/files/delete",
        json!({"paths": ["Pics"], "permanent": true}),
    )
    .await;
    let cid = local_conn(&t, "Disk", "disk").await;
    std::fs::create_dir_all(t.dir.path().join("local/disk")).unwrap();
    for at in ["Docs", "Pics"] {
        t.ok(
            "POST",
            "/api/mounts",
            json!({"path": at, "connection_id": cid, "mode": "raw"}),
        )
        .await;
    }
    let all = items(&t).await;
    // The name is taken by a mount: the folder comes back beside it.
    let docs = restore(&t, id_of(&all, "Docs")).await;
    assert_ne!(docs, "Docs");
    assert!(docs.starts_with("Docs"), "{docs}");
    assert_eq!(t.download(&format!("{docs}/a.txt")).await, b"a");
    // The parent is covered by a mount: refused, nothing written there.
    let (s, v) = t
        .json(
            "POST",
            &format!("/api/trash/{}/restore", id_of(&all, "Pics/x.txt")),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(!t.dir.path().join("local/disk/x.txt").exists());
    assert_eq!(items(&t).await.len(), 1, "the item stays in the trash");
}

#[tokio::test]
async fn orphan_trash_folders_are_swept() {
    let Some(t) = app().await else { return };
    local_conn(&t, "Disk", "disk").await;
    let orphan = t.dir.path().join("local/disk/.strata-trash/987654");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("left.txt"), b"left").unwrap();
    let other = t.dir.path().join("local/disk/.strata-trash/notes");
    std::fs::create_dir_all(&other).unwrap();
    assert_eq!(strata_server::trash::reconcile(&t.st).await.unwrap(), 1);
    assert!(!orphan.exists());
    assert!(
        other.exists(),
        "only folders named like trash ids are swept"
    );
}

#[tokio::test]
async fn reconcile_leaves_an_item_made_again_after_a_crash() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(&disk).unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let uid: i64 = count(&t, "SELECT id FROM users WHERE username = 'admin'").await;
    let vid: i64 = count(&t, "SELECT volume_id FROM mounts WHERE path = 'Disk'").await;
    // Crash after the rename, before the row was marked ready...
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO trash (owner_id, volume_id, original_path, rel_path, name, is_dir, ready, purge_after)
         VALUES ($1, $2, 'Disk/a.txt', 'a.txt', 'a.txt', FALSE, FALSE, now() + interval '30 days') RETURNING id",
    )
    .bind(uid)
    .bind(vid)
    .fetch_one(&t.st.db)
    .await
    .unwrap();
    let dir = disk.join(format!(".strata-trash/{id}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), b"old").unwrap();
    // ...then a new a.txt was uploaded before reconcile ran.
    t.upload("Disk/a.txt", b"new").await;
    let seq: i64 = count(&t, "SELECT COALESCE(MAX(id), 0) FROM journal").await;
    assert_eq!(strata_server::trash::reconcile(&t.st).await.unwrap(), 1);
    assert_eq!(
        count(
            &t,
            &format!("SELECT COUNT(*) FROM journal WHERE id > {seq}")
        )
        .await,
        0,
        "no delete is journaled for the new file"
    );
    assert!(t.names("Disk").await.contains(&"a.txt".to_string()));
    assert_eq!(t.download("Disk/a.txt").await, b"new");
    let all = items(&t).await;
    assert_eq!(
        restore(&t, id_of(&all, "Disk/a.txt")).await,
        "Disk/a (restored).txt"
    );
}

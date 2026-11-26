//! Name conflicts: a move, copy or upload onto a taken name fails with
//! `code: "exists"`, replaces (the old item to the trash, or deleted only once
//! the new one is complete), or keeps both under "name (2)". Never an
//! overwrite nobody asked for.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};

async fn trash_names(t: &TestApp) -> Vec<String> {
    let (_, v) = t.get("/api/trash").await;
    let mut n: Vec<String> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["original_path"].as_str().unwrap().to_string())
        .collect();
    n.sort();
    n
}

async fn op(t: &TestApp, what: &str, from: &str, to: &str, mode: &str) -> (StatusCode, Value) {
    t.json(
        "POST",
        &format!("/api/files/{what}"),
        json!({"from": from, "to": to, "on_conflict": mode}),
    )
    .await
}

async fn local_mount(t: &TestApp, mode: &str) -> std::path::PathBuf {
    let v = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "Disk", "config": {"kind": "local", "path": "disk"}}),
        )
        .await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(&disk).unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": v["id"], "mode": mode}),
    )
    .await;
    disk
}

/// Every mode of move and copy, under `root` ("" or a mount).
async fn move_and_copy_modes(t: &TestApp, root: &str) {
    let p = |s: &str| {
        if root.is_empty() {
            s.to_string()
        } else {
            format!("{root}/{s}")
        }
    };
    t.upload(&p("a/report.txt"), b"new").await;
    t.upload(&p("b/report.txt"), b"old").await;

    // Fail (the default): 409 with what is there, nothing changes.
    let (s, v) = t
        .json(
            "POST",
            "/api/files/copy",
            json!({"from": p("a/report.txt"), "to": p("b/report.txt")}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "exists");
    assert_eq!(v["details"]["path"], p("b/report.txt"));
    assert_eq!(v["details"]["is_dir"], false);
    assert_eq!(v["details"]["size"], 3);
    assert_eq!(t.download(&p("b/report.txt")).await, b"old");

    // Keep both: "report (2).txt", then "(3)".
    let (s, v) = op(t, "copy", &p("a/report.txt"), &p("b/report.txt"), "rename").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["path"], p("b/report (2).txt"));
    let (_, v) = op(t, "copy", &p("a/report.txt"), &p("b/report.txt"), "rename").await;
    assert_eq!(v["path"], p("b/report (3).txt"));
    assert_eq!(t.download(&p("b/report (3).txt")).await, b"new");
    assert_eq!(t.download(&p("b/report.txt")).await, b"old");

    // Replace: the new one takes the name, the old one is in the trash, and
    // no staged leftovers stay.
    let (s, v) = op(t, "move", &p("a/report.txt"), &p("b/report.txt"), "replace").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["path"], p("b/report.txt"));
    assert_eq!(t.download(&p("b/report.txt")).await, b"new");
    let gone = |path: String| async move {
        t.get(&format!("/api/files/content?path={}", enc(&path)))
            .await
            .0
    };
    assert_eq!(gone(p("a/report.txt")).await, StatusCode::NOT_FOUND);
    assert_eq!(
        t.names(&p("b")).await,
        vec!["report (2).txt", "report (3).txt", "report.txt"]
    );
    assert!(trash_names(t).await.contains(&p("b/report.txt")));

    // Copy onto itself, keeping both: a duplicate in place.
    let (_, v) = op(t, "copy", &p("b/report.txt"), &p("b/report.txt"), "rename").await;
    assert_eq!(v["path"], p("b/report (4).txt"));
    // ...but never replacing itself, nor (as a move) going nowhere.
    let (s, _) = op(t, "copy", &p("b/report.txt"), &p("b/report.txt"), "replace").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = op(t, "move", &p("b/report.txt"), &p("b/report.txt"), "rename").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // A folder replacing a folder: the whole old one goes to the trash.
    t.upload(&p("c/x.txt"), b"c").await;
    t.upload(&p("d/c/y.txt"), b"old c").await;
    let (s, v) = op(t, "copy", &p("c"), &p("d/c"), "replace").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(t.names(&p("d/c")).await, vec!["x.txt"]);
    assert_eq!(t.names(&p("c")).await, vec!["x.txt"]);
    assert!(trash_names(t).await.contains(&p("d/c")));

    // A folder can't be replaced by something inside it.
    let (s, v) = op(t, "copy", &p("d/c/x.txt"), &p("d/c"), "replace").await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(t.names(&p("d/c")).await, vec!["x.txt"]);
    assert!(t.names(&p("d")).await.iter().all(|n| !n.starts_with('.')));
}

#[tokio::test]
async fn move_and_copy_on_hashed_storage() {
    let Some(t) = app().await else { return };
    move_and_copy_modes(&t, "").await;
}

#[tokio::test]
async fn move_and_copy_on_a_raw_local_folder() {
    let Some(t) = app().await else { return };
    let disk = local_mount(&t, "raw").await;
    move_and_copy_modes(&t, "Disk").await;
    // On disk too, with nothing staged left behind.
    assert_eq!(std::fs::read(disk.join("b/report.txt")).unwrap(), b"new");
    let left: Vec<_> = std::fs::read_dir(disk.join("b"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn uploads_ask_at_create_and_again_at_commit() {
    let Some(t) = app().await else { return };
    t.upload("doc.txt", b"first").await;
    let spec =
        |d: &[u8]| json!([{ "hash": blake3::hash(d).to_hex().to_string(), "size": d.len() }]);

    // At create: 409 with what is there, before any byte is sent.
    let (s, v) = t
        .json(
            "POST",
            "/api/uploads",
            json!({"path": "doc.txt", "size": 6, "chunks": spec(b"second")}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "exists");
    assert_eq!(v["details"]["size"], 5);

    // Keep both and Replace, through the whole upload.
    t.upload_as("doc.txt", b"second", "rename").await;
    assert_eq!(t.download("doc (2).txt").await, b"second");
    t.upload_replacing("doc.txt", b"third").await;
    assert_eq!(t.download("doc.txt").await, b"third");
    assert_eq!(trash_names(&t).await, vec!["doc.txt"]);
    assert_eq!(t.names("").await, vec!["doc (2).txt", "doc.txt"]);

    // Free at create, taken by commit time: the commit asks.
    let v = t
        .ok(
            "POST",
            "/api/uploads",
            json!({"path": "late.txt", "size": 4, "chunks": spec(b"mine")}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_string();
    let h = blake3::hash(b"mine").to_hex().to_string();
    let (s, _, _) = t
        .raw(
            "PUT",
            &format!("/api/uploads/{id}/chunks/{h}"),
            axum::body::Body::from("mine"),
            &[],
        )
        .await;
    assert!(s.is_success());
    t.upload("late.txt", b"theirs").await;
    let commit = format!("/api/uploads/{id}/commit");
    let (s, v) = t.json("POST", &commit, json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["code"], "exists");
    assert_eq!(t.download("late.txt").await, b"theirs");
    // The upload is still there for the answer.
    let v = t
        .ok("POST", &commit, json!({"on_conflict": "rename"}))
        .await;
    assert_eq!(v["path"], "late (2).txt");
    assert_eq!(t.download("late (2).txt").await, b"mine");

    // A file never replaces a folder.
    t.ok("POST", "/api/files/mkdir", json!({"path": "dir"}))
        .await;
    let (s, v) = t
        .json(
            "POST",
            "/api/uploads",
            json!({"path": "dir", "size": 4, "chunks": spec(b"mine"), "on_conflict": "replace"}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    t.upload_as("dir", b"mine", "rename").await;
    assert_eq!(t.download("dir (2)").await, b"mine");
}

#[tokio::test]
async fn replace_without_a_trash_deletes_the_old_one_only_after() {
    let Some(t) = app_with(|c| c.trash_days = 0).await else {
        return;
    };
    let disk = local_mount(&t, "raw").await;
    t.upload("Disk/a/f.txt", b"new").await;
    t.upload("Disk/b/f.txt", b"old").await;
    let (s, v) = op(&t, "move", "Disk/a/f.txt", "Disk/b/f.txt", "replace").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(std::fs::read(disk.join("b/f.txt")).unwrap(), b"new");
    assert_eq!(t.names("Disk/b").await, vec!["f.txt"]);
    assert!(!disk.join("a/f.txt").exists());
    // An upload too.
    t.upload_replacing("Disk/b/f.txt", b"newer").await;
    assert_eq!(std::fs::read(disk.join("b/f.txt")).unwrap(), b"newer");
    assert_eq!(t.names("Disk/b").await, vec!["f.txt"]);
}

#[tokio::test]
async fn a_failed_replace_keeps_the_old_item_and_the_source() {
    let Some(t) = app().await else { return };
    let disk = local_mount(&t, "raw").await;
    t.upload("Disk/src/f.txt", b"new").await;
    t.upload("Disk/dst/f/inner.txt", b"old").await;
    // A mount inside the folder being replaced: it can't be deleted.
    let v = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "Other", "config": {"kind": "local", "path": "other"}}),
        )
        .await;
    std::fs::create_dir_all(t.dir.path().join("local/other")).unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk/dst/f/m", "connection_id": v["id"], "mode": "raw"}),
    )
    .await;
    let (s, v) = op(&t, "move", "Disk/src/f.txt", "Disk/dst/f", "replace").await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    // The source is back where it was, the old folder untouched, nothing staged.
    assert_eq!(std::fs::read(disk.join("src/f.txt")).unwrap(), b"new");
    assert_eq!(std::fs::read(disk.join("dst/f/inner.txt")).unwrap(), b"old");
    assert_eq!(t.names("Disk/dst").await, vec!["f"]);
    // A copy cleans up its staged copy.
    let (s, _) = op(&t, "copy", "Disk/src/f.txt", "Disk/dst/f", "replace").await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(t.names("Disk/dst").await, vec!["f"]);
    assert_eq!(t.names("Disk/src").await, vec!["f.txt"]);
}

#[tokio::test]
async fn racing_keep_both_get_two_names() {
    let Some(t) = app().await else { return };
    t.upload("src/a.txt", &pseudo(2_000_000, 1)).await;
    t.upload("dst/a.txt", b"there").await;
    for _ in 0..3 {
        let (x, y) = tokio::join!(
            op(&t, "copy", "src/a.txt", "dst/a.txt", "rename"),
            op(&t, "copy", "src/a.txt", "dst/a.txt", "rename"),
        );
        assert_eq!(x.0, StatusCode::OK, "{}", x.1);
        assert_eq!(y.0, StatusCode::OK, "{}", y.1);
        assert_ne!(x.1["path"], y.1["path"]);
    }
    let names = t.names("dst").await;
    assert_eq!(names.len(), 7, "{names:?}");
    assert_eq!(t.download("dst/a.txt").await, b"there");
}

#[tokio::test]
async fn mount_points_are_never_replaced() {
    let Some(t) = app().await else { return };
    local_mount(&t, "raw").await;
    t.upload("x/Disk", b"file").await;
    let (s, v) = op(&t, "move", "x/Disk", "Disk", "replace").await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (s, v) = op(&t, "copy", "x/Disk", "Disk", "rename").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["path"], "Disk (2)");
}

#[tokio::test]
async fn names_in_flight_are_never_reused() {
    use strata_server::conflicts::{OnConflict, place};
    let Some(t) = app().await else { return };
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    // A name another operation is writing: Fail says so, Keep both skips it.
    let held = place(&t.st, uid, "f.txt", false, OnConflict::Fail)
        .await
        .unwrap();
    assert_eq!(held.path, "f.txt");
    let busy = place(&t.st, uid, "f.txt", false, OnConflict::Fail).await;
    assert!(matches!(
        busy,
        Err(strata_server::error::ApiError::Conflict(_))
    ));
    let next = place(&t.st, uid, "f.txt", false, OnConflict::Rename)
        .await
        .unwrap();
    assert_eq!(next.path, "f (2).txt");
    // Released when the operation ends.
    drop(held);
    assert_eq!(
        place(&t.st, uid, "f.txt", false, OnConflict::Fail)
            .await
            .unwrap()
            .path,
        "f.txt"
    );
    drop(next);

    // Every number taken: refused, not looped forever.
    let (m, _) = strata_server::mounts::resolve(&t.st, uid, "g.txt")
        .await
        .unwrap();
    let vid = m.volume_id;
    t.st.claims.insert((vid, "g.txt".into()), ());
    for n in 2..1000 {
        t.st.claims.insert((vid, format!("g ({n}).txt")), ());
    }
    let full = place(&t.st, uid, "g.txt", false, OnConflict::Rename).await;
    assert!(matches!(
        full,
        Err(strata_server::error::ApiError::Conflict(_))
    ));

    // A name with no room for a number.
    let long = format!("{}.txt", "n".repeat(250));
    t.upload(&long, b"x").await;
    let (s, v) = t
        .json(
            "POST",
            "/api/files/copy",
            json!({"from": long, "to": long, "on_conflict": "rename"}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("too long"), "{v}");
}

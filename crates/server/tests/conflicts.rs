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
    // (A save over the file: the same file, nothing in the trash.)
    assert!(trash_names(&t).await.is_empty());
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
    // No body or `null` is the default too; a body that isn't JSON is refused.
    for (body, want) in [
        (&b" null "[..], StatusCode::CONFLICT),
        (&b"\n"[..], StatusCode::CONFLICT),
        (&b"\xff\xfe"[..], StatusCode::BAD_REQUEST),
        (
            &b"{\"on_conflict\": \"sideways\"}"[..],
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (s, _, _) = t
            .raw("POST", &commit, axum::body::Body::from(body.to_vec()), &[])
            .await;
        assert_eq!(s, want, "{body:?}");
    }
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
    t.upload("Disk/src/f/new.txt", b"new").await;
    t.upload("Disk/dst/f/inner.txt", b"old").await;
    // A mount inside the folder being replaced: refused before anything is
    // written.
    let v = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "Other", "config": {"kind": "local", "path": "other"}}),
        )
        .await;
    std::fs::create_dir_all(t.dir.path().join("local/other")).unwrap();
    let m = t
        .ok(
            "POST",
            "/api/mounts",
            json!({"path": "Disk/dst/f/m", "connection_id": v["id"], "mode": "raw"}),
        )
        .await;
    let (s, v) = op(&t, "move", "Disk/src/f", "Disk/dst/f", "replace").await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("mounted inside"),
        "{v}"
    );
    assert_eq!(t.names("Disk/dst").await, vec!["f"]);
    t.ok("DELETE", &format!("/api/mounts/{}", m["id"]), json!({}))
        .await;

    // The old one can't go to the trash (where the trash would be is a file).
    let trash = disk.join(strata_storage::backend::TRASH_DIR);
    let _ = std::fs::remove_dir_all(&trash);
    std::fs::write(&trash, b"in the way").unwrap();
    let (s, v) = op(&t, "move", "Disk/src/f", "Disk/dst/f", "replace").await;
    assert!(!s.is_success(), "{v}");
    // The source is back where it was, the old folder untouched, nothing staged.
    assert_eq!(std::fs::read(disk.join("src/f/new.txt")).unwrap(), b"new");
    assert_eq!(std::fs::read(disk.join("dst/f/inner.txt")).unwrap(), b"old");
    assert_eq!(t.names("Disk/dst").await, vec!["f"]);
    // A copy cleans up its staged copy.
    let (s, v) = op(&t, "copy", "Disk/src/f", "Disk/dst/f", "replace").await;
    assert!(!s.is_success(), "{v}");
    assert_eq!(t.names("Disk/dst").await, vec!["f"]);
    assert_eq!(t.names("Disk/src").await, vec!["f"]);
    assert_eq!(std::fs::read(disk.join("dst/f/inner.txt")).unwrap(), b"old");
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
    let Some(t) = app_with(|c| c.busy_wait_ms = 300).await else {
        return;
    };
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    // A name another operation is writing: Fail says so, Keep both skips it.
    let held = place(&t.st, uid, "f.txt", false, OnConflict::Fail)
        .await
        .unwrap();
    assert_eq!(held.path, "f.txt");
    // (Fail waits for the other one first, up to a limit.)
    let busy = place(&t.st, uid, "f.txt", false, OnConflict::Fail).await;
    assert!(matches!(
        busy,
        Err(strata_server::error::ApiError::Conflict(_))
    ));
    let next = place(&t.st, uid, "f.txt", false, OnConflict::Rename)
        .await
        .unwrap();
    assert_eq!(next.path, "f (2).txt");
    // Released when the operation ends, which one waiting sees.
    let (after, ()) = tokio::join!(place(&t.st, uid, "f.txt", false, OnConflict::Fail), async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drop(held);
    });
    assert_eq!(after.unwrap().path, "f.txt");
    drop(next);

    // Every number taken: refused, not looped forever.
    let mut all = Vec::new();
    for n in 1..1000 {
        let name = if n == 1 {
            "g.txt".to_string()
        } else {
            format!("g ({n}).txt")
        };
        all.push(
            place(&t.st, uid, &name, false, OnConflict::Fail)
                .await
                .unwrap(),
        );
    }
    let full = place(&t.st, uid, "g.txt", false, OnConflict::Rename).await;
    assert!(matches!(
        full,
        Err(strata_server::error::ApiError::Conflict(_))
    ));
    drop(all);

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

#[tokio::test]
async fn replace_keeps_share_links_and_kinds() {
    let Some(t) = app().await else { return };
    t.upload("a/f.txt", b"new").await;
    t.upload("b/f.txt", b"old").await;
    let s = t
        .ok("POST", "/api/shares", json!({"path": "b/f.txt"}))
        .await;
    let link = s["token"].as_str().unwrap().to_string();
    let db = t.st.db.clone();
    let share_node = || {
        sqlx::query_scalar::<_, Option<i64>>("SELECT node_id FROM shares WHERE token = $1")
            .bind(link.clone())
            .fetch_one(&db)
    };
    let node_at = |name: &'static str| {
        sqlx::query_scalar::<_, i64>("SELECT id FROM nodes WHERE parent = 'b' AND name = $1")
            .bind(name)
            .fetch_one(&db)
    };
    let old = node_at("f.txt").await.unwrap();
    assert_eq!(share_node().await.unwrap(), Some(old));
    // An upload saves over the file: the same file, its link still opens it.
    t.upload_replacing("b/f.txt", b"newer").await;
    assert_eq!(node_at("f.txt").await.unwrap(), old);
    assert_eq!(share_node().await.unwrap(), Some(old));
    assert!(trash_names(&t).await.is_empty());
    // An item moved in its place is another item: the link goes to the
    // trash with the old one, never opening what took its name.
    let (s, v) = op(&t, "move", "a/f.txt", "b/f.txt", "replace").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(t.download("b/f.txt").await, b"new");
    assert_ne!(node_at("f.txt").await.unwrap(), old);
    assert_eq!(share_node().await.unwrap(), None);
    assert_eq!(trash_names(&t).await, vec!["b/f.txt"]);

    // Keep both counts on from a number already there.
    t.upload("n/a (2).txt", b"2").await;
    let (s, v) = op(&t, "copy", "n/a (2).txt", "n/a (2).txt", "rename").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["path"], "n/a (3).txt");

    // A file never replaces a folder, nor a folder a file.
    t.upload("c/x/inner.txt", b"i").await;
    t.upload("c/y", b"y").await;
    let (s, _) = op(&t, "copy", "c/y", "c/x", "replace").await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = op(&t, "copy", "c/x", "c/y", "replace").await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(t.names("c/x").await, vec!["inner.txt"]);
    assert_eq!(t.download("c/y").await, b"y");

    // An item onto itself: only a copy keeping both.
    let (s, _) = op(&t, "copy", "c/y", "c/y", "fail").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = op(&t, "move", "gone", "gone", "rename").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn never_replaces_the_folder_a_copy_comes_from_through_another_mount() {
    let Some(t) = app().await else { return };
    let disk = local_mount(&t, "raw").await;
    let conn: i64 = sqlx::query_scalar("SELECT id FROM connections WHERE name = 'Disk'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Again", "connection_id": conn, "mode": "raw"}),
    )
    .await;
    t.upload("Disk/x/y.txt", b"y").await;
    let (s, v) = op(&t, "copy", "Disk/x/y.txt", "Again/x", "replace").await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    // The same item: said so.
    let (s, v) = op(&t, "move", "Disk/x/y.txt", "Again/x/y.txt", "replace").await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("already there"),
        "{v}"
    );
    assert_eq!(std::fs::read(disk.join("x/y.txt")).unwrap(), b"y");
    assert!(trash_names(&t).await.is_empty());
}

#[tokio::test]
async fn a_commit_sent_again_waits_for_the_first() {
    let Some(t) = app().await else { return };
    let h = blake3::hash(b"mine").to_hex().to_string();
    let v = t
        .ok(
            "POST",
            "/api/uploads",
            json!({"path": "w.txt", "size": 4, "chunks": [{"hash": h, "size": 4}]}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_string();
    let (s, _, _) = t
        .raw(
            "PUT",
            &format!("/api/uploads/{id}/chunks/{h}"),
            axum::body::Body::from("mine"),
            &[],
        )
        .await;
    assert!(s.is_success());
    // (As if the first commit were still writing.)
    let first = strata_server::conflicts::claim_key(&t.st, format!("upload:{id}")).unwrap();
    let commit = format!("/api/uploads/{id}/commit");
    let ((s, v), ()) = tokio::join!(t.json("POST", &commit, json!({})), async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        drop(first);
    });
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(t.download("w.txt").await, b"mine");
    // Once done, a commit sent again gets the same answer, writing nothing.
    let (s, again) = t
        .json("POST", &commit, json!({"on_conflict": "rename"}))
        .await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(again["path"], "w.txt");
    assert_eq!(t.names("").await, vec!["w.txt"]);
}

#[tokio::test]
async fn a_commit_waits_only_so_long_and_keeps_the_choice_from_create() {
    let Some(t) = app_with(|c| c.busy_wait_ms = 100).await else {
        return;
    };
    t.upload("k.txt", b"there").await;
    let h = blake3::hash(b"mine").to_hex().to_string();
    let v = t
        .ok(
            "POST",
            "/api/uploads",
            json!({"path": "k.txt", "size": 4, "chunks": [{"hash": h, "size": 4}], "on_conflict": "rename"}),
        )
        .await;
    let id = v["id"].as_str().unwrap().to_string();
    let (s, _, _) = t
        .raw(
            "PUT",
            &format!("/api/uploads/{id}/chunks/{h}"),
            axum::body::Body::from("mine"),
            &[],
        )
        .await;
    assert!(s.is_success());
    let commit = format!("/api/uploads/{id}/commit");
    let first = strata_server::conflicts::claim_key(&t.st, format!("upload:{id}")).unwrap();
    let (s, v) = t.json("POST", &commit, json!({})).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    drop(first);
    // No answer at commit: keep both, as asked at create.
    let v = t.ok("POST", &commit, json!({})).await;
    assert_eq!(v["path"], "k (2).txt");
    assert_eq!(t.download("k.txt").await, b"there");
}

#[tokio::test]
async fn replaces_a_crash_interrupted_are_settled() {
    use strata_server::conflicts::{OnConflict, place, reconcile};
    let Some(t) = app().await else { return };
    let uid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    let staged = |path: &str, target: &str, is_move: bool| {
        sqlx::query(
            "INSERT INTO staged_items (user_id, path, target, is_dir, is_move) VALUES ($1, $2, $3, FALSE, $4)",
        )
        .bind(uid)
        .bind(path.to_string())
        .bind(target.to_string())
        .bind(is_move)
        .execute(&t.st.db)
    };
    // Part of a move: shown beside the item it was replacing, or in its
    // place if that one went already.
    t.upload("d/f.txt", b"old").await;
    t.upload("d/.strata-moved", b"moved").await;
    staged("d/.strata-moved", "d/f.txt", true).await.unwrap();
    t.upload("d/.strata-alone", b"alone").await;
    staged("d/.strata-alone", "d/g.txt", true).await.unwrap();
    // Part of a copy: goes.
    t.upload("d/.strata-copied", b"copied").await;
    staged("d/.strata-copied", "d/f.txt", false).await.unwrap();
    // Gone already: only the record goes.
    staged("d/.strata-gone", "d/f.txt", false).await.unwrap();
    // Still being written by an operation here: left alone.
    let busy = place(&t.st, uid, "d/.strata-busy", false, OnConflict::Fail)
        .await
        .unwrap();
    staged("d/.strata-busy", "d/f.txt", true).await.unwrap();
    // Somewhere that doesn't resolve: next time.
    local_mount(&t, "raw").await;
    staged("Disk/.strata-trash/x", "Disk/x", true)
        .await
        .unwrap();

    assert_eq!(reconcile(&t.st).await.unwrap(), 4);
    assert_eq!(t.names("d").await, vec!["f (2).txt", "f.txt", "g.txt"]);
    assert_eq!(t.download("d/f (2).txt").await, b"moved");
    assert_eq!(t.download("d/g.txt").await, b"alone");
    assert_eq!(t.download("d/f.txt").await, b"old");
    drop(busy);
    let left: Vec<String> = sqlx::query_scalar("SELECT path FROM staged_items ORDER BY id")
        .fetch_all(&t.st.db)
        .await
        .unwrap();
    assert_eq!(left, vec!["d/.strata-busy", "Disk/.strata-trash/x"]);
    // Nothing is left after a replace that ran to its end.
    t.upload("e/a.txt", b"1").await;
    t.upload("e/b.txt", b"2").await;
    let (s, v) = op(&t, "copy", "e/a.txt", "e/b.txt", "replace").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM staged_items WHERE target = 'e/b.txt'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

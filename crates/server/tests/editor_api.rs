//! Conflict-safe saves (task 010): `PUT /api/files/content` with If-Match.
mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::*;
use serde_json::json;

async fn get_etag(t: &TestApp, path: &str) -> String {
    let (s, _, h) = t
        .raw(
            "GET",
            &format!("/api/files/content?path={}", enc(path)),
            Body::empty(),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::OK, "GET {path}");
    h["etag"].to_str().unwrap().to_string()
}

async fn put(
    t: &TestApp,
    path: &str,
    body: &str,
    hdrs: &[(&str, &str)],
) -> (StatusCode, serde_json::Value, axum::http::HeaderMap) {
    let (s, b, h) = t
        .raw(
            "PUT",
            &format!("/api/files/content?path={}", enc(path)),
            Body::from(body.to_string()),
            hdrs,
        )
        .await;
    (s, serde_json::from_slice(&b).unwrap_or_default(), h)
}

async fn exercise(t: &TestApp, dir: &str) {
    let f = format!("{dir}/notes.md");
    // create needs If-None-Match: *
    assert_eq!(
        put(t, &f, "# one\n", &[]).await.0,
        StatusCode::PRECONDITION_REQUIRED
    );
    let (s, v, h) = put(t, &f, "# one\n", &[("if-none-match", "*")]).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let e1 = h["etag"].to_str().unwrap().to_string();
    assert_eq!(v["etag"], e1);
    // GET and PUT agree on the ETag
    assert_eq!(get_etag(t, &f).await, e1);
    // creating again over an existing file fails
    assert_eq!(
        put(t, &f, "x", &[("if-none-match", "*")]).await.0,
        StatusCode::PRECONDITION_FAILED
    );
    // blind overwrite refused
    assert_eq!(
        put(t, &f, "x", &[]).await.0,
        StatusCode::PRECONDITION_REQUIRED
    );
    // save with the right ETag
    let (s, _, h) = put(t, &f, "# two\n", &[("if-match", &e1)]).await;
    assert_eq!(s, StatusCode::OK);
    let e2 = h["etag"].to_str().unwrap().to_string();
    assert_ne!(e1, e2);
    assert_eq!(get_etag(t, &f).await, e2);
    assert_eq!(t.download(&f).await, b"# two\n");
    // a stale ETag (the second tab) gets 412 with the current ETag; nothing changes
    let (s, v, h) = put(t, &f, "# stale\n", &[("if-match", &e1)]).await;
    assert_eq!(s, StatusCode::PRECONDITION_FAILED);
    assert_eq!(v["etag"], e2);
    assert_eq!(h["etag"].to_str().unwrap(), e2);
    assert_eq!(t.download(&f).await, b"# two\n");
    // weak or list ETags are refused
    assert_eq!(
        put(t, &f, "x", &[("if-match", &format!("W/{e2}"))]).await.0,
        StatusCode::BAD_REQUEST
    );
    // a deleted file can't be saved over with its old ETag
    t.ok("POST", "/api/files/delete", json!({"paths": [f.clone()]}))
        .await;
    assert_eq!(
        put(t, &f, "x", &[("if-match", &e2)]).await.0,
        StatusCode::PRECONDITION_FAILED
    );
}

#[tokio::test]
async fn conditional_saves_on_hashed_mounts() {
    let Some(t) = app().await else { return };
    exercise(&t, "docs").await;
}

#[tokio::test]
async fn conditional_saves_on_raw_mounts() {
    let Some(t) = app().await else { return };
    let cid = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "d", "config": {"kind": "local", "path": "d"}}),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Raw", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    exercise(&t, "Raw").await;
}

#[tokio::test]
async fn outside_change_on_raw_disk_is_detected() {
    let Some(t) = app().await else { return };
    let cid = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "d", "config": {"kind": "local", "path": "d"}}),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Raw", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let disk = t.dir.path().join("local/d/a.txt");
    put(&t, "Raw/a.txt", "hello", &[("if-none-match", "*")]).await;
    let e = get_etag(&t, "Raw/a.txt").await;
    // same size, different content, written behind Strata's back
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&disk, b"HELLO").unwrap();
    let (s, _, _) = put(&t, "Raw/a.txt", "mine!", &[("if-match", &e)]).await;
    assert_eq!(s, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        std::fs::read(&disk).unwrap(),
        b"HELLO",
        "the outside change survives"
    );
}

#[tokio::test]
async fn only_one_of_two_racing_saves_wins() {
    let Some(t) = app().await else { return };
    put(&t, "r.txt", "base", &[("if-none-match", "*")]).await;
    let e = get_etag(&t, "r.txt").await;
    let mut tasks = Vec::new();
    for i in 0..8 {
        let router = t.router.clone();
        let cookie = t.cookie.clone().unwrap();
        let e = e.clone();
        tasks.push(tokio::spawn(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::put("/api/files/content?path=r.txt")
                .header("cookie", cookie)
                .header("x-strata", "1")
                .header("if-match", e)
                .body(Body::from(format!("writer {i}")))
                .unwrap();
            router.oneshot(req).await.unwrap().status()
        }));
    }
    let mut ok = 0;
    for h in tasks {
        match h.await.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::PRECONDITION_FAILED => {}
            s => panic!("unexpected {s}"),
        }
    }
    assert_eq!(ok, 1, "exactly one save may win");
}

#[tokio::test]
async fn saves_are_journaled_and_size_limited() {
    let Some(t) = app().await else { return };
    let (_, v) = t.get("/api/changes").await;
    let cursor = v["cursor"].as_str().unwrap().to_string();
    put(&t, "j.txt", "a", &[("if-none-match", "*")]).await;
    let e = get_etag(&t, "j.txt").await;
    put(&t, "j.txt", "b", &[("if-match", &e)]).await;
    let (_, v) = t.get(&format!("/api/changes?cursor={cursor}")).await;
    let ops: Vec<&str> = v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, vec!["create", "modify"]);
    let big = "x".repeat(5 * 1024 * 1024 + 10);
    let (s, _, _) = put(&t, "big.txt", &big, &[("if-none-match", "*")]).await;
    assert!(
        s == StatusCode::PAYLOAD_TOO_LARGE || s == StatusCode::BAD_REQUEST,
        "{s}"
    );
    // mount points can't be written as files
    assert_eq!(
        put(&t, "", "x", &[("if-none-match", "*")]).await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn refused_saves_never_recreate_deleted_folders() {
    let Some(t) = app().await else { return };
    let cid = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "d", "config": {"kind": "local", "path": "d"}}),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Raw", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    for root in ["docs", "Raw"] {
        let f = format!("{root}/sub/a.md");
        put(&t, &f, "x", &[("if-none-match", "*")]).await;
        let e = get_etag(&t, &f).await;
        t.ok(
            "POST",
            "/api/files/delete",
            json!({"paths": [format!("{root}/sub")]}),
        )
        .await;
        assert_eq!(
            put(&t, &f, "y", &[("if-match", &e)]).await.0,
            StatusCode::PRECONDITION_FAILED
        );
        assert!(
            !t.names(root).await.contains(&"sub".to_string()),
            "{root}/sub came back"
        );
    }
    assert!(!t.dir.path().join("local/d/sub").exists());
}

#[tokio::test]
async fn the_returned_etag_is_the_written_version() {
    let Some(t) = app().await else { return };
    let (_, v, h) = put(&t, "w.txt", "one", &[("if-none-match", "*")]).await;
    let e = h["etag"].to_str().unwrap().to_string();
    assert_eq!(v["etag"], e);
    // saving again with exactly that ETag works (nothing else was written)
    assert_eq!(
        put(&t, "w.txt", "two", &[("if-match", &e)]).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn coarse_mtime_storage_still_detects_same_size_changes() {
    let Some(t) = app().await else { return };
    let cid = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "d", "config": {"kind": "local", "path": "d"}}),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Raw", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let disk = t.dir.path().join("local/d/c.txt");
    std::fs::create_dir_all(disk.parent().unwrap()).unwrap();
    // a backend with whole-second mtimes (FAT, many FTP/WebDAV servers)
    let second = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
    let write = |content: &[u8]| {
        std::fs::write(&disk, content).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&disk)
            .unwrap()
            .set_modified(second)
            .unwrap();
    };
    write(b"aaaa");
    let e = get_etag(&t, "Raw/c.txt").await;
    write(b"bbbb"); // same size, same mtime, different content
    // the metadata can't tell the versions apart...
    assert_eq!(get_etag(&t, "Raw/c.txt").await, e);
    // ...but the editor sends the hash of the bytes it is based on
    let base_a = blake3::hash(b"aaaa").to_hex().to_string();
    let (s, _, _) = put(
        &t,
        "Raw/c.txt",
        "mine",
        &[("if-match", &e), ("x-strata-base-hash", &base_a)],
    )
    .await;
    assert_eq!(s, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        std::fs::read(&disk).unwrap(),
        b"bbbb",
        "the outside change survives"
    );
    // based on the current bytes, the save goes through
    let base_b = blake3::hash(b"bbbb").to_hex().to_string();
    let (s, _, h) = put(
        &t,
        "Raw/c.txt",
        "cccc",
        &[("if-match", &e), ("x-strata-base-hash", &base_b)],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(get_etag(&t, "Raw/c.txt").await, h["etag"].to_str().unwrap());
    assert_eq!(
        put(
            &t,
            "Raw/c.txt",
            "x",
            &[("if-match", &e), ("x-strata-base-hash", "nothex")]
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

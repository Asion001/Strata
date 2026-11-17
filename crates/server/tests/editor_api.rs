//! Conflict-safe saves (task 010): `PUT /api/files/content` with If-Match.
mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::*;
use serde_json::json;

/// What the editor knows: for each ETag it saw, the BLAKE3 of those bytes
/// (sent as X-Strata-Base-Hash with If-Match, like the real client).
fn bases() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static B: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    B.get_or_init(Default::default)
}

async fn get_etag(t: &TestApp, path: &str) -> String {
    let (s, body, h) = t
        .raw(
            "GET",
            &format!("/api/files/content?path={}", enc(path)),
            Body::empty(),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::OK, "GET {path}");
    let e = h["etag"].to_str().unwrap().to_string();
    bases()
        .lock()
        .unwrap()
        .insert(e.clone(), blake3::hash(&body).to_hex().to_string());
    e
}

async fn put(
    t: &TestApp,
    path: &str,
    body: &str,
    hdrs: &[(&str, &str)],
) -> (StatusCode, serde_json::Value, axum::http::HeaderMap) {
    // Like the editor: an If-Match comes with the hash of its base bytes,
    // unless the test says otherwise ("x-strata-base-hash": "none").
    let mut hdrs: Vec<(&str, String)> = hdrs.iter().map(|(k, v)| (*k, v.to_string())).collect();
    let has_base = hdrs.iter().any(|(k, _)| *k == "x-strata-base-hash");
    if !has_base
        && let Some((_, e)) = hdrs.iter().find(|(k, _)| *k == "if-match")
        && let Some(b) = bases().lock().unwrap().get(e).cloned()
    {
        hdrs.push(("x-strata-base-hash", b));
    }
    hdrs.retain(|(k, v)| !(*k == "x-strata-base-hash" && v == "none"));
    let hdrs: Vec<(&str, &str)> = hdrs.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (s, b, h) = t
        .raw(
            "PUT",
            &format!("/api/files/content?path={}", enc(path)),
            Body::from(body.to_string()),
            &hdrs,
        )
        .await;
    if s == StatusCode::OK
        && let Some(e) = h.get("etag")
    {
        bases().lock().unwrap().insert(
            e.to_str().unwrap().to_string(),
            blake3::hash(body.as_bytes()).to_hex().to_string(),
        );
    }
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
    // "any version" would skip the conflict check
    assert_eq!(
        put(t, &f, "x", &[("if-match", "*")]).await.0,
        StatusCode::BAD_REQUEST
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
    // lists are refused; a proxy-weakened tag still names the version
    assert_eq!(
        put(t, &f, "x", &[("if-match", &format!("{e2}, {e1}"))])
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let base2 = blake3::hash(b"# two\n").to_hex().to_string();
    let (s, _, h) = put(
        t,
        &f,
        "# three\n",
        &[
            ("if-match", &format!("W/{e2}")),
            ("x-strata-base-hash", &base2),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let e2 = h["etag"].to_str().unwrap().to_string();
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
    // raw If-Match without the base hash is refused: metadata alone can miss changes
    put(&t, "Raw/b.txt", "x", &[("if-none-match", "*")]).await;
    let e = get_etag(&t, "Raw/b.txt").await;
    let (s, _, _) = put(
        &t,
        "Raw/b.txt",
        "y",
        &[("if-match", &e), ("x-strata-base-hash", "none")],
    )
    .await;
    assert_eq!(s, StatusCode::PRECONDITION_REQUIRED);
    // hashed versions are content hashes already: no base needed
    put(&t, "h.txt", "x", &[("if-none-match", "*")]).await;
    let e = get_etag(&t, "h.txt").await;
    let (s, _, _) = put(
        &t,
        "h.txt",
        "y",
        &[("if-match", &e), ("x-strata-base-hash", "none")],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
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
    // ...so such files are never revalidated into a stale 304
    let (s, body, _) = t
        .raw(
            "GET",
            "/api/files/content?path=Raw/c.txt",
            Body::empty(),
            &[("if-none-match", &e)],
        )
        .await;
    assert_eq!((s, body.as_slice()), (StatusCode::OK, &b"bbbb"[..]));
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

#[test]
fn coarse_versions_are_recognised() {
    use strata_server::vfs::{Node, is_coarse};
    let node = |etag: Option<&str>, size: i64, chunks: bool| Node {
        name: "f".into(),
        is_dir: false,
        size,
        mtime: chrono::Utc::now(),
        etag: etag.map(str::to_string),
        chunks: chunks.then(Vec::new),
        chunk_sizes: None,
    };
    // ours: size hex, then mtime ns
    assert!(is_coarse(&node(Some("1a-0"), 26, false)));
    assert!(is_coarse(&node(
        Some(&format!("1a-{:x}", 1_700_000_000u128 * 1_000_000_000)),
        26,
        false
    )));
    assert!(!is_coarse(&node(
        Some(&format!("1a-{:x}", 1_700_000_000_123_456_789u128)),
        26,
        false
    )));
    // nginx-style mtime-size backend etags
    assert!(is_coarse(&node(Some("\"6553f100-1a\""), 26, false)));
    // content etags and hashed nodes are not coarse; no etag at all is
    assert!(!is_coarse(&node(
        Some("\"9e107d9d372bb6826bd81d3542a419d6\""),
        26,
        false
    )));
    assert!(!is_coarse(&node(Some("abc"), 26, true)));
    assert!(is_coarse(&node(None, 26, false)));
}

#[tokio::test]
async fn saves_through_overlapping_raw_mounts_are_serialised() {
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
        json!({"path": "All", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Docs", "connection_id": cid, "base_path": "docs", "mode": "raw"}),
    )
    .await;
    put(&t, "All/docs/n.md", "base", &[("if-none-match", "*")]).await;
    let e = get_etag(&t, "All/docs/n.md").await;
    let e2 = get_etag(&t, "Docs/n.md").await;
    assert_eq!(e, e2, "same file, same version through both mounts");
    let base = blake3::hash(b"base").to_hex().to_string();
    let mut tasks = Vec::new();
    for (i, path) in ["All/docs/n.md", "Docs/n.md", "All/docs/n.md", "Docs/n.md"]
        .into_iter()
        .enumerate()
    {
        let router = t.router.clone();
        let cookie = t.cookie.clone().unwrap();
        let (e, base) = (e.clone(), base.clone());
        tasks.push(tokio::spawn(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::put(format!("/api/files/content?path={}", enc(path)))
                .header("cookie", cookie)
                .header("x-strata", "1")
                .header("if-match", e)
                .header("x-strata-base-hash", base)
                .body(Body::from(format!("writer {i}")))
                .unwrap();
            router.oneshot(req).await.unwrap().status()
        }));
    }
    let mut ok = 0;
    for h in tasks {
        if h.await.unwrap() == StatusCode::OK {
            ok += 1;
        }
    }
    assert_eq!(ok, 1, "exactly one save may win across overlapping mounts");
}

#[tokio::test]
async fn editing_a_connection_cannot_put_a_raw_view_over_a_hashed_store() {
    let Some(t) = app().await else { return };
    let disk = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "disk", "config": {"kind": "local", "path": "disk"}}),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    let vault = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "vault", "config": {"kind": "local", "path": "vault"}}),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "R", "connection_id": disk, "mode": "raw"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "V", "connection_id": vault, "mode": "hashed"}),
    )
    .await;
    let (s, _) = t
        .json(
            "PUT",
            &format!("/api/connections/{disk}"),
            json!({"name": "disk", "config": {"kind": "local", "path": "vault"}}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // still pointing where it was
    let (_, v) = t.get(&format!("/api/connections/{disk}")).await;
    assert_eq!(v["config"]["path"], "disk");
}

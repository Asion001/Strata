mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::*;
use serde_json::json;

#[tokio::test]
async fn auth_required_and_csrf() {
    let Some(mut t) = app().await else { return };
    t.cookie = None;
    assert_eq!(t.get("/api/files/list").await.0, StatusCode::UNAUTHORIZED);
    t.login("admin", "password123").await;
    // cookie-authenticated mutation without X-Strata is refused
    let req = axum::http::Request::post("/api/files/mkdir")
        .header("cookie", t.cookie.clone().unwrap())
        .header("content-type", "application/json")
        .body(Body::from(r#"{"path":"x"}"#))
        .unwrap();
    use tower::ServiceExt;
    let r = t.router.clone().oneshot(req).await.unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let (s, _) = t
        .json(
            "POST",
            "/api/auth/login",
            json!({"username":"admin","password":"nope"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn hashed_root_file_lifecycle() {
    let Some(t) = app().await else { return };
    assert!(t.names("").await.is_empty());
    t.ok("POST", "/api/files/mkdir", json!({"path": "Docs/Sub"}))
        .await;
    assert_eq!(t.names("").await, vec!["Docs"]);
    t.upload("Docs/hello.txt", b"hello world").await;
    assert_eq!(t.names("Docs").await, vec!["Sub", "hello.txt"]);
    assert_eq!(t.download("Docs/hello.txt").await, b"hello world");

    // byte range
    let (s, b, h) = t
        .raw(
            "GET",
            "/api/files/content?path=Docs/hello.txt",
            Body::empty(),
            &[("range", "bytes=6-")],
        )
        .await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT);
    assert_eq!(b, b"world");
    assert_eq!(h["content-range"], "bytes 6-10/11");
    let (s, _, _) = t
        .raw(
            "GET",
            "/api/files/content?path=Docs/hello.txt",
            Body::empty(),
            &[("range", "bytes=50-60")],
        )
        .await;
    assert_eq!(s, StatusCode::RANGE_NOT_SATISFIABLE);

    // File contents are never compressed on the fly, even when the client
    // accepts gzip: range readers (the PDF viewer) need the real length.
    let big = vec![b'a'; 64 * 1024];
    t.upload("Docs/big.txt", &big).await;
    let (s, b, h) = t
        .raw(
            "GET",
            "/api/files/content?path=Docs/big.txt",
            Body::empty(),
            &[("accept-encoding", "gzip")],
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(h.get("content-encoding").is_none());
    assert_eq!(h["accept-ranges"], "bytes");
    assert_eq!(h["content-length"], "65536");
    assert_eq!(b.len(), big.len());

    // rename, move folder, overwrite
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "Docs/hello.txt", "to": "Docs/Sub/hi.txt"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "Docs", "to": "Archive/Docs"}),
    )
    .await;
    assert_eq!(t.download("Archive/Docs/Sub/hi.txt").await, b"hello world");
    t.upload_replacing("Archive/Docs/Sub/hi.txt", b"v2").await;
    assert_eq!(t.download("Archive/Docs/Sub/hi.txt").await, b"v2");
    let (s, _) = t
        .json(
            "POST",
            "/api/files/move",
            json!({"from": "Archive", "to": "Archive/x"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // copy shares blobs
    t.ok(
        "POST",
        "/api/files/copy",
        json!({"from": "Archive/Docs/Sub/hi.txt", "to": "copy.txt"}),
    )
    .await;
    assert_eq!(t.download("copy.txt").await, b"v2");

    t.ok("POST", "/api/files/delete", json!({"paths": ["Archive"]}))
        .await;
    assert_eq!(t.names("").await, vec!["copy.txt"]);
    assert_eq!(
        t.get("/api/files/list?path=Archive").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        t.get("/api/files/list?path=copy.txt").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        t.get("/api/files/list?path=..").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn chunked_upload_dedupes_and_resumes() {
    let Some(t) = app().await else { return };
    let data = pseudo(20 * 1024 * 1024 + 17, 1);
    let missing = t.upload("big.bin", &data).await;
    assert!(
        missing.len() > 2,
        "expected several chunks, got {missing:?}"
    );
    assert_eq!(t.download("big.bin").await, data);
    // same content again: nothing to send
    assert!(t.upload("big-copy.bin", &data).await.is_empty());
    // range across chunk boundaries
    let (s, b, _) = t
        .raw(
            "GET",
            "/api/files/content?path=big.bin",
            Body::empty(),
            &[("range", "bytes=1000000-9000000")],
        )
        .await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT);
    assert_eq!(b, &data[1_000_000..=9_000_000]);

    // a commit with chunks missing is refused; status reports them
    let v = t.ok("POST", "/api/uploads", json!({"path": "p.bin", "size": 3, "chunks": [{"hash": blake3::hash(b"abc").to_hex().to_string(), "size": 3}]})).await;
    let id = v["id"].as_str().unwrap();
    assert_eq!(
        t.json("POST", &format!("/api/uploads/{id}/commit"), json!({}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        t.get(&format!("/api/uploads/{id}")).await.1["missing"],
        json!([0])
    );
    // wrong bytes for a hash are refused
    let h = blake3::hash(b"abc").to_hex().to_string();
    let (s, _, _) = t
        .raw(
            "PUT",
            &format!("/api/uploads/{id}/chunks/{h}"),
            Body::from("abd"),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

async fn local_conn(t: &TestApp, name: &str, path: &str) -> i64 {
    let v = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": name, "config": {"kind": "local", "path": path}}),
        )
        .await;
    assert_eq!(v["health"], "ok", "{v}");
    v["id"].as_i64().unwrap()
}

#[tokio::test]
async fn raw_mount_scans_and_moves_across_mounts() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(disk.join("photos")).unwrap();
    std::fs::write(disk.join("photos/a.txt"), b"outside strata").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    assert_eq!(t.names("").await, vec!["Disk"]);
    assert_eq!(t.names("Disk").await, vec!["photos"]);
    assert_eq!(t.names("Disk/photos").await, vec!["a.txt"]);
    assert_eq!(t.download("Disk/photos/a.txt").await, b"outside strata");

    // changes made outside are picked up by scanning (TTL 0 in tests)
    std::fs::write(disk.join("photos/b.txt"), b"new").unwrap();
    std::fs::remove_file(disk.join("photos/a.txt")).unwrap();
    assert_eq!(t.names("Disk/photos").await, vec!["b.txt"]);

    // uploads land on disk as plain files
    let data = pseudo(9 * 1024 * 1024, 3);
    t.upload("Disk/photos/up.bin", &data).await;
    assert_eq!(std::fs::read(disk.join("photos/up.bin")).unwrap(), data);

    // rename inside raw = real rename
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "Disk/photos", "to": "Disk/pics"}),
    )
    .await;
    assert!(disk.join("pics/up.bin").exists());
    // move raw -> hashed root: copy, verify, delete
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "Disk/pics", "to": "Pics"}),
    )
    .await;
    assert!(!disk.join("pics").exists());
    assert_eq!(t.download("Pics/up.bin").await, data);
    assert_eq!(t.download("Pics/b.txt").await, b"new");
    // and back hashed -> raw
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "Pics/b.txt", "to": "Disk/b.txt"}),
    )
    .await;
    assert_eq!(std::fs::read(disk.join("b.txt")).unwrap(), b"new");

    // search finds both raw and hashed entries
    let (_, v) = t.get("/api/search?q=up.b").await;
    assert_eq!(v["hits"][0]["path"], "Pics/up.bin");
    let (_, v) = t.get("/api/search?q=b.tx").await;
    assert_eq!(v["hits"][0]["path"], "Disk/b.txt");
}

#[tokio::test]
async fn nested_mounts_and_shadowing() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(disk.join("vault")).unwrap();
    std::fs::write(disk.join("vault/real.txt"), b"shadowed").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    // hashed store inside the raw mount, shadowing the real 'vault' folder
    let (s, _) = t.json("POST", "/api/mounts", json!({"path": "Disk/vault", "connection_id": cid, "base_path": ".strata-vault", "mode": "hashed"})).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "hashed store on the same storage as a raw view is refused"
    );
    let vault = local_conn(&t, "Vault", "vaultstore").await;
    let m = t
        .ok(
            "POST",
            "/api/mounts",
            json!({"path": "Disk/vault", "connection_id": vault, "mode": "hashed"}),
        )
        .await;
    assert_eq!(t.names("Disk/vault").await, Vec::<String>::new());
    t.upload("Disk/vault/secret.txt", b"in the hashed store")
        .await;
    assert_eq!(t.names("Disk/vault").await, vec!["secret.txt"]);
    assert!(!disk.join("vault/secret.txt").exists());
    // a raw mount three levels deep inside the hashed one
    let cid2 = local_conn(&t, "Other", "other").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Disk/vault/a/b/raw", "connection_id": cid2, "mode": "raw"}),
    )
    .await;
    t.upload("Disk/vault/a/b/raw/x.txt", b"x").await;
    assert_eq!(
        std::fs::read(t.dir.path().join("local/other/x.txt")).unwrap(),
        b"x"
    );
    // deleting a folder with mounts inside is refused; mount points cannot be deleted as files
    assert_eq!(
        t.json(
            "POST",
            "/api/files/delete",
            json!({"paths": ["Disk/vault/a"]})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        t.json(
            "POST",
            "/api/files/delete",
            json!({"paths": ["Disk/vault"]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    // moving a mount point re-mounts it
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "Disk/vault/a/b/raw", "to": "Elsewhere"}),
    )
    .await;
    assert_eq!(t.download("Elsewhere/x.txt").await, b"x");
    // unmount reveals the shadowed folder again
    let (s, _) = t
        .json("DELETE", &format!("/api/mounts/{}", m["id"]), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(t.names("Disk/vault").await, vec!["real.txt"]);
}

#[tokio::test]
async fn shares_journal_and_gc() {
    let Some(t) = app().await else { return };
    let (_, v) = t.get("/api/changes").await;
    let start = v["cursor"].as_str().unwrap().to_string();
    t.upload("pub/photo.txt", b"shared").await;
    let (_, v) = t.get(&format!("/api/changes?cursor={start}")).await;
    let ops: Vec<&str> = v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, vec!["create", "create"]);
    assert_ne!(v["cursor"].as_str().unwrap(), start);

    let s = t
        .ok(
            "POST",
            "/api/shares",
            json!({"path": "pub", "password": "letmein1"}),
        )
        .await;
    let token = s["token"].as_str().unwrap().to_string();
    let mut anon = TestApp {
        cookie: None,
        ..app_clone(&t)
    };
    anon.cookie = None;
    let (_, info) = anon.get(&format!("/api/public/{token}")).await;
    assert_eq!(info["needs_password"], true);
    assert_eq!(
        anon.get(&format!("/api/public/{token}/content?path=photo.txt"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        anon.json(
            "POST",
            &format!("/api/public/{token}/unlock"),
            json!({"password": "x"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let k = anon
        .ok(
            "POST",
            &format!("/api/public/{token}/unlock"),
            json!({"password": "letmein1"}),
        )
        .await["key"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, info) = anon.get(&format!("/api/public/{token}?k={k}")).await;
    assert_eq!(info["entries"][0]["name"], "photo.txt");
    let (s, b, _) = anon
        .raw(
            "GET",
            &format!("/api/public/{token}/content?path=photo.txt&k={k}"),
            Body::empty(),
            &[],
        )
        .await;
    assert_eq!((s, b.as_slice()), (StatusCode::OK, &b"shared"[..]));
    // cannot escape the share
    assert_eq!(
        anon.get(&format!("/api/public/{token}/content?path=../x&k={k}"))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );

    // expired share is gone
    let s2 = t
        .ok(
            "POST",
            "/api/shares",
            json!({"path": "pub/photo.txt", "expires_at": "2000-01-01T00:00:00Z"}),
        )
        .await;
    assert_eq!(
        anon.get(&format!("/api/public/{}", s2["token"].as_str().unwrap()))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // GC: delete the file for good (the trash would keep it); with grace 0
    // its blob goes away
    t.ok(
        "POST",
        "/api/files/delete",
        json!({"paths": ["pub"], "permanent": true}),
    )
    .await;
    let n = strata_server::background::gc(&t.st, 0).await.unwrap();
    assert_eq!(n, 1);
    let blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    assert_eq!(blobs, 0);
}

fn app_clone(t: &TestApp) -> TestApp {
    TestApp {
        router: t.router.clone(),
        st: t.st.clone(),
        cookie: t.cookie.clone(),
        dir: tempfile::tempdir().unwrap(),
    }
}

#[tokio::test]
async fn users_are_isolated() {
    let Some(mut t) = app().await else { return };
    t.ok(
        "POST",
        "/api/admin/users",
        json!({"username": "bob", "password": "bobpassword"}),
    )
    .await;
    t.upload("admin-only.txt", b"x").await;
    t.login("bob", "bobpassword").await;
    assert!(t.names("").await.is_empty());
    // non-admins cannot create local connections
    let (s, _) = t
        .json(
            "POST",
            "/api/connections",
            json!({"name": "x", "config": {"kind": "local", "path": ""}}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(t.get("/api/admin/users").await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn s3_hashed_mount_via_minio() {
    let Some(t) = app().await else { return };
    let Ok(endpoint) = std::env::var("STRATA_TEST_S3_ENDPOINT") else {
        eprintln!("skipping: STRATA_TEST_S3_ENDPOINT not set");
        return;
    };
    let v = t
        .ok("POST", "/api/connections", json!({"name": "minio", "config": {
            "kind": "s3", "bucket": std::env::var("STRATA_TEST_S3_BUCKET").unwrap_or("strata-test".into()),
            "endpoint": endpoint, "access_key_id": "minioadmin", "secret_access_key": "minioadmin",
            "root": format!("/it{}", rand::random::<u32>())}}))
        .await;
    assert_eq!(v["health"], "ok", "{v}");
    assert_eq!(
        v["config"]["secret_access_key"], "",
        "secrets must be redacted"
    );
    let cid = v["id"].as_i64().unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "S3", "connection_id": cid, "base_path": "store", "mode": "hashed"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "S3raw", "connection_id": cid, "base_path": "raw", "mode": "raw"}),
    )
    .await;
    let data = pseudo(6 * 1024 * 1024, 9);
    t.upload("S3/f.bin", &data).await;
    assert_eq!(t.download("S3/f.bin").await, data);
    t.upload("S3raw/dir/g.bin", &data).await;
    assert_eq!(t.names("S3raw/dir").await, vec!["g.bin"]);
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "S3raw/dir", "to": "S3raw/dir2"}),
    )
    .await;
    assert_eq!(t.download("S3raw/dir2/g.bin").await, data);
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "S3raw/dir2/g.bin", "to": "S3/g.bin"}),
    )
    .await;
    assert_eq!(t.download("S3/g.bin").await, data);
}

#[tokio::test]
async fn chunk_negotiation_is_per_user() {
    let Some(mut t) = app().await else { return };
    // bob gets a root on the same server storage as admin, but a separate volume;
    // mount a shared hashed volume for both to prove nothing leaks across users.
    t.ok(
        "POST",
        "/api/admin/users",
        json!({"username": "bob", "password": "bobpassword"}),
    )
    .await;
    let cid = local_conn(&t, "Shared", "shared").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "S", "connection_id": cid, "mode": "hashed"}),
    )
    .await;
    let data = pseudo(5 * 1024 * 1024, 4);
    assert!(!t.upload("S/a.bin", &data).await.is_empty());
    assert!(
        t.upload("S/b.bin", &data).await.is_empty(),
        "same user dedupes"
    );
    // bob mounts the same volume (admin owns the connection; admins may mount any)
    let bob_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'bob'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    let vol: i64 = sqlx::query_scalar("SELECT volume_id FROM mounts WHERE path = 'S'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO mounts (owner_id, path, parent_path, name, volume_id) VALUES ($1, 'S', '', 'S', $2)")
        .bind(bob_id).bind(vol).execute(&t.st.db).await.unwrap();
    t.login("bob", "bobpassword").await;
    let missing = t.upload("S/c.bin", &data).await;
    assert!(
        !missing.is_empty(),
        "another user's chunks must not be reported as present"
    );
    assert_eq!(t.download("S/c.bin").await, data);
    let blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE volume_id = $1")
        .bind(vol)
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    let chunks = strata_storage::chunker::chunk_lengths(&data).len() as i64;
    assert_eq!(blobs, chunks, "server still stores each chunk once");
}

#[tokio::test]
async fn raw_uploads_send_every_chunk() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "D", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let data = pseudo(9 * 1024 * 1024, 5);
    let n = strata_storage::chunker::chunk_lengths(&data).len();
    assert_eq!(t.upload("D/a.bin", &data).await.len(), n);
    assert_eq!(t.upload("D/b.bin", &data).await.len(), n);
    assert_eq!(
        std::fs::read(t.dir.path().join("local/disk/b.bin")).unwrap(),
        data
    );
}

#[tokio::test]
async fn journal_is_per_volume_and_overlaps_fan_out() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    std::fs::create_dir_all(t.dir.path().join("local/disk/photos")).unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "All", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let m = t
        .ok(
            "POST",
            "/api/mounts",
            json!({"path": "Photos", "connection_id": cid, "base_path": "photos", "mode": "raw"}),
        )
        .await;
    assert_eq!(
        m["overlaps"],
        json!(["All"]),
        "UI is told about the overlap"
    );
    let (_, v) = t.get("/api/changes").await;
    let start = v["cursor"].as_str().unwrap().to_string();
    t.upload("Photos/x.txt", b"x").await;
    let (_, v) = t.get(&format!("/api/changes?cursor={start}")).await;
    let mut got: Vec<String> = v["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect();
    got.sort();
    assert_eq!(got, vec!["All/photos/x.txt", "Photos/x.txt"]);
    // recorded once per volume, not once per user
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM journal WHERE op = 'create' AND (path = 'x.txt' OR path = 'photos/x.txt')",
    )
    .fetch_one(&t.st.db)
    .await
    .unwrap();
    assert_eq!(rows, 2);
    // the overlapping mount's cache was invalidated, so it lists the new file
    assert_eq!(t.names("All/photos").await, vec!["x.txt"]);
}

// ---- regression tests for review findings ----

#[tokio::test]
async fn no_move_or_copy_into_itself_through_a_second_mount() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "A", "connection_id": cid, "base_path": "v", "mode": "hashed"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "B", "connection_id": cid, "base_path": "v", "mode": "hashed"}),
    )
    .await;
    t.upload("A/x/f.txt", b"f").await;
    assert_eq!(
        t.json(
            "POST",
            "/api/files/move",
            json!({"from": "A/x", "to": "B/x/y"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        t.json(
            "POST",
            "/api/files/copy",
            json!({"from": "A/x", "to": "B/x/y"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    // raw folders of one connection that overlap
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "R", "connection_id": cid, "base_path": "r", "mode": "raw"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "RS", "connection_id": cid, "base_path": "r/sub", "mode": "raw"}),
    )
    .await;
    t.upload("R/sub/z.txt", b"z").await;
    assert_eq!(
        t.json(
            "POST",
            "/api/files/copy",
            json!({"from": "R/sub", "to": "RS/inner"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(t.download("A/x/f.txt").await, b"f");
}

#[tokio::test]
async fn mkdir_under_a_file_changes_nothing() {
    let Some(t) = app().await else { return };
    t.upload("report.pdf", b"pdf").await;
    assert_eq!(
        t.json(
            "POST",
            "/api/files/mkdir",
            json!({"path": "report.pdf/sub"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        t.get("/api/files/list?path=report.pdf/sub").await.0,
        StatusCode::NOT_FOUND
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent = 'report.pdf'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn refused_user_delete_keeps_the_account_intact() {
    let Some(mut t) = app().await else { return };
    let v = t
        .ok(
            "POST",
            "/api/admin/users",
            json!({"username": "carol", "password": "carolpassword"}),
        )
        .await;
    let carol = v["id"].as_i64().unwrap();
    t.login("carol", "carolpassword").await;
    // carol owns an FTP connection (never reached) that holds a volume
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "ftp", "config": {"kind": "ftp", "endpoint": "ftp://127.0.0.1:1"}}),
        )
        .await;
    let m = t
        .ok(
            "POST",
            "/api/mounts",
            json!({"path": "F", "connection_id": c["id"], "mode": "hashed"}),
        )
        .await;
    t.ok("DELETE", &format!("/api/mounts/{}", m["id"]), json!({}))
        .await;
    t.login("admin", "password123").await;
    assert_eq!(
        t.json("DELETE", &format!("/api/admin/users/{carol}"), json!({}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    t.login("carol", "carolpassword").await;
    assert!(
        t.names("").await.is_empty(),
        "carol still has her root mount"
    );
}

#[tokio::test]
async fn deleting_a_connection_never_drops_a_hashed_tree() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let m = t
        .ok(
            "POST",
            "/api/mounts",
            json!({"path": "H", "connection_id": cid, "mode": "hashed"}),
        )
        .await;
    t.upload("H/keep.txt", b"keep").await;
    t.ok("DELETE", &format!("/api/mounts/{}", m["id"]), json!({}))
        .await;
    assert_eq!(
        t.json("DELETE", &format!("/api/connections/{cid}"), json!({}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    // remount: the tree is still there
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "H2", "connection_id": cid, "mode": "hashed"}),
    )
    .await;
    assert_eq!(t.download("H2/keep.txt").await, b"keep");
}

#[tokio::test]
async fn gc_keeps_blobs_of_pending_uploads_and_survives_broken_backends() {
    let Some(t) = app().await else { return };
    t.upload("a.txt", b"pending content").await;
    t.ok("POST", "/api/files/delete", json!({"paths": ["a.txt"]}))
        .await;
    sqlx::query("UPDATE blobs SET created_at = now() - interval '3 days'")
        .execute(&t.st.db)
        .await
        .unwrap();
    // a new upload of the same content: the user already owns the chunk, so it is not resent
    let h = blake3::hash(b"pending content").to_hex().to_string();
    let v = t
        .ok(
            "POST",
            "/api/uploads",
            json!({"path": "b.txt", "size": 15, "chunks": [{"hash": h, "size": 15}]}),
        )
        .await;
    assert_eq!(v["missing"], json!([]));
    // a hashed volume on an unreachable FTP server must not stop GC
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "ftp", "config": {"kind": "ftp", "endpoint": "ftp://127.0.0.1:1"}}),
        )
        .await;
    let vid: i64 = sqlx::query_scalar("INSERT INTO volumes (connection_id, base_path, mode) VALUES ($1, '', 'hashed') RETURNING id")
        .bind(c["id"].as_i64().unwrap()).fetch_one(&t.st.db).await.unwrap();
    sqlx::query("INSERT INTO blobs (volume_id, hash, size, created_at) VALUES ($1, '\\x00', 1, now() - interval '3 days')")
        .bind(vid).execute(&t.st.db).await.unwrap();
    strata_server::background::gc(&t.st, 24).await.unwrap();
    t.ok(
        "POST",
        &format!("/api/uploads/{}/commit", v["id"].as_str().unwrap()),
        json!({}),
    )
    .await;
    assert_eq!(t.download("b.txt").await, b"pending content");
    // the unreachable volume's blob row is kept for a retry
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM blobs WHERE volume_id = $1")
        .bind(vid)
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    assert_eq!(kept, 1);
}

#[tokio::test]
async fn store_blob_rewrites_a_blob_gc_removed_meanwhile() {
    let Some(t) = app().await else { return };
    t.upload("a.txt", b"again").await;
    t.ok(
        "POST",
        "/api/files/delete",
        json!({"paths": ["a.txt"], "permanent": true}),
    )
    .await;
    sqlx::query("UPDATE blobs SET created_at = now() - interval '3 days'")
        .execute(&t.st.db)
        .await
        .unwrap();
    assert_eq!(strata_server::background::gc(&t.st, 24).await.unwrap(), 1);
    // same content again (fresh owner rows were cascaded away): must be written anew
    t.upload("b.txt", b"again").await;
    assert_eq!(t.download("b.txt").await, b"again");
}

// ---- second review round ----

#[tokio::test]
async fn journal_is_gap_free_under_parallel_writers() {
    let Some(t) = app().await else { return };
    let (_, v) = t.get("/api/changes").await;
    let start = v["cursor"].as_str().unwrap().to_string();
    let mut tasks = Vec::new();
    for i in 0..50 {
        let st = t.st.clone();
        tasks.push(tokio::spawn(async move {
            strata_server::vfs::mkdir_p(&st, 1, &format!("p{i}"))
                .await
                .unwrap();
        }));
    }
    for h in tasks {
        h.await.unwrap();
    }
    let seqs: Vec<i64> = sqlx::query_scalar(
        "SELECT seq FROM journal WHERE volume_id = (SELECT volume_id FROM mounts WHERE owner_id = 1 AND path = '') ORDER BY seq",
    )
    .fetch_all(&t.st.db)
    .await
    .unwrap();
    assert_eq!(seqs, (1..=50).collect::<Vec<i64>>());
    // paging with a small limit still delivers everything exactly once
    let mut cursor = start;
    let mut seen = std::collections::HashSet::new();
    loop {
        let (_, v) = t
            .get(&format!("/api/changes?cursor={cursor}&limit=7"))
            .await;
        for c in v["changes"].as_array().unwrap() {
            assert!(seen.insert(c["path"].as_str().unwrap().to_string()));
        }
        cursor = v["cursor"].as_str().unwrap().to_string();
        if !v["more"].as_bool().unwrap() {
            break;
        }
    }
    assert_eq!(seen.len(), 50);
}

#[tokio::test]
async fn pruned_cursor_gets_410_resync_and_foreign_volumes_are_ignored() {
    let Some(t) = app().await else { return };
    let (_, v) = t.get("/api/changes").await;
    let old = v["cursor"].as_str().unwrap().to_string();
    t.upload("a.txt", b"a").await;
    sqlx::query("UPDATE journal SET at = now() - interval '40 days'")
        .execute(&t.st.db)
        .await
        .unwrap();
    strata_server::journal::prune(&t.st).await.unwrap();
    let (s, v) = t.get(&format!("/api/changes?cursor={old}")).await;
    assert_eq!(s, StatusCode::GONE);
    assert_eq!(v["resync"], true);
    // a cursor naming a volume the user cannot see is accepted but reveals nothing
    let (_, v) = t.get("/api/changes").await;
    let mut c = strata_server::journal::Cursor::decode(v["cursor"].as_str().unwrap()).unwrap();
    c.v.insert(9999, 0);
    let (s, v) = t.get(&format!("/api/changes?cursor={}", c.encode())).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["changes"], json!([]));
}

#[tokio::test]
async fn share_dies_when_its_path_points_elsewhere() {
    let Some(t) = app().await else { return };
    t.upload("public/a.txt", b"public").await;
    t.upload("private/a.txt", b"secret").await;
    let s = t.ok("POST", "/api/shares", json!({"path": "public"})).await;
    let token = s["token"].as_str().unwrap();
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "public", "to": "old"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/files/move",
        json!({"from": "private", "to": "public"}),
    )
    .await;
    let mut anon = app_clone(&t);
    anon.cookie = None;
    assert_eq!(
        anon.get(&format!("/api/public/{token}")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        anon.get(&format!("/api/public/{token}/content?path=a.txt"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn cross_volume_move_keeps_what_it_cannot_copy() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    let disk = t.dir.path().join("local/disk");
    std::fs::create_dir_all(disk.join("d")).unwrap();
    std::fs::write(disk.join("d/ok.txt"), b"ok").unwrap();
    std::fs::write(disk.join("d/bad\nname.txt"), b"invalid name").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "D", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    let (s, _) = t
        .json(
            "POST",
            "/api/files/move",
            json!({"from": "D/d", "to": "Moved"}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "partial move is reported");
    assert_eq!(t.download("Moved/ok.txt").await, b"ok");
    assert!(!disk.join("d/ok.txt").exists());
    assert_eq!(
        std::fs::read(disk.join("d/bad\nname.txt")).unwrap(),
        b"invalid name"
    );
}

#[tokio::test]
async fn store_blob_repairs_a_blob_gc_crashed_on() {
    let Some(t) = app().await else { return };
    t.upload("a.txt", b"crash").await;
    t.ok("POST", "/api/files/delete", json!({"paths": ["a.txt"]}))
        .await;
    // GC marked the row and deleted the object, then "crashed" before deleting the row
    sqlx::query("UPDATE blobs SET deleting = TRUE")
        .execute(&t.st.db)
        .await
        .unwrap();
    std::fs::remove_dir_all(t.dir.path().join("local/blobs/users/1/blobs")).unwrap();
    // the chunk is reported missing (row is `deleting`), re-sent and repaired
    assert_eq!(t.upload("b.txt", b"crash").await, vec![0]);
    assert_eq!(t.download("b.txt").await, b"crash");
    let deleting: bool = sqlx::query_scalar("SELECT deleting FROM blobs")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    assert!(!deleting);
}

#[tokio::test]
async fn refused_move_creates_nothing() {
    let Some(t) = app().await else { return };
    let cid = local_conn(&t, "Disk", "disk").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "X", "connection_id": cid, "base_path": "r", "mode": "raw"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Y", "connection_id": cid, "base_path": "r", "mode": "raw"}),
    )
    .await;
    t.upload("X/a/f.txt", b"f").await;
    let (s, _) = t
        .json(
            "POST",
            "/api/files/move",
            json!({"from": "X/a", "to": "Y/a/sub/a"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(!t.dir.path().join("local/disk/r/a/sub").exists());
}

#[tokio::test]
async fn healthz_and_bootstrap_layout() {
    let Some(t) = app().await else { return };
    let (s, v) = t.get("/healthz").await;
    assert_eq!((s, v["ok"].as_bool()), (StatusCode::OK, Some(true)));
    let (_, conns) = t.get("/api/connections").await;
    let paths: Vec<(String, bool)> = conns
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["config"]["path"].as_str().unwrap().to_string(),
                c["is_default"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(paths.contains(&("blobs".into(), true)));
    assert!(paths.contains(&("files".into(), false)));
    t.upload("x.txt", b"x").await;
    assert!(t.dir.path().join("local/blobs/users/1/blobs").exists());
}

#[tokio::test]
async fn reserved_folders_are_never_exposed() {
    let Some(t) = app().await else { return };
    // no connection or mount may point into an atomic-write temp folder
    let (s, _) = t
        .json(
            "POST",
            "/api/connections",
            json!({"name": "x", "config": {"kind": "local", "path": "files/.strata-tmp"}}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let f = local_conn(&t, "Files", "files").await;
    for mode in ["raw", "hashed"] {
        let (s, _) = t
            .json("POST", "/api/mounts", json!({"path": format!("T{mode}"), "connection_id": f, "base_path": "a/.strata-tmp", "mode": mode}))
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "F", "connection_id": f, "mode": "raw"}),
    )
    .await;
    t.upload("F/a.txt", b"a").await; // the atomic write creates files/.strata-tmp
    assert!(t.dir.path().join("local/files/.strata-tmp").exists());
    assert_eq!(t.names("F").await, vec!["a.txt"]);
    assert_eq!(
        t.get("/api/files/list?path=F/.strata-tmp").await.0,
        StatusCode::NOT_FOUND
    );
    // a folder named .strata is ordinary user data
    t.upload("F/.strata/mine.txt", b"mine").await;
    assert_eq!(t.download("F/.strata/mine.txt").await, b"mine");
    // non-admins are refused local connections outright (403), whatever the path
    t.ok(
        "POST",
        "/api/admin/users",
        json!({"username": "eve", "password": "evepassword"}),
    )
    .await;
    let mut eve = app_clone(&t);
    eve.login("eve", "evepassword").await;
    let (s, _) = eve
        .json(
            "POST",
            "/api/connections/test",
            json!({"config": {"kind": "local", "path": "files/.strata-tmp"}}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[test]
fn state_dir_inside_storage_base_is_refused() {
    let mut cfg = strata_server::Config {
        database_url: String::new(),
        bind: String::new(),
        data_dir: "/srv/data/state".into(),
        local_base: "/srv/data".into(),
        default_store: "blobs".into(),
        files_dir: "files".into(),
        web_dir: "/w".into(),
        secret_key: "k".into(),
        admin_user: None,
        admin_password: None,
        cookie_secure: true,
        session_days: 1,
        gc_grace_hours: 1,
        raw_ttl_secs: 1,
        probe_secs: 60,
        trash_days: 30,
    };
    assert!(cfg.clone().checked().is_err());
    cfg.data_dir = "/var/lib/strata".into();
    assert!(cfg.checked().is_ok());
}

/// A local folder Strata may read but not write (e.g. bind-mounted read-only)
/// can be added and browsed; every change is refused and nothing on disk moves.
#[tokio::test]
async fn read_only_local_folder_can_be_browsed_but_not_changed() {
    use std::os::unix::fs::PermissionsExt;
    let Some(t) = app().await else { return };
    let disk = t.dir.path().join("local/media");
    std::fs::create_dir_all(disk.join("films")).unwrap();
    std::fs::write(disk.join("films/a.txt"), b"keep me").unwrap();
    let lock = |mode| {
        for d in [disk.join("films"), disk.clone()] {
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    };
    lock(0o555);
    // root ignores permission bits, so this needs an ordinary user (as in CI)
    if std::fs::create_dir(disk.join("probe")).is_ok() {
        eprintln!("skipping: permission bits are not enforced (running as root?)");
        return;
    }

    let v = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "Media", "config": {"kind": "local", "path": "media"}}),
        )
        .await;
    assert_eq!(v["health"], "ok", "{v}");
    assert_eq!(v["capabilities"]["read_only"], true, "{v}");
    let cid = v["id"].as_i64().unwrap();
    assert!(!disk.join(".strata-tmp").exists());

    // hashed mode needs to write blobs: refused with a clear message
    let (s, v) = t
        .json(
            "POST",
            "/api/mounts",
            json!({"path": "Media", "connection_id": cid, "mode": "hashed"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("read-only"), "{v}");

    // a missing base folder cannot be created on read-only storage
    let (s, v) = t
        .json(
            "POST",
            "/api/mounts",
            json!({"path": "Typo", "connection_id": cid, "mode": "raw", "base_path": "flims"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");

    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Media", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    assert_eq!(t.names("Media").await, vec!["films"]);
    // a mount below an existing read-only folder only walks it
    let w = local_conn(&t, "Scratch", "scratch").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Media/films/extra", "connection_id": w, "mode": "raw"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/files/mkdir",
        json!({"path": "Media/films/extra/ok"}),
    )
    .await;
    assert!(t.dir.path().join("local/scratch/ok").is_dir());
    // a connection holding hashed storage cannot be pointed at it
    let vault = local_conn(&t, "Vault", "vault").await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Vault", "connection_id": vault, "mode": "hashed"}),
    )
    .await;
    t.upload("Vault/v.txt", b"blob").await;
    let (s, v) = t
        .json(
            "PUT",
            &format!("/api/connections/{vault}"),
            json!({"name": "Vault", "config": {"kind": "local", "path": "media"}}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(t.download("Vault/v.txt").await, b"blob");
    assert_eq!(t.names("Media/films").await, vec!["extra", "a.txt"]);
    assert_eq!(t.download("Media/films/a.txt").await, b"keep me");

    let refused = |(s, v): (StatusCode, serde_json::Value)| {
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], "this folder is read-only", "{v}");
    };
    refused(
        t.json("POST", "/api/files/mkdir", json!({"path": "Media/new"}))
            .await,
    );
    refused(
        t.json(
            "POST",
            "/api/files/delete",
            json!({"paths": ["Media/films/a.txt"]}),
        )
        .await,
    );
    refused(
        t.json(
            "POST",
            "/api/files/move",
            json!({"from": "Media/films/a.txt", "to": "Media/films/b.txt"}),
        )
        .await,
    );
    // moving out would copy and then fail to delete: refused before copying
    refused(
        t.json(
            "POST",
            "/api/files/move",
            json!({"from": "Media/films", "to": "Elsewhere"}),
        )
        .await,
    );
    assert!(!t.names("").await.contains(&"Elsewhere".to_string()));
    refused(
        t.json(
            "POST",
            "/api/files/copy",
            json!({"from": "Media/films/a.txt", "to": "Media/c.txt"}),
        )
        .await,
    );
    refused(
        t.json(
            "POST",
            "/api/uploads",
            json!({"path": "Media/up.txt", "size": 1, "chunks": [
                {"hash": blake3::hash(b"x").to_hex().to_string(), "size": 1}
            ]}),
        )
        .await,
    );

    // copying out is fine: the source is only read
    t.ok(
        "POST",
        "/api/files/copy",
        json!({"from": "Media/films/a.txt", "to": "Copy.txt"}),
    )
    .await;
    assert_eq!(t.download("Copy.txt").await, b"keep me");

    // probing picks up a folder that turned read-only after it was added
    let flip = t.dir.path().join("local/flip");
    std::fs::create_dir_all(&flip).unwrap();
    let f = local_conn(&t, "Flip", "flip").await;
    std::fs::remove_dir(flip.join(".strata-tmp")).unwrap();
    std::fs::set_permissions(&flip, std::fs::Permissions::from_mode(0o555)).unwrap();
    let v = t
        .ok("POST", &format!("/api/connections/{f}/probe"), json!({}))
        .await;
    assert_eq!(v["capabilities"]["read_only"], true, "{v}");
    let (s, _) = t
        .json(
            "POST",
            "/api/mounts",
            json!({"path": "Flip", "connection_id": f, "mode": "hashed"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // and back
    std::fs::set_permissions(&flip, std::fs::Permissions::from_mode(0o755)).unwrap();
    let v = t
        .ok("POST", &format!("/api/connections/{f}/probe"), json!({}))
        .await;
    assert_eq!(v["capabilities"]["read_only"], false, "{v}");
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "Flip", "connection_id": f, "mode": "hashed"}),
    )
    .await;

    lock(0o755);
    let mut on_disk: Vec<_> = walk(&disk);
    on_disk.sort();
    assert_eq!(on_disk, vec!["films", "films/a.txt"]);
    assert!(!disk.join("films/extra").exists());
    assert_eq!(std::fs::read(disk.join("films/a.txt")).unwrap(), b"keep me");
}

fn walk(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type().unwrap().is_dir() {
            out.extend(walk(&e.path()).into_iter().map(|n| format!("{name}/{n}")));
        }
        out.push(name);
    }
    out
}

/// Text Postgres can't store (a NUL character) is a bad request, not a
/// server error (found by the API contract test).
#[tokio::test]
async fn nul_characters_are_bad_requests() {
    let Some(t) = app().await else { return };
    let (s, v) = t
        .json(
            "POST",
            "/api/auth/login",
            json!({"username": "a\u{0}", "password": "x"}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = t
        .json(
            "POST",
            "/api/shares",
            json!({"path": "", "password": "p\u{0}w"}),
        )
        .await;
    assert!(s.is_client_error(), "{s} {v}");
}

/// A mount over a folder that holds other mounts is refused: they would be
/// cut off (found by the state-machine test).
#[tokio::test]
async fn no_mount_over_other_mounts() {
    let Some(t) = app().await else { return };
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "disk", "config": {"kind": "local", "path": "d"}}),
        )
        .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "a/a/a", "connection_id": c["id"], "base_path": "one", "mode": "raw"}),
    )
    .await;
    let (s, v) = t
        .json(
            "POST",
            "/api/mounts",
            json!({"path": "a", "connection_id": c["id"], "base_path": "two", "mode": "raw"}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    // Beside it is fine.
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "b", "connection_id": c["id"], "base_path": "three", "mode": "raw"}),
    )
    .await;
}

/// A path below a file is simply not there (on a raw local folder too, where
/// the filesystem says "not a directory"): 404, not a server error (found
/// by the state-machine test).
#[tokio::test]
async fn paths_below_a_file_are_not_found() {
    let Some(t) = app().await else { return };
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "disk", "config": {"kind": "local", "path": "d"}}),
        )
        .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "raw", "connection_id": c["id"], "base_path": "", "mode": "raw"}),
    )
    .await;
    t.upload("raw/f", b"").await;
    t.upload("home-f", b"x").await;
    t.upload("src", b"s").await;
    for p in ["raw/f/x", "home-f/x"] {
        let (s, v) = t
            .json(
                "POST",
                "/api/files/move",
                json!({"from": p, "to": "elsewhere"}),
            )
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "move {p}: {v}");
        let (s, v) = t.get(&format!("/api/files/stat?path={}", enc(p))).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "stat {p}: {v}");
        let (s, v) = t
            .json("POST", "/api/files/delete", json!({"paths": [p]}))
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "delete {p}: {v}");
        // Writing below a file is a conflict, not a missing path.
        let (s, v) = t
            .json(
                "POST",
                "/api/files/mkdir",
                json!({"path": format!("{p}/y")}),
            )
            .await;
        assert_eq!(s, StatusCode::CONFLICT, "mkdir below {p}: {v}");
        let (s, v) = t
            .json(
                "POST",
                "/api/files/move",
                json!({"from": "src", "to": format!("{p}/y")}),
            )
            .await;
        assert_eq!(s, StatusCode::CONFLICT, "move to below {p}: {v}");
    }
}

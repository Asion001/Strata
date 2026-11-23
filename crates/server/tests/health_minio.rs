//! Connection health against a real S3 server (MinIO in a container):
//! capability detection, probes that leave nothing behind, a stopped server
//! going offline (its mounts flagged, reads and writes refused, the rest of
//! the tree working) and coming back.
//!
//! Needs Docker and `STRATA_TEST_DATABASE_URL`; skipped without either.

mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::*;
use serde_json::{Value, json};
use strata_server::health;
use testcontainers_modules::minio::MinIO;
use testcontainers_modules::testcontainers::core::{ExecCommand, IntoContainerPort};
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

async fn mc(c: &ContainerAsync<MinIO>, args: &str) -> String {
    let mut r = c
        .exec(ExecCommand::new(["sh", "-c", &format!("mc {args}")]))
        .await
        .expect("mc");
    let out = r.stdout_to_vec().await.unwrap_or_default();
    String::from_utf8_lossy(&out).into_owned()
}

fn s3(port: u16, bucket: &str) -> Value {
    json!({
        "kind": "s3",
        "bucket": bucket,
        "region": "us-east-1",
        "endpoint": format!("http://127.0.0.1:{port}"),
        "access_key_id": "minioadmin",
        "secret_access_key": "minioadmin",
    })
}

async fn view(t: &TestApp, id: i64) -> Value {
    let (s, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

async fn content(t: &TestApp, path: &str) -> StatusCode {
    t.raw(
        "GET",
        &format!("/api/files/content?path={}", enc(path)),
        Body::empty(),
        &[],
    )
    .await
    .0
}

#[tokio::test]
async fn minio_offline_and_back() {
    let Some(mut t) = app().await else { return };
    // A fixed host port, so the server comes back at the same address.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    // The official MinIO images are no longer published on Docker Hub or
    // quay.io; Chainguard's build (with `mc`) is, pinned by digest.
    let minio = match MinIO::default()
        .with_name("cgr.dev/chainguard/minio")
        .with_tag("latest@sha256:304e0bbeb70a9aa0c40ff26d5a9c0a24296019c6d326ba8b75214fb73441d6e9")
        .with_mapped_port(port, 9000.tcp())
        .start()
        .await
    {
        Ok(c) => c,
        // In CI it must run: a skip there would hide the whole test.
        Err(e) if std::env::var_os("CI").is_some() => panic!("can't start MinIO: {e:?}"),
        Err(e) => {
            eprintln!("skipping: can't start MinIO (Docker running?): {e}");
            return;
        }
    };
    mc(
        &minio,
        "alias set l http://127.0.0.1:9000 minioadmin minioadmin",
    )
    .await;
    mc(&minio, "mb l/plain l/versioned").await;
    mc(&minio, "version enable l/versioned").await;

    // Capabilities: a plain bucket and a versioned one.
    let plain = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "plain", "config": s3(port, "plain")}),
        )
        .await;
    assert_eq!(plain["health"], "ok", "{plain}");
    let caps = &plain["capabilities"];
    assert_eq!(
        caps["snapshots"],
        json!({"kind": "none", "create": false}),
        "{caps}"
    );
    assert_eq!(caps["atomic_write"], true);
    assert_eq!(caps["list_fast"], true);
    assert!(plain["health_latency_ms"].as_i64().is_some());
    let versioned = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "versioned", "config": s3(port, "versioned")}),
        )
        .await;
    assert_eq!(versioned["health"], "ok", "{versioned}");
    assert_eq!(
        versioned["capabilities"]["snapshots"]["kind"],
        "s3_versioning"
    );
    let id = plain["id"].as_i64().unwrap();
    let vid = versioned["id"].as_i64().unwrap();

    // A raw mount of one bucket's root and a hashed store in the other.
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "s3", "connection_id": id, "base_path": "", "mode": "raw"}),
    )
    .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "s3h", "connection_id": vid, "base_path": "h", "mode": "hashed"}),
    )
    .await;
    t.upload("s3/a.txt", b"raw file").await;
    t.upload("s3h/b.txt", b"hashed file").await;
    t.upload("local.txt", b"private root").await;

    // Probes leave nothing behind, in either bucket.
    health::probe_one(&t.st, id, true, true).await;
    health::probe_one(&t.st, vid, true, true).await;
    let objects = mc(&minio, "ls --recursive --versions l/plain l/versioned").await;
    assert!(!objects.contains(".strata-probe"), "probe junk: {objects}");
    // ... and the probe folder never shows in a raw listing.
    assert_eq!(t.names("s3").await, vec!["a.txt"]);

    // Stop the server: three failed probes make it offline.
    minio.stop().await.unwrap();
    for i in 1..=3 {
        health::probe_one(&t.st, id, true, true).await;
        health::probe_one(&t.st, vid, true, true).await;
        assert_eq!(view(&t, vid).await["health"], view(&t, id).await["health"]);
        let v = view(&t, id).await;
        let want = if i < health::OFFLINE_AFTER {
            "degraded"
        } else {
            "offline"
        };
        assert_eq!(v["health"], want, "after {i} failed probes: {v}");
    }
    let v = view(&t, id).await;
    assert_eq!(v["health_reason"], "unreachable", "{v}");
    let detail = v["health_error"].as_str().unwrap_or_default().to_string();
    assert!(!detail.is_empty(), "admins see the detail");
    assert!(
        !detail.contains("minioadmin"),
        "credentials in the error: {detail}"
    );

    // The tree still lists, with the mounts flagged offline.
    let (s, root) = t.get("/api/files/list?path=").await;
    assert_eq!(s, StatusCode::OK);
    let offline: Vec<(String, bool)> = root["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["offline"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(offline.contains(&("s3".into(), true)), "{offline:?}");
    assert!(offline.contains(&("s3h".into(), true)), "{offline:?}");
    assert!(
        offline.contains(&("local.txt".into(), false)),
        "{offline:?}"
    );
    // Everything else keeps working.
    assert_eq!(t.download("local.txt").await, b"private root");
    t.ok("POST", "/api/files/mkdir", json!({"path": "still-works"}))
        .await;
    // A hashed mount lists from the database...
    let (s, h) = t.get("/api/files/list?path=s3h").await;
    assert_eq!(s, StatusCode::OK, "{h}");
    assert_eq!(h["mount"]["offline"], true);
    assert_eq!(t.names("s3h").await, vec!["b.txt"]);
    // ... but its contents, and anything on the raw mount, are 503.
    assert_eq!(
        content(&t, "s3h/b.txt").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        content(&t, "s3/a.txt").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let (s, e) = t.get("/api/files/list?path=s3").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{e}");
    assert!(e["error"].as_str().unwrap().contains("offline"), "{e}");
    // Writes are refused, including database-only ones on the hashed mount.
    let (s, _) = t
        .json("POST", "/api/files/mkdir", json!({"path": "s3h/new"}))
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    let (s, _) = t
        .json("POST", "/api/files/delete", json!({"paths": ["s3h/b.txt"]}))
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    let (s, _, _) = t
        .raw(
            "PUT",
            &format!("/api/files/content?path={}", enc("s3/new.txt")),
            Body::from("x"),
            &[("if-none-match", "*")],
        )
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);

    // A non-admin sees why, never the backend's message.
    t.ok(
        "POST",
        "/api/admin/users",
        json!({"username": "bob", "password": "password456", "is_admin": false}),
    )
    .await;
    t.login("bob", "password456").await;
    let own = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "bob-s3", "config": s3(port, "plain")}),
        )
        .await;
    assert_eq!(own["health"], "degraded", "{own}");
    assert_eq!(own["health_reason"], "unreachable");
    assert!(own["health_error"].is_null(), "{own}");
    let tested = t
        .ok(
            "POST",
            "/api/connections/test",
            json!({"config": s3(port, "plain")}),
        )
        .await;
    assert_eq!(tested["ok"], false);
    assert!(tested["error"].is_null(), "{tested}");
    assert_eq!(tested["reason"], "unreachable");
    t.login("admin", "password123").await;

    // Back: one successful probe brings the mount online again.
    minio.start().await.unwrap();
    let mut back = false;
    for _ in 0..30 {
        health::probe_one(&t.st, id, true, true).await;
        health::probe_one(&t.st, vid, true, true).await;
        if view(&t, id).await["health"] == "ok" && view(&t, vid).await["health"] == "ok" {
            back = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(back, "MinIO came back but the connection didn't");
    assert_eq!(t.download("s3/a.txt").await, b"raw file");
    assert_eq!(t.download("s3h/b.txt").await, b"hashed file");
    t.upload("s3/after.txt", b"ok").await;
    let objects = mc(&minio, "ls --recursive --versions l/plain").await;
    assert!(!objects.contains(".strata-probe"), "probe junk: {objects}");
}

//! A local connection whose folder disappears (an unmounted disk) goes
//! offline, and nothing recreates the folder on the disk underneath.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use strata_server::health;

/// Only the connection's marker may be left in the probe folder.
fn assert_no_probe_junk(dir: &std::path::Path, id: i64) {
    let p = dir.join(".strata-probe");
    let names: Vec<String> = std::fs::read_dir(&p)
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![format!("conn-{id}")],
        "probe folder holds {names:?}"
    );
}

#[tokio::test]
async fn missing_local_folder_goes_offline_and_is_never_recreated() {
    let Some(t) = app().await else { return };
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "disk", "config": {"kind": "local", "path": "disk"}}),
        )
        .await;
    assert_eq!(c["health"], "ok", "{c}");
    let id = c["id"].as_i64().unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "disk", "connection_id": id, "base_path": "", "mode": "raw"}),
    )
    .await;
    t.upload("disk/a.txt", b"on the disk").await;
    let dir = t.st.cfg.local_base.join("disk");
    assert_no_probe_junk(&dir, id);

    // "Unmount" it.
    let parked = t.st.cfg.local_base.join("parked");
    std::fs::rename(&dir, &parked).unwrap();
    // Before any probe noticed: a write is refused at once and doesn't
    // create the folder again (a cached operator).
    let put = |p: &'static str| {
        let t = &t;
        async move {
            t.raw(
                "PUT",
                &format!("/api/files/content?path={}", enc(p)),
                axum::body::Body::from("x"),
                &[("if-none-match", "*")],
            )
            .await
            .0
        }
    };
    assert_eq!(put("disk/c.txt").await, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!dir.exists(), "a write recreated the missing folder");
    // ... nor with a fresh operator (as after a restart).
    t.st.forget_operator(id);
    assert_eq!(put("disk/d.txt").await, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!dir.exists(), "a write recreated the missing folder");
    for _ in 0..health::OFFLINE_AFTER {
        health::probe_one(&t.st, id, true, true).await;
    }
    let (_, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(v["health"], "offline", "{v}");
    assert_eq!(v["health_reason"], "not found", "{v}");
    assert!(!dir.exists(), "the probe recreated the missing folder");
    let (s, _) = t.get("/api/files/list?path=disk").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!dir.exists(), "a listing recreated the missing folder");
    // Renaming the connection doesn't touch its storage either.
    let renamed = t
        .ok(
            "PUT",
            &format!("/api/connections/{id}"),
            json!({"name": "disk2", "config": {"kind": "local", "path": "disk"}}),
        )
        .await;
    assert!(!dir.exists(), "an edit recreated the missing folder");
    // Testing the unchanged config reports the folder missing (and doesn't
    // make it).
    let tested = t
        .ok(
            "POST",
            "/api/connections/test",
            json!({"id": id, "config": {"kind": "local", "path": "disk"}}),
        )
        .await;
    assert_eq!(tested["ok"], false, "{tested}");
    assert_eq!(tested["reason"], "not found");
    assert!(!dir.exists(), "testing recreated the missing folder");
    assert_eq!(renamed["health"], "offline", "{renamed}");

    // Back.
    std::fs::rename(&parked, &dir).unwrap();
    health::probe_one(&t.st, id, true, true).await;
    let (_, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(v["health"], "ok", "{v}");
    assert_eq!(t.download("disk/a.txt").await, b"on the disk");
    t.upload("disk/b.txt", b"written after").await;
    assert_no_probe_junk(&dir, id);
}

#[tokio::test]
async fn an_empty_mount_point_left_by_an_unmounted_disk_is_not_the_folder() {
    let Some(t) = app().await else { return };
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "disk", "config": {"kind": "local", "path": "disk"}}),
        )
        .await;
    let id = c["id"].as_i64().unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "disk", "connection_id": id, "base_path": "", "mode": "raw"}),
    )
    .await;
    t.upload("disk/a.txt", b"on the disk").await;
    // A scheduled probe runs the write test; the marker survives it.
    health::probe_one(&t.st, id, true, true).await;
    let dir = t.st.cfg.local_base.join("disk");
    assert_no_probe_junk(&dir, id);

    // Unmount: the disk's contents go away, an empty mount point remains.
    let parked = t.st.cfg.local_base.join("parked");
    std::fs::rename(&dir, &parked).unwrap();
    std::fs::create_dir(&dir).unwrap();
    let (s, _, _) = t
        .raw(
            "PUT",
            &format!("/api/files/content?path={}", enc("disk/b.txt")),
            axum::body::Body::from("x"),
            &[("if-none-match", "*")],
        )
        .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    let (s, _) = t.get("/api/files/list?path=disk").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    for _ in 0..health::OFFLINE_AFTER {
        health::probe_one(&t.st, id, true, true).await;
    }
    let (_, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(v["health"], "offline", "{v}");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "something was written to the empty mount point"
    );

    // Mounted again.
    std::fs::remove_dir(&dir).unwrap();
    std::fs::rename(&parked, &dir).unwrap();
    health::probe_one(&t.st, id, true, true).await;
    let (_, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(v["health"], "ok", "{v}");
    assert_eq!(t.download("disk/a.txt").await, b"on the disk");
}

#[tokio::test]
async fn manual_checks_never_take_a_connection_offline() {
    let Some(t) = app().await else { return };
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "disk", "config": {"kind": "local", "path": "disk"}}),
        )
        .await;
    let id = c["id"].as_i64().unwrap();
    let dir = t.st.cfg.local_base.join("disk");
    std::fs::rename(&dir, t.st.cfg.local_base.join("parked")).unwrap();
    for _ in 0..5 {
        t.ok("POST", &format!("/api/connections/{id}/probe"), json!({}))
            .await;
    }
    let (_, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(v["health"], "degraded", "{v}");
    assert!(!t.st.is_offline(id));
}

#[tokio::test]
async fn busy_folders_repoints_and_nested_markers() {
    let Some(t) = app().await else { return };
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "disk", "config": {"kind": "local", "path": "a"}}),
        )
        .await;
    let id = c["id"].as_i64().unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "disk", "connection_id": id, "base_path": "", "mode": "raw"}),
    )
    .await;
    t.upload("disk/f.txt", b"one").await;

    // Many requests at once on one folder: none is refused.
    let reqs = (0..40).map(|_| async {
        t.raw(
            "GET",
            &format!("/api/files/content?path={}", enc("disk/f.txt")),
            axum::body::Body::empty(),
            &[],
        )
        .await
        .0
    });
    let codes = futures::future::join_all(reqs).await;
    assert!(codes.iter().all(|c| *c == StatusCode::OK), "{codes:?}");

    // Re-pointed to another folder: its own marker, healthy.
    let v = t
        .ok(
            "PUT",
            &format!("/api/connections/{id}"),
            json!({"name": "disk", "config": {"kind": "local", "path": "x/b"}}),
        )
        .await;
    assert_eq!(v["health"], "ok", "{v}");
    health::probe_one(&t.st, id, true, true).await;
    let (_, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(v["health"], "ok", "{v}");
    t.upload("disk/g.txt", b"two").await;
    assert!(t.st.cfg.local_base.join("x/b/g.txt").exists());

    // A connection inside another one's folder: its marker can't be reached
    // (or removed) through the outer one's raw mount.
    let outer = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "outer", "config": {"kind": "local", "path": "x"}}),
        )
        .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "outer", "connection_id": outer["id"], "base_path": "", "mode": "raw"}),
    )
    .await;
    assert!(
        !t.names("outer/b")
            .await
            .contains(&".strata-probe".to_string())
    );
    let (s, _) = t
        .json(
            "POST",
            "/api/files/delete",
            json!({"paths": ["outer/b/.strata-probe"]}),
        )
        .await;
    assert_ne!(s, StatusCode::OK);
    assert!(
        t.st.cfg
            .local_base
            .join(format!("x/b/.strata-probe/conn-{id}"))
            .exists()
    );
}

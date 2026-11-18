//! A local connection whose folder disappears (an unmounted disk) goes
//! offline, and nothing recreates the folder on the disk underneath.

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use strata_server::health;

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
    assert!(!dir.join(".strata-probe").exists(), "probe junk");

    // "Unmount" it.
    let parked = t.st.cfg.local_base.join("parked");
    std::fs::rename(&dir, &parked).unwrap();
    // A fresh operator would be built from scratch (as after a restart).
    t.st.forget_operator(id);
    for _ in 0..health::OFFLINE_AFTER {
        health::probe_one(&t.st, id, true).await;
    }
    let (_, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(v["health"], "offline", "{v}");
    assert_eq!(v["health_reason"], "not found", "{v}");
    assert!(!dir.exists(), "the probe recreated the missing folder");
    let (s, _) = t.get("/api/files/list?path=disk").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!dir.exists(), "a listing recreated the missing folder");

    // Back.
    std::fs::rename(&parked, &dir).unwrap();
    health::probe_one(&t.st, id, true).await;
    let (_, v) = t.get(&format!("/api/connections/{id}")).await;
    assert_eq!(v["health"], "ok", "{v}");
    assert_eq!(t.download("disk/a.txt").await, b"on the disk");
    t.upload("disk/b.txt", b"written after").await;
    assert!(!dir.join(".strata-probe").exists(), "probe junk");
}

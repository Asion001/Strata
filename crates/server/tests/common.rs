#![allow(dead_code)]
//! Test harness: a fresh database per test (needs `STRATA_TEST_DATABASE_URL`,
//! e.g. postgres://strata:strata@localhost:55499/strata) and an in-process router.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use strata_server::{AppState, Config, auth, routes};
use tower::ServiceExt;

pub struct TestApp {
    pub router: Router,
    pub st: AppState,
    pub cookie: Option<String>,
    pub dir: tempfile::TempDir,
}

pub async fn app() -> Option<TestApp> {
    let Ok(base) = std::env::var("STRATA_TEST_DATABASE_URL") else {
        eprintln!("skipping: STRATA_TEST_DATABASE_URL not set");
        return None;
    };
    let admin = sqlx::PgPool::connect(&base).await.expect("connect test db");
    let name = format!("strata_t_{}", rand::random::<u32>());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let url = match base.rfind('/') {
        Some(i) => format!("{}/{name}", &base[..i]),
        None => unreachable!(),
    };
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        database_url: url.clone(),
        bind: "127.0.0.1:0".into(),
        data_dir: dir.path().join("data"),
        local_base: dir.path().join("local"),
        web_dir: dir.path().join("web"),
        secret_key: "test-secret".into(),
        admin_user: Some("admin".into()),
        admin_password: Some("password123".into()),
        cookie_secure: false,
        session_days: 1,
        gc_grace_hours: 24,
        raw_ttl_secs: 0,
    };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let st = AppState::with_pool(cfg, pool).await.unwrap();
    auth::bootstrap_admin(&st).await.unwrap();
    let mut t = TestApp {
        router: routes::router(st.clone()),
        st,
        cookie: None,
        dir,
    };
    t.login("admin", "password123").await;
    Some(t)
}

impl TestApp {
    pub async fn login(&mut self, user: &str, pw: &str) {
        let req = Request::post("/api/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "username": user, "password": pw }).to_string(),
            ))
            .unwrap();
        let resp = self.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "login failed");
        let c = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
        self.cookie = Some(c.split(';').next().unwrap().to_string());
    }

    pub async fn raw(
        &self,
        method: &str,
        uri: &str,
        body: Body,
        extra: &[(&str, &str)],
    ) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-strata", "1");
        if let Some(c) = &self.cookie {
            b = b.header("cookie", c);
        }
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let resp = self
            .router
            .clone()
            .oneshot(b.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, bytes, headers)
    }

    pub async fn json(&self, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let (s, b, _) = self
            .raw(
                method,
                uri,
                Body::from(body.to_string()),
                &[("content-type", "application/json")],
            )
            .await;
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    pub async fn get(&self, uri: &str) -> (StatusCode, Value) {
        let (s, b, _) = self.raw("GET", uri, Body::empty(), &[]).await;
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    pub async fn ok(&self, method: &str, uri: &str, body: Value) -> Value {
        let (s, v) = self.json(method, uri, body).await;
        assert!(s.is_success(), "{method} {uri} -> {s}: {v}");
        v
    }

    pub async fn names(&self, path: &str) -> Vec<String> {
        let (s, v) = self
            .get(&format!("/api/files/list?path={}", enc(path)))
            .await;
        assert_eq!(s, StatusCode::OK, "list {path}: {v}");
        v["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// Upload using the real chunked protocol. Returns the "missing" list of
    /// the first call (to test dedupe).
    pub async fn upload(&self, path: &str, data: &[u8]) -> Vec<usize> {
        let lens = strata_storage::chunker::chunk_lengths(data);
        let mut chunks = Vec::new();
        let mut off = 0;
        for l in &lens {
            let c = &data[off..off + l];
            chunks.push((blake3::hash(c).to_hex().to_string(), off, *l));
            off += l;
        }
        let spec: Vec<Value> = chunks
            .iter()
            .map(|(h, _, l)| serde_json::json!({ "hash": h, "size": l }))
            .collect();
        let v = self
            .ok(
                "POST",
                "/api/uploads",
                serde_json::json!({ "path": path, "size": data.len(), "chunks": spec }),
            )
            .await;
        let id = v["id"].as_str().unwrap().to_string();
        let missing: Vec<usize> = v["missing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap() as usize)
            .collect();
        for &i in &missing {
            let (h, o, l) = &chunks[i];
            let (s, b, _) = self
                .raw(
                    "PUT",
                    &format!("/api/uploads/{id}/chunks/{h}"),
                    Body::from(data[*o..o + l].to_vec()),
                    &[],
                )
                .await;
            assert!(
                s.is_success(),
                "chunk put {s}: {}",
                String::from_utf8_lossy(&b)
            );
        }
        self.ok(
            "POST",
            &format!("/api/uploads/{id}/commit"),
            serde_json::json!({}),
        )
        .await;
        missing
    }

    pub async fn download(&self, path: &str) -> Vec<u8> {
        let (s, b, _) = self
            .raw(
                "GET",
                &format!("/api/files/content?path={}", enc(path)),
                Body::empty(),
                &[],
            )
            .await;
        assert_eq!(
            s,
            StatusCode::OK,
            "download {path}: {}",
            String::from_utf8_lossy(&b)
        );
        b
    }
}

pub fn enc(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

pub fn pseudo(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) as u8
        })
        .collect()
}

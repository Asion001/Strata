//! The chunked upload protocol through the real router, against Postgres
//! (`STRATA_TEST_DATABASE_URL`; one database per fuzzing process): random
//! sequences of create, status (the missing list), put-chunk and commit,
//! with random hashes and lengths, on a hashed and a raw volume.
//!
//! Never a 5xx or a panic, and a committed file always is exactly its
//! declared chunks: its size is the sum of their sizes, and each slice of
//! its bytes hashes to its chunk's hash.
#![no_main]

use std::sync::OnceLock;

use arbitrary::Arbitrary;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use libfuzzer_sys::fuzz_target;
use serde_json::{Value, json};
use strata_server::{AppState, Config, auth, routes};
use tower::ServiceExt;

/// Up to this many chunks of pool data per upload, and steps per input.
const MAX_CHUNKS: usize = 6;
const MAX_STEPS: usize = 12;

#[derive(Arbitrary, Debug)]
enum Chunk {
    /// A piece of the pool, declared truthfully.
    Pool(u8),
    /// A piece of the pool with a wrong declared size.
    PoolSize(u8, u16),
    /// Any hash and size.
    Any([u8; 32], u16),
}

#[derive(Arbitrary, Debug)]
enum Step {
    Create {
        /// 0..4 in the home folder (hashed), 4..8 in the raw mount.
        file: u8,
        chunks: Vec<Chunk>,
        /// Declare a total other than the sum of the chunk sizes.
        size_off: Option<i16>,
    },
    Status(u8),
    /// Send pool piece `data` for the chunk whose hash is that of piece
    /// `hash` (the same piece: a correct send).
    Put {
        upload: u8,
        hash: u8,
        data: u8,
    },
    /// Send bytes under a hash that isn't theirs.
    PutAny {
        upload: u8,
        hash: [u8; 32],
        data: Vec<u8>,
    },
    Commit(u8),
}

#[derive(Arbitrary, Debug)]
struct Input {
    pool: Vec<Vec<u8>>,
    steps: Vec<Step>,
}

struct App {
    rt: tokio::runtime::Runtime,
    router: Router,
    cookie: String,
    _dir: tempfile::TempDir,
}

fn app() -> &'static App {
    static APP: OnceLock<App> = OnceLock::new();
    APP.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let base = std::env::var("STRATA_TEST_DATABASE_URL")
            .expect("upload_protocol needs STRATA_TEST_DATABASE_URL (a Postgres it may create databases in)");
        let dir = tempfile::tempdir().unwrap();
        let (router, cookie) = rt.block_on(async {
            let admin = sqlx::PgPool::connect(&base).await.expect("connect");
            let name = format!("strata_fuzz_{}_{}", std::process::id(), rand::random::<u32>());
            sqlx::query(&format!("CREATE DATABASE {name}")).execute(&admin).await.unwrap();
            let url = format!("{}/{name}", &base[..base.rfind('/').unwrap()]);
            let cfg = Config {
                database_url: url.clone(),
                bind: "127.0.0.1:0".into(),
                data_dir: dir.path().join("state"),
                local_base: dir.path().join("local"),
                default_store: "blobs".into(),
                files_dir: "files".into(),
                web_dir: dir.path().join("web"),
                secret_key: "fuzz-secret".into(),
                admin_user: Some("admin".into()),
                admin_password: Some("password123".into()),
                cookie_secure: false,
                session_days: 1,
                gc_grace_hours: 24,
                raw_ttl_secs: 0,
                probe_secs: 3600,
                trash_days: 30,
            };
            let pool = sqlx::PgPool::connect(&url).await.unwrap();
            let st = AppState::with_pool(cfg, pool).await.unwrap();
            auth::bootstrap_admin(&st).await.unwrap();
            let router = routes::router(st);
            let login = Request::post("/api/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(json!({"username": "admin", "password": "password123"}).to_string()))
                .unwrap();
            let resp = router.clone().oneshot(login).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let cookie = resp.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
            // A raw volume next to the hashed home folder.
            let conn = call(&router, &cookie, "POST", "/api/connections",
                Body::from(json!({"name": "disk", "config": {"kind": "local", "path": "raw"}}).to_string())).await;
            assert_eq!(conn.0, StatusCode::OK, "{:?}", conn.1);
            let m = call(&router, &cookie, "POST", "/api/mounts",
                Body::from(json!({"path": "raw", "connection_id": conn.1["id"], "base_path": "", "mode": "raw"}).to_string())).await;
            assert_eq!(m.0, StatusCode::OK, "{:?}", m.1);
            (router, cookie)
        });
        App { rt, router, cookie, _dir: dir }
    })
}

async fn call(
    router: &Router,
    cookie: &str,
    method: &str,
    uri: &str,
    body: Body,
) -> (StatusCode, Value, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-strata", "1")
        .header("cookie", cookie)
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert!(!status.is_server_error(), "{method} {uri}: {status} {v}");
    (status, v, bytes)
}

/// An upload as created: its file and declared chunks.
struct Made {
    id: String,
    path: String,
    chunks: Vec<([u8; 32], usize)>,
}

fuzz_target!(|input: Input| {
    let a = app();
    let mut pool: Vec<Vec<u8>> = input
        .pool
        .into_iter()
        .take(8)
        .map(|mut p| {
            p.truncate(300);
            p
        })
        .collect();
    if pool.is_empty() {
        pool.push(b"x".to_vec());
    }
    let piece = |i: u8| &pool[i as usize % pool.len()];
    a.rt.block_on(async {
        let mut made: Vec<Made> = Vec::new();
        for step in input.steps.into_iter().take(MAX_STEPS) {
            match step {
                Step::Create { file, chunks, size_off } => {
                    let path = match file % 8 {
                        f @ 0..4 => format!("f{f}"),
                        f => format!("raw/f{}", f - 4),
                    };
                    let chunks: Vec<([u8; 32], usize)> = chunks
                        .into_iter()
                        .take(MAX_CHUNKS)
                        .map(|c| match c {
                            Chunk::Pool(i) => (*blake3::hash(piece(i)).as_bytes(), piece(i).len()),
                            Chunk::PoolSize(i, s) => (*blake3::hash(piece(i)).as_bytes(), s as usize % 512),
                            Chunk::Any(h, s) => (h, s as usize % 512),
                        })
                        .collect();
                    let sum: i64 = chunks.iter().map(|c| c.1 as i64).sum();
                    let size = sum + size_off.map_or(0, i64::from);
                    let body = json!({
                        "path": path,
                        "size": size,
                        "chunks": chunks.iter().map(|(h, s)| json!({"hash": hex::encode(h), "size": s})).collect::<Vec<_>>(),
                    });
                    let (s, v, _) = call(&a.router, &a.cookie, "POST", "/api/uploads", Body::from(body.to_string())).await;
                    if s == StatusCode::OK {
                        made.push(Made { id: v["id"].as_str().unwrap().to_string(), path, chunks });
                    }
                }
                Step::Status(u) => {
                    let Some(m) = pick(&made, u) else { continue };
                    let (s, v, _) = call(&a.router, &a.cookie, "GET", &format!("/api/uploads/{}", m.id), Body::empty()).await;
                    if s == StatusCode::OK {
                        for i in v["missing"].as_array().unwrap() {
                            assert!((i.as_u64().unwrap() as usize) < m.chunks.len(), "missing index out of range: {v}");
                        }
                    }
                }
                Step::Put { upload, hash, data } => {
                    let Some(m) = pick(&made, upload) else { continue };
                    let h = blake3::hash(piece(hash)).to_hex().to_string();
                    let uri = format!("/api/uploads/{}/chunks/{h}", m.id);
                    call(&a.router, &a.cookie, "PUT", &uri, Body::from(piece(data).clone())).await;
                }
                Step::PutAny { upload, hash, data } => {
                    let Some(m) = pick(&made, upload) else { continue };
                    let uri = format!("/api/uploads/{}/chunks/{}", m.id, hex::encode(hash));
                    let (s, ..) = call(&a.router, &a.cookie, "PUT", &uri, Body::from(data.clone())).await;
                    if *blake3::hash(&data).as_bytes() != hash {
                        assert_ne!(s, StatusCode::OK, "a chunk was taken under someone else's hash");
                    }
                }
                Step::Commit(u) => {
                    let Some(m) = pick(&made, u) else { continue };
                    let (s, v, _) = call(&a.router, &a.cookie, "POST", &format!("/api/uploads/{}/commit", m.id), Body::empty()).await;
                    if s != StatusCode::OK {
                        continue;
                    }
                    let enc: String = m.path.replace('/', "%2F");
                    let (s, _, bytes) = call(&a.router, &a.cookie, "GET", &format!("/api/files/content?path={enc}"), Body::empty()).await;
                    assert_eq!(s, StatusCode::OK, "committed {} but can't read it", m.path);
                    let want: usize = m.chunks.iter().map(|c| c.1).sum();
                    assert_eq!(bytes.len(), want, "{}: {} bytes, chunks declare {want} ({v})", m.path, bytes.len());
                    assert_eq!(v["size"].as_u64(), Some(want as u64), "commit answer {v}");
                    let mut off = 0;
                    for (h, len) in &m.chunks {
                        assert_eq!(blake3::hash(&bytes[off..off + len]).as_bytes(), h, "{}: chunk at {off} differs", m.path);
                        off += len;
                    }
                }
            }
        }
    });
});

fn pick(made: &[Made], i: u8) -> Option<&Made> {
    (!made.is_empty()).then(|| &made[i as usize % made.len()])
}

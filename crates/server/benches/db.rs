//! `cargo bench -p strata-server --bench db`: server time of the paths that
//! touch Postgres, through the real router in process (no network). Needs
//! `STRATA_TEST_DATABASE_URL` (a Postgres it may create a database in).
//!
//! * `db/list_10k_entries`: `GET /api/files/list` of a folder holding 10,000
//!   files on a hashed mount: the query and the JSON answer.
//! * `db/move_10k_node_subtree`: `POST /api/files/move` of a folder holding
//!   10,000 nodes (100 folders of 99 files) on a hashed mount, back and
//!   forth: one move per iteration.
//! * `db/journal_append`: one change appended to a volume's journal.
//!
//! Cache state: warm. Criterion's warm-up runs every path first, so
//! Postgres' shared buffers, the OS page cache and the server's own caches
//! (mounts, sessions) hold what the measured iterations touch. A cold run
//! would need Postgres restarted and the OS cache dropped between
//! iterations, which a shared CI runner can't do. Answers are read to the
//! end and go through `black_box`, so no work is skipped.
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use criterion::{Criterion, criterion_group, criterion_main};
use http_body_util::BodyExt;
use serde_json::json;
use strata_server::journal::{self, Change};
use strata_server::{AppState, Config, auth, routes};
use tower::ServiceExt;

/// Files in the listed folder, and folders × files in the moved one.
const ENTRIES: i64 = 10_000;
const DIRS: i64 = 100;
const PER_DIR: i64 = 99;

struct Bench {
    st: AppState,
    router: Router,
    cookie: String,
    volume: i64,
    _dir: tempfile::TempDir,
}

async fn setup() -> Bench {
    let base = std::env::var("STRATA_TEST_DATABASE_URL")
        .expect("the db benches need STRATA_TEST_DATABASE_URL");
    let admin = sqlx::PgPool::connect(&base).await.expect("connect");
    // Databases of earlier runs go first (one run at a time per server).
    let old: Vec<String> = sqlx::query_scalar(
        "SELECT datname FROM pg_database WHERE datname LIKE 'strata\\_bench\\_%'",
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    for db in old {
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{db}\" WITH (FORCE)"))
            .execute(&admin)
            .await;
    }
    let name = format!("strata_bench_{}", std::process::id());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let url = format!("{}/{name}", &base[..base.rfind('/').unwrap()]);
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        database_url: url.clone(),
        bind: "127.0.0.1:0".into(),
        data_dir: dir.path().join("state"),
        local_base: dir.path().join("local"),
        default_store: "blobs".into(),
        files_dir: "files".into(),
        web_dir: dir.path().join("web"),
        secret_key: "bench-secret".into(),
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
    let router = routes::router(st.clone());
    let login = Request::post("/api/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"username": "admin", "password": "password123"}).to_string(),
        ))
        .unwrap();
    let resp = router.clone().oneshot(login).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    // The home folder: a hashed volume.
    let volume: i64 =
        sqlx::query_scalar("SELECT volume_id FROM mounts WHERE owner_id = 1 AND path = ''")
            .fetch_one(&st.db)
            .await
            .unwrap();
    // Seeded straight into the tree (10,000 uploads would take minutes);
    // the rows are what uploads leave on a hashed volume.
    let seed = [
        "INSERT INTO nodes (volume_id, parent, name, is_dir) VALUES ($1, '', 'big', TRUE), ($1, '', 'tree', TRUE)",
        "INSERT INTO nodes (volume_id, parent, name, is_dir, size, etag, content_hash, chunks, chunk_sizes)
         SELECT $1, 'big', 'file-' || lpad(i::text, 5, '0') || '.jpg', FALSE, 1000 + i, md5(i::text),
                sha256(i::text::bytea), ARRAY[sha256(i::text::bytea)], ARRAY[1000 + i::bigint]
         FROM generate_series(1, $2) i",
        "INSERT INTO nodes (volume_id, parent, name, is_dir)
         SELECT $1, 'tree', 'd' || lpad(d::text, 3, '0'), TRUE FROM generate_series(1, $3) d",
        "INSERT INTO nodes (volume_id, parent, name, is_dir, size, etag, content_hash, chunks, chunk_sizes)
         SELECT $1, 'tree/d' || lpad(d::text, 3, '0'), 'f' || lpad(f::text, 3, '0'), FALSE, 10, md5(d::text || f::text),
                sha256((d * 1000 + f)::text::bytea), ARRAY[sha256((d * 1000 + f)::text::bytea)], ARRAY[10::bigint]
         FROM generate_series(1, $3) d, generate_series(1, $4) f",
    ];
    for q in seed {
        sqlx::query(q)
            .bind(volume)
            .bind(ENTRIES)
            .bind(DIRS)
            .bind(PER_DIR)
            .execute(&st.db)
            .await
            .unwrap();
    }
    sqlx::query("ANALYZE nodes").execute(&st.db).await.unwrap();
    Bench {
        st,
        router,
        cookie,
        volume,
        _dir: dir,
    }
}

impl Bench {
    /// One request, its answer read to the end; must succeed.
    async fn call(&self, method: &str, uri: &str, body: Option<serde_json::Value>) -> Vec<u8> {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("cookie", &self.cookie)
            .header("x-strata", "1")
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        assert!(status.is_success(), "{method} {uri}: {status}");
        bytes
    }
}

fn db(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let b = rt.block_on(setup());
    let mut g = c.benchmark_group("db");
    g.sample_size(30).measurement_time(Duration::from_secs(10));

    // Every entry comes back (or the bench measures an error page).
    let listed = rt.block_on(b.call("GET", "/api/files/list?path=big", None));
    let n = serde_json::from_slice::<serde_json::Value>(&listed).unwrap()["entries"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(n as i64, ENTRIES);
    g.bench_function("list_10k_entries", |i| {
        i.to_async(&rt)
            .iter(|| async { black_box(b.call("GET", "/api/files/list?path=big", None).await) })
    });

    // Back and forth: every iteration moves the whole subtree. Each move
    // leaves 10,000 dead rows; a VACUUM after each one, outside the timed
    // part, keeps the table as autovacuum would between real moves (else
    // the bench would measure its own bloat).
    let flips = AtomicU64::new(0);
    g.bench_function("move_10k_node_subtree", |i| {
        i.to_async(&rt).iter_custom(|iters| {
            let (b, flips) = (&b, &flips);
            async move {
                let mut took = Duration::ZERO;
                for _ in 0..iters {
                    let (from, to) = if flips.fetch_add(1, Ordering::Relaxed) % 2 == 0 {
                        ("tree", "moved")
                    } else {
                        ("moved", "tree")
                    };
                    let t = std::time::Instant::now();
                    black_box(
                        b.call(
                            "POST",
                            "/api/files/move",
                            Some(json!({"from": from, "to": to})),
                        )
                        .await,
                    );
                    took += t.elapsed();
                    sqlx::query("VACUUM nodes").execute(&b.st.db).await.unwrap();
                }
                took
            }
        })
    });
    // (Still all there, wherever it ended.)
    let moved: i64 = rt.block_on(
        sqlx::query_scalar(
            "SELECT count(*) FROM nodes WHERE volume_id = $1 AND (parent LIKE 'tree%' OR parent LIKE 'moved%')",
        )
        .bind(b.volume)
        .fetch_one(&b.st.db),
    )
    .unwrap();
    assert_eq!(moved, DIRS + DIRS * PER_DIR);

    let n = AtomicU64::new(0);
    g.bench_function("journal_append", |i| {
        i.to_async(&rt).iter(|| async {
            let path = format!("journal/{}", n.fetch_add(1, Ordering::Relaxed));
            journal::record_volume(&b.st, b.volume, vec![Change::create(&path, false)])
                .await
                .unwrap();
        })
    });
    g.finish();
}

criterion_group!(benches, db);
criterion_main!(benches);

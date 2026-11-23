//! Byte ranges on the download route (RFC 9110): hashed and raw mounts on a
//! local folder, S3 (MinIO) and FTP. Every response is compared with the
//! original bytes.
//!
//! Needs `STRATA_TEST_DATABASE_URL`; the MinIO and FTP tests also need
//! Docker (skipped without it, except in CI where they must run).

mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::*;
use rand::{Rng, SeedableRng};
use serde_json::json;
use testcontainers_modules::minio::MinIO;
use testcontainers_modules::testcontainers::core::{ExecCommand, IntoContainerPort, WaitFor};
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, GenericImage, ImageExt};

fn data(len: usize, seed: u64) -> Vec<u8> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    (0..len).map(|_| rng.r#gen()).collect()
}

async fn get(
    t: &TestApp,
    path: &str,
    hdrs: &[(&str, &str)],
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    t.raw(
        "GET",
        &format!("/api/files/content?path={}", enc(path)),
        Body::empty(),
        hdrs,
    )
    .await
}

/// Check every interesting range of `path` against `data`. `strong`: the
/// backend's version is exact (else If-Range never lets a range through:
/// a coarse version can miss a change).
async fn check_ranges(t: &TestApp, path: &str, data: &[u8], chunk_bounds: &[usize], strong: bool) {
    let n = data.len() as u64;
    let (s, body, h) = get(t, path, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, data);
    assert_eq!(h["accept-ranges"], "bytes");
    assert_eq!(h["content-length"], n.to_string());
    let etag = h["etag"].to_str().unwrap().to_string();

    // HEAD: size, type and ETag, no body
    let (s, body, hh) = t
        .raw(
            "HEAD",
            &format!("/api/files/content?path={}", enc(path)),
            Body::empty(),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.is_empty());
    assert_eq!(hh["content-length"], n.to_string());
    assert_eq!(hh["etag"], etag.as_str());
    assert!(hh.contains_key("content-type"));

    // ranges: edges, chunk boundaries (one byte either side) and random ones
    let mut ranges: Vec<(u64, u64)> = vec![(0, 0), (0, n - 1), (n - 1, n - 1), (1, n - 2)];
    for &b in chunk_bounds {
        let b = b as u64;
        for (a, z) in [
            (b - 1, b),
            (b, b),
            (b - 1, b - 1),
            (b.saturating_sub(5), b + 5),
        ] {
            if z < n {
                ranges.push((a, z));
            }
        }
    }
    let mut rng = rand::rngs::StdRng::seed_from_u64(n);
    for _ in 0..40 {
        let a = rng.gen_range(0..n);
        ranges.push((a, rng.gen_range(a..n)));
    }
    for (a, z) in ranges {
        let (s, body, h) = get(t, path, &[("range", &format!("bytes={a}-{z}"))]).await;
        assert_eq!(s, StatusCode::PARTIAL_CONTENT, "{a}-{z}");
        assert_eq!(body, &data[a as usize..=z as usize], "{a}-{z}");
        assert_eq!(h["content-range"], format!("bytes {a}-{z}/{n}"));
        assert_eq!(h["content-length"], (z - a + 1).to_string());
    }
    // open-ended and suffix forms
    let (s, body, _) = get(t, path, &[("range", "bytes=-7")]).await;
    assert_eq!(
        (s, body.as_slice()),
        (StatusCode::PARTIAL_CONTENT, &data[data.len() - 7..])
    );
    let (s, body, _) = get(t, path, &[("range", &format!("bytes={}-", n - 3))]).await;
    assert_eq!(
        (s, body.as_slice()),
        (StatusCode::PARTIAL_CONTENT, &data[data.len() - 3..])
    );
    // clamped past the end, and a huge last-byte-pos
    let (s, body, h) = get(t, path, &[("range", "bytes=0-99999999999999999999999")]).await;
    assert_eq!((s, body.len() as u64), (StatusCode::PARTIAL_CONTENT, n));
    assert_eq!(h["content-range"], format!("bytes 0-{}/{n}", n - 1));
    // unsatisfiable: 416 with the size
    for r in [
        format!("bytes={n}-"),
        "bytes=-0".into(),
        format!("bytes={}-", u64::MAX),
    ] {
        let (s, _, h) = get(t, path, &[("range", &r)]).await;
        assert_eq!(s, StatusCode::RANGE_NOT_SATISFIABLE, "{r}");
        assert_eq!(h["content-range"], format!("bytes */{n}"));
    }
    // ignored: several ranges, other units, nonsense: the whole file
    for r in ["bytes=0-1,5-6", "items=0-1", "bytes=5-2", "bytes=x-"] {
        let (s, body, _) = get(t, path, &[("range", r)]).await;
        assert_eq!((s, body.len() as u64), (StatusCode::OK, n), "{r}");
    }
    // If-Range: the current ETag gets the range, anything else the file
    let (s, body, _) = get(t, path, &[("range", "bytes=0-9"), ("if-range", &etag)]).await;
    if strong {
        assert_eq!((s, body.len()), (StatusCode::PARTIAL_CONTENT, 10));
    } else {
        assert_eq!((s, body.len() as u64), (StatusCode::OK, n));
    }
    for other in ["\"r3-old\"", "W/\"x\"", "Tue, 15 Nov 1994 08:12:31 GMT"] {
        let (s, body, _) = get(t, path, &[("range", "bytes=0-9"), ("if-range", other)]).await;
        assert_eq!((s, body.len() as u64), (StatusCode::OK, n), "{other}");
    }
}

fn bounds(lens: &[usize]) -> Vec<usize> {
    lens.iter()
        .scan(0, |off, l| {
            *off += l;
            Some(*off)
        })
        .take(lens.len().saturating_sub(1))
        .collect()
}

#[tokio::test]
async fn hashed_and_raw_local() {
    let Some(t) = app().await else { return };
    // several content-defined chunks
    let d = data(12 << 20, 1);
    t.upload("big.bin", &d).await;
    let lens = strata_storage::chunker::chunk_lengths(&d);
    assert!(lens.len() > 1, "want several chunks");
    check_ranges(&t, "big.bin", &d, &bounds(&lens), true).await;

    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "disk", "config": {"kind": "local", "path": "disk"}}),
        )
        .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "disk", "connection_id": c["id"], "base_path": "", "mode": "raw"}),
    )
    .await;
    let d = data(1 << 20, 2);
    t.upload("disk/raw.bin", &d).await;
    check_ranges(&t, "disk/raw.bin", &d, &[], true).await;
}

fn require_docker(e: impl std::fmt::Debug, what: &str) -> Option<()> {
    if std::env::var_os("CI").is_some() {
        panic!("can't start {what}: {e:?}");
    }
    eprintln!("skipping: can't start {what} (Docker running?): {e:?}");
    None
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn raw_on_minio() {
    let Some(t) = app().await else { return };
    let port = free_port();
    let minio: ContainerAsync<MinIO> = match MinIO::default()
        .with_name("cgr.dev/chainguard/minio")
        .with_tag("latest@sha256:304e0bbeb70a9aa0c40ff26d5a9c0a24296019c6d326ba8b75214fb73441d6e9")
        .with_mapped_port(port, 9000.tcp())
        .start()
        .await
    {
        Ok(c) => c,
        Err(e) => return require_docker(e, "MinIO").unwrap_or(()),
    };
    minio
        .exec(ExecCommand::new([
            "sh",
            "-c",
            "mc alias set l http://127.0.0.1:9000 minioadmin minioadmin && mc mb l/media",
        ]))
        .await
        .unwrap();
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "s3", "config": {"kind": "s3", "bucket": "media", "region": "us-east-1",
                "endpoint": format!("http://127.0.0.1:{port}"),
                "access_key_id": "minioadmin", "secret_access_key": "minioadmin"}}),
        )
        .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "s3", "connection_id": c["id"], "base_path": "", "mode": "raw"}),
    )
    .await;
    let d = data(1 << 20, 3);
    t.upload("s3/raw.bin", &d).await;
    check_ranges(&t, "s3/raw.bin", &d, &[], true).await;
}

#[tokio::test]
async fn raw_on_ftp() {
    let Some(t) = app().await else { return };
    // Passive data ports are published 1:1, so the server's PASV reply
    // (127.0.0.1:<port>) is reachable from the host.
    let control = free_port();
    let pasv = 21100;
    let mut img = GenericImage::new(
        "delfer/alpine-ftp-server",
        "latest@sha256:b27013e7436cea343826ba7edb77aa3a8b2d22df14e366b82d10a3b0b5ecc880",
    )
    .with_wait_for(WaitFor::seconds(1))
    .with_env_var("USERS", "strata|secret123")
    .with_env_var("ADDRESS", "127.0.0.1")
    .with_env_var("MIN_PORT", pasv.to_string())
    .with_env_var("MAX_PORT", (pasv + 4).to_string())
    .with_mapped_port(control, 21.tcp());
    for p in pasv..=pasv + 4 {
        img = img.with_mapped_port(p, p.tcp());
    }
    let _ftp = match img.start().await {
        Ok(c) => c,
        Err(e) => return require_docker(e, "an FTP server").unwrap_or(()),
    };
    // Ready once it greets (a slow runner can take a while).
    let mut ready = false;
    for _ in 0..60 {
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", control)) {
            use std::io::Read;
            s.set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .ok();
            let mut b = [0u8; 3];
            if s.read_exact(&mut b).is_ok() && &b == b"220" {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(ready, "the FTP server never greeted");
    let c = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "ftp", "config": {"kind": "ftp",
                "endpoint": format!("ftp://127.0.0.1:{control}"),
                "user": "strata", "password": "secret123",
                // (this server doesn't chroot users)
                "root": "/ftp/strata"}}),
        )
        .await;
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "ftp", "connection_id": c["id"], "base_path": "", "mode": "raw"}),
    )
    .await;
    let d = data(256 << 10, 4);
    t.upload("ftp/raw.bin", &d).await;
    // FTP versions are mtime-based, seconds at best.
    check_ranges(&t, "ftp/raw.bin", &d, &[], false).await;
}

/// A range in the middle of a large hashed file reads only the chunks that
/// cover it (plus the file's first bytes, for its type): with every other
/// chunk deleted from storage, the range still comes back intact.
#[tokio::test]
async fn hashed_range_reads_only_covering_chunks() {
    let Some(t) = app().await else { return };
    let d = data(40 << 20, 5);
    t.upload("huge.bin", &d).await;
    let admin: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
        .fetch_one(&t.st.db)
        .await
        .unwrap();
    let (m, rel) = strata_server::mounts::resolve(&t.st, admin, "huge.bin")
        .await
        .unwrap();
    let node = strata_server::vfs::get_node(&t.st, m.volume_id, &rel)
        .await
        .unwrap()
        .unwrap();
    let chunks = node.chunks.clone().unwrap();
    let sizes: Vec<u64> = node
        .chunk_sizes
        .clone()
        .unwrap()
        .iter()
        .map(|&s| s as u64)
        .collect();
    assert!(chunks.len() >= 8, "want many chunks, got {}", chunks.len());

    let (a, z) = (17_000_000u64, 17_500_000u64); // inclusive
    let keep: Vec<usize> = strata_server::range::covering(&sizes, a, z + 1)
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert!(
        keep.len() <= 2,
        "a 500 KB range spans at most two chunks here"
    );
    // The first chunk stays too: the content type is sniffed from the
    // file's first bytes (once; then it is cached).
    let mut keep = keep;
    keep.push(0);
    let store = strata_server::vfs::hashed_store(&t.st, &m).await.unwrap();
    for (i, c) in chunks.iter().enumerate() {
        if !keep.contains(&i) {
            store
                .delete(&strata_storage::Hash::from_slice(c).unwrap())
                .await
                .unwrap();
        }
    }
    let (s, body, _) = get(&t, "huge.bin", &[("range", &format!("bytes={a}-{z}"))]).await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, &d[a as usize..=z as usize]);
    // sanity: the other chunks really are gone
    let gone = (0..chunks.len()).find(|i| !keep.contains(i)).unwrap();
    let h = strata_storage::Hash::from_slice(&chunks[gone]).unwrap();
    assert!(!store.exists(&h).await.unwrap());
}

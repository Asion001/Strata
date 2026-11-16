//! Every route that returns user file bytes must be safe (task 006):
//! sandboxing headers, a sniffed Content-Type, a header-safe disposition.
mod common;

use axum::body::Body;
use axum::http::{HeaderMap, StatusCode};
use common::*;
use serde_json::json;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/tests/fixtures/evil");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}")).unwrap()
}

fn assert_protected(h: &HeaderMap, what: &str) {
    assert_eq!(
        h["content-security-policy"],
        "sandbox; default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'",
        "{what}"
    );
    assert_eq!(h["x-content-type-options"], "nosniff", "{what}");
    assert_eq!(h["cross-origin-resource-policy"], "same-origin", "{what}");
    assert_eq!(h["referrer-policy"], "no-referrer", "{what}");
}

fn ctype(h: &HeaderMap) -> &str {
    h["content-type"].to_str().unwrap()
}

#[tokio::test]
async fn every_file_route_is_protected() {
    let Some(t) = app().await else { return };
    t.upload("evil/evil.html", &fixture("evil.html")).await;
    t.upload("evil/evil.svg", &fixture("evil.svg")).await;
    t.upload("evil/polyglot.jpg", &fixture("polyglot.jpg"))
        .await;
    t.upload("evil/polyglot.html", &fixture("polyglot.html"))
        .await;
    t.upload("evil/real.png", &png()).await;

    let get = |uri: String| {
        let t = &t;
        async move { t.raw("GET", &uri, Body::empty(), &[]).await }
    };
    // authenticated routes
    for name in [
        "evil.html",
        "evil.svg",
        "polyglot.jpg",
        "polyglot.html",
        "real.png",
    ] {
        for route in ["content", "image"] {
            let (s, _, h) = get(format!("/api/files/{route}?path=evil/{name}")).await;
            assert_eq!(s, StatusCode::OK, "{route} {name}");
            assert_protected(&h, &format!("{route} {name}"));
        }
        let (_, _, h) = get(format!("/api/files/content?path=evil/{name}&download=1")).await;
        assert_protected(&h, name);
        assert!(
            h["content-disposition"]
                .to_str()
                .unwrap()
                .starts_with("attachment;")
        );
    }
    let (s, _, h) = get("/api/files/thumb?path=evil/real.png".into()).await;
    assert_eq!(s, StatusCode::OK);
    assert_protected(&h, "thumb");
    // range responses too
    let (s, _, h) = t
        .raw(
            "GET",
            "/api/files/content?path=evil/evil.html",
            Body::empty(),
            &[("range", "bytes=0-9")],
        )
        .await;
    assert_eq!(s, StatusCode::PARTIAL_CONTENT);
    assert_protected(&h, "range");

    // Content-Type decisions: active content is never served as such
    let ct = |route: &str, name: &str| {
        let uri = format!("/api/files/{route}?path=evil/{name}");
        async move { ctype(&get(uri).await.2).to_string() }
    };
    assert_eq!(
        ct("content", "evil.html").await,
        "text/plain; charset=utf-8"
    );
    assert_eq!(ct("content", "evil.svg").await, "text/plain; charset=utf-8");
    assert_eq!(ct("content", "polyglot.html").await, "image/jpeg");
    assert_eq!(ct("content", "polyglot.jpg").await, "image/jpeg");
    assert_eq!(ct("image", "evil.svg").await, "image/svg+xml");
    assert_eq!(ct("image", "evil.html").await, "text/plain; charset=utf-8");
    assert_eq!(ct("content", "real.png").await, "image/png");

    // public share routes behave the same
    let s = t.ok("POST", "/api/shares", json!({"path": "evil"})).await;
    let token = s["token"].as_str().unwrap().to_string();
    let mut anon = TestApp {
        router: t.router.clone(),
        st: t.st.clone(),
        cookie: None,
        dir: tempfile::tempdir().unwrap(),
    };
    anon.cookie = None;
    for name in ["evil.html", "evil.svg", "polyglot.html"] {
        for route in ["content", "image"] {
            let (s, _, h) = anon
                .raw(
                    "GET",
                    &format!("/api/public/{token}/{route}?path={name}"),
                    Body::empty(),
                    &[],
                )
                .await;
            assert_eq!(s, StatusCode::OK);
            assert_protected(&h, &format!("public {route} {name}"));
            let want = match (route, name) {
                ("image", "evil.svg") => "image/svg+xml",
                (_, "polyglot.html") => "image/jpeg",
                _ => "text/plain; charset=utf-8",
            };
            assert_eq!(ctype(&h), want, "public {route} {name}");
        }
    }
    let (s, _, h) = anon
        .raw(
            "GET",
            &format!("/api/public/{token}/thumb?path=real.png"),
            Body::empty(),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_protected(&h, "public thumb");
}

#[tokio::test]
async fn hostile_file_names_cannot_inject_headers() {
    let Some(t) = app().await else { return };
    // control characters are refused outright
    let h = blake3::hash(b"x").to_hex().to_string();
    let (s, _) = t
        .json("POST", "/api/uploads", json!({"path": "a\r\nSet-Cookie: pwned=1.txt", "size": 1, "chunks": [{"hash": h, "size": 1}]}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // quotes and backslashes are stripped from the header
    t.upload("q\"uo\\te; Set-Cookie: pwned=1.txt", b"x").await;
    let (s, _, hd) = t
        .raw(
            "GET",
            &format!(
                "/api/files/content?path={}&download=1",
                enc("q\"uo\\te; Set-Cookie: pwned=1.txt")
            ),
            Body::empty(),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(hd.get("set-cookie").is_none());
    let cd = hd["content-disposition"].to_str().unwrap();
    assert_eq!(cd.matches('"').count(), 2, "{cd}");
    assert!(
        cd.starts_with(
            "attachment; filename=\"quote; Set-Cookie: pwned=1.txt\"; filename*=UTF-8''"
        ),
        "{cd}"
    );
    // a raw folder holding such a name never lists or serves it
    let cid = t
        .ok(
            "POST",
            "/api/connections",
            json!({"name": "d", "config": {"kind": "local", "path": "d"}}),
        )
        .await["id"]
        .as_i64()
        .unwrap();
    let dir = t.dir.path().join("local/d");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("bad\r\nSet-Cookie: x=1"), b"x").unwrap();
    t.ok(
        "POST",
        "/api/mounts",
        json!({"path": "D", "connection_id": cid, "mode": "raw"}),
    )
    .await;
    assert!(t.names("D").await.is_empty());
}

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 30, 30]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

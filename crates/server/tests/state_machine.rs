//! A state-machine property test of the file tree, against the real server
//! (in process, real Postgres, MinIO in Docker).
//!
//! A reference model (below, written from the API's rules, not from the
//! server's code) keeps every storage location ("volume") as a map from path
//! to folder or file, and the user's mounts as a map from mount path to
//! volume. Random operations run against both: mkdir, upload (sizes from 0
//! and 1 to chunk boundaries and 20 MiB), overwrite, rename and move (within
//! a mount and across mounts: local to S3, raw to hashed), delete, nested
//! mounts added and removed, and GC. Each must succeed exactly when the
//! model says so, and after every step:
//!
//! * the listing of the whole tree equals the model;
//! * every file's bytes hash-match the model;
//! * no unreferenced blob is older than the GC grace period, and after a GC
//!   (grace 0) none is left, in the database or on its backend;
//! * every chunk a file references exists on its backend;
//! * replaying each volume's journal from empty rebuilds its tree;
//! * journal sequence numbers are gap-free, per volume and per user.
//!
//! Cases: `PROPTEST_CASES` (CI 256, nightly 10,000). Failing cases are kept
//! in `proptest-regressions/` and run first.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use common::*;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use serde_json::{Value, json};
use strata_server::background;
use strata_storage::Hash;
use strata_storage::chunker::{MAX_SIZE, SINGLE_BLOB_MAX};
use testcontainers_modules::minio::MinIO;
use testcontainers_modules::testcontainers::core::{ExecCommand, IntoContainerPort};
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

// ---------------------------------------------------------------- model

/// Where a mount's files live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Store {
    LocalRaw,
    LocalHashed,
    S3Raw,
    S3Hashed,
}

/// A file's content: `size` bytes made from `seed` (see `bytes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Content {
    seed: u64,
    size: usize,
}

impl Content {
    fn bytes(self) -> Vec<u8> {
        pseudo(self.size, self.seed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Dir,
    File(Content),
}

/// One storage location: relative path -> item ("" is its root, implicit).
#[derive(Clone, Debug, Default)]
struct Vol {
    tree: BTreeMap<String, Item>,
}

#[derive(Clone, Debug)]
struct Model {
    /// Volume 0 is the user's private root.
    vols: Vec<(Store, Vol)>,
    /// Mount path -> volume ("" -> 0, always).
    mounts: BTreeMap<String, usize>,
}

fn parent(p: &str) -> &str {
    p.rfind('/').map_or("", |i| &p[..i])
}

/// Is `p` at or below `dir`?
fn under(p: &str, dir: &str) -> bool {
    dir.is_empty() || p == dir || p.starts_with(&format!("{dir}/"))
}

fn rel_to(p: &str, dir: &str) -> String {
    if dir.is_empty() {
        p.to_string()
    } else if p == dir {
        String::new()
    } else {
        p[dir.len() + 1..].to_string()
    }
}

fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}/{b}"),
    }
}

impl Vol {
    fn get(&self, rel: &str) -> Option<&Item> {
        if rel.is_empty() {
            Some(&Item::Dir)
        } else {
            self.tree.get(rel)
        }
    }

    /// mkdir -p inside the volume: fails if a component is a file.
    fn mkdir_p(&mut self, rel: &str) -> Result<(), ()> {
        if rel.is_empty() {
            return Ok(());
        }
        let parts: Vec<&str> = rel.split('/').collect();
        for i in 1..=parts.len() {
            if let Some(Item::File(_)) = self.tree.get(&parts[..i].join("/")) {
                return Err(());
            }
        }
        for i in 1..=parts.len() {
            self.tree.insert(parts[..i].join("/"), Item::Dir);
        }
        Ok(())
    }

    fn subtree(&self, rel: &str) -> Vec<(String, Item)> {
        self.tree
            .iter()
            .filter(|(p, _)| under(p, rel) && !p.is_empty())
            .map(|(p, i)| (p.clone(), i.clone()))
            .collect()
    }

    fn remove_subtree(&mut self, rel: &str) {
        self.tree.retain(|p, _| !under(p, rel));
    }
}

impl Model {
    fn new() -> Self {
        Model {
            vols: vec![(Store::LocalHashed, Vol::default())],
            mounts: BTreeMap::from([(String::new(), 0)]),
        }
    }

    /// The deepest mount holding `p`, and `p` inside it.
    fn resolve(&self, p: &str) -> (String, usize, String) {
        let (mp, &v) = self
            .mounts
            .iter()
            .filter(|(m, _)| under(p, m))
            .max_by_key(|(m, _)| m.len())
            .unwrap();
        (mp.clone(), v, rel_to(p, mp))
    }

    /// What is at `p` in the user's tree.
    fn at(&self, p: &str) -> Option<Item> {
        let (_, v, rel) = self.resolve(p);
        self.vols[v].1.get(&rel).cloned()
    }

    fn mounts_below(&self, p: &str) -> bool {
        self.mounts
            .keys()
            .any(|m| !m.is_empty() && m != p && under(m, p))
    }

    /// The whole tree as the user sees it: path -> item (mount points are
    /// folders, and hide what their parent volume has at that path).
    fn view(&self) -> BTreeMap<String, Item> {
        let mut out = BTreeMap::new();
        for (mp, &v) in &self.mounts {
            if !mp.is_empty() {
                out.insert(mp.clone(), Item::Dir);
            }
            for (rel, item) in &self.vols[v].1.tree {
                let p = join(mp, rel);
                // Shadowed by a deeper mount?
                if self.resolve(&p).0 == *mp {
                    out.insert(p, item.clone());
                }
            }
        }
        // A mount point's parent folders are there too (made when mounting).
        out
    }

    fn apply(&mut self, op: &Op) -> Result<(), ()> {
        match op {
            Op::Mkdir(p) => {
                let (_, v, rel) = self.resolve(p);
                self.vols[v].1.mkdir_p(&rel)
            }
            Op::Upload(p, c) => {
                let (_, v, rel) = self.resolve(p);
                if rel.is_empty() {
                    return Err(());
                }
                let vol = &mut self.vols[v].1;
                if let Some(Item::Dir) = vol.tree.get(&rel) {
                    return Err(());
                }
                vol.mkdir_p(parent(&rel))?;
                vol.tree.insert(rel, Item::File(*c));
                Ok(())
            }
            Op::Delete(p) => {
                if p.is_empty() || self.mounts.contains_key(p) || self.mounts_below(p) {
                    return Err(());
                }
                let (_, v, rel) = self.resolve(p);
                let vol = &mut self.vols[v].1;
                if vol.get(&rel).is_none() {
                    return Err(());
                }
                vol.remove_subtree(&rel);
                Ok(())
            }
            Op::Move(from, to) => {
                if from.is_empty() || to.is_empty() || from == to || under(to, from) {
                    return Err(());
                }
                if self.mounts.contains_key(to) || self.at(to).is_some() {
                    return Err(());
                }
                let (_, sv, srel) = self.resolve(from);
                let Some(item) = self.vols[sv].1.get(&srel).cloned() else {
                    return Err(());
                };
                // The destination's parents (in whichever mount holds them).
                let (_, pv, prel) = self.resolve(parent(to));
                self.vols[pv].1.mkdir_p(&prel)?;
                let (_, dv, drel) = self.resolve(to);
                let mut moved = vec![(srel.clone(), item)];
                moved.extend(
                    self.vols[sv]
                        .1
                        .subtree(&srel)
                        .into_iter()
                        .filter(|(q, _)| q != &srel),
                );
                self.vols[sv].1.remove_subtree(&srel);
                for (q, i) in moved {
                    let nq = join(&drel, &rel_to(&q, &srel));
                    self.vols[dv].1.tree.insert(nq, i);
                }
                Ok(())
            }
            Op::Mount(p, store) => {
                // (Not over other mounts: they would be cut off.)
                if p.is_empty() || self.mounts.contains_key(p) || self.mounts_below(p) {
                    return Err(());
                }
                // Its parent folder is made (in the mount holding it).
                let (_, pv, prel) = self.resolve(parent(p));
                self.vols[pv].1.mkdir_p(&prel)?;
                self.vols.push((*store, Vol::default()));
                self.mounts.insert(p.clone(), self.vols.len() - 1);
                Ok(())
            }
            Op::Unmount(p) => {
                if p.is_empty() || !self.mounts.contains_key(p) || self.mounts_below(p) {
                    return Err(());
                }
                self.mounts.remove(p);
                Ok(())
            }
            Op::Gc => Ok(()),
        }
    }
}

// ---------------------------------------------------------------- operations

#[derive(Clone, Debug)]
enum Op {
    Mkdir(String),
    Upload(String, Content),
    Delete(String),
    Move(String, String),
    Mount(String, Store),
    Unmount(String),
    Gc,
}

const NAMES: &[&str] = &["a", "b", "c", "d"];

fn sizes() -> impl Strategy<Value = usize> {
    prop_oneof![
        8 => Just(0usize),
        8 => Just(1usize),
        8 => 2usize..4096,
        1 => Just(SINGLE_BLOB_MAX - 1),
        1 => Just(SINGLE_BLOB_MAX),
        1 => Just(SINGLE_BLOB_MAX + 1),
        1 => Just(MAX_SIZE + 1),
        1 => Just(20 * 1024 * 1024),
    ]
}

/// A path: mostly one already there (or below one), sometimes a new one.
fn path(m: &Model) -> BoxedStrategy<String> {
    let mut known: Vec<String> = m.view().into_keys().collect();
    known.extend(m.mounts.keys().filter(|k| !k.is_empty()).cloned());
    let fresh = prop::collection::vec(prop::sample::select(NAMES), 1..4).prop_map(|v| v.join("/"));
    if known.is_empty() {
        return fresh.boxed();
    }
    let known2 = known.clone();
    prop_oneof![
        3 => prop::sample::select(known),
        2 => (prop::sample::select(known2), prop::sample::select(NAMES)).prop_map(|(k, n)| format!("{k}/{n}")),
        2 => fresh,
    ]
    .boxed()
}

/// A move's destination: a new name in a folder there (often another
/// mount's), sometimes anything.
fn destination(m: &Model) -> BoxedStrategy<String> {
    let mut dirs: Vec<String> = m
        .view()
        .into_iter()
        .filter(|(_, i)| *i == Item::Dir)
        .map(|(p, _)| p)
        .collect();
    dirs.extend(m.mounts.keys().cloned());
    prop_oneof![
        4 => (prop::sample::select(dirs), prop::sample::select(NAMES)).prop_map(|(d, n)| join(&d, &format!("{n}2"))),
        1 => path(m),
    ]
    .boxed()
}

struct Tree;

impl ReferenceStateMachine for Tree {
    type State = Model;
    type Transition = Op;

    fn init_state() -> BoxedStrategy<Model> {
        Just(Model::new()).boxed()
    }

    fn transitions(m: &Model) -> BoxedStrategy<Op> {
        let stores = prop_oneof![
            Just(Store::LocalRaw),
            Just(Store::LocalHashed),
            Just(Store::S3Raw),
            Just(Store::S3Hashed)
        ];
        let mounts: Vec<String> = m.mounts.keys().filter(|k| !k.is_empty()).cloned().collect();
        let unmount = if mounts.is_empty() {
            Just(Op::Gc).boxed()
        } else {
            prop::sample::select(mounts).prop_map(Op::Unmount).boxed()
        };
        prop_oneof![
            3 => path(m).prop_map(Op::Mkdir),
            6 => (path(m), any::<u64>(), sizes()).prop_map(|(p, seed, size)| Op::Upload(p, Content { seed, size })),
            2 => path(m).prop_map(Op::Delete),
            5 => (path(m), destination(m)).prop_map(|(a, b)| Op::Move(a, b)),
            2 => (path(m), stores).prop_map(|(p, s)| Op::Mount(p, s)),
            1 => unmount,
            1 => Just(Op::Gc),
        ]
        .boxed()
    }

    fn preconditions(m: &Model, op: &Op) -> bool {
        match op {
            // Moving or deleting mount points, or folders holding them, and
            // mounting over a file, aren't what this test is about.
            Op::Move(from, _) => !m.mounts.contains_key(from) && !m.mounts_below(from),
            Op::Mount(p, _) => !matches!(m.at(p), Some(Item::File(_))),
            _ => true,
        }
    }

    fn apply(mut m: Model, op: &Op) -> Model {
        let _ = m.apply(op);
        m
    }
}

// ---------------------------------------------------------------- the server

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
fn rt() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    })
}

/// MinIO, once per test process (port, container).
static MINIO: OnceLock<Option<(u16, ContainerAsync<MinIO>)>> = OnceLock::new();
fn minio() -> Option<u16> {
    MINIO
        .get_or_init(|| {
            rt().block_on(async {
                let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
                let c = match MinIO::default()
                    .with_name("cgr.dev/chainguard/minio")
                    .with_tag("latest@sha256:304e0bbeb70a9aa0c40ff26d5a9c0a24296019c6d326ba8b75214fb73441d6e9")
                    .with_mapped_port(port, 9000.tcp())
                    .start()
                    .await
                {
                    Ok(c) => c,
                    Err(e) if std::env::var_os("CI").is_some() => panic!("can't start MinIO: {e:?}"),
                    Err(e) => {
                        eprintln!("skipping: can't start MinIO (Docker running?): {e}");
                        return None;
                    }
                };
                let mut r = c
                    .exec(ExecCommand::new(["sh", "-c", "mc alias set l http://127.0.0.1:9000 minioadmin minioadmin && mc mb l/prop"]))
                    .await
                    .unwrap();
                let _ = r.stdout_to_vec().await;
                Some((port, c))
            })
        })
        .as_ref()
        .map(|(p, _)| *p)
}

struct Server {
    t: TestApp,
    /// The model as of this step (to know whether a step should work).
    shadow: Model,
    local: i64,
    s3: i64,
    /// Fresh base folders, so no two mounts overlap.
    next: u32,
    case: u32,
}

impl Server {
    fn run<F: std::future::Future>(&self, f: F) -> F::Output {
        rt().block_on(f)
    }

    async fn status(&self, method: &str, uri: &str, body: Value) -> (bool, Value) {
        let (s, v) = self.t.json(method, uri, body).await;
        assert!(!s.is_server_error(), "{method} {uri}: {s} {v}");
        (s.is_success(), v)
    }

    async fn upload(&self, path: &str, data: &[u8]) -> bool {
        // The real protocol (chunks, then commit), tolerating refusals.
        let lens = strata_storage::chunker::chunk_lengths(data);
        let mut off = 0;
        let mut chunks = Vec::new();
        for l in &lens {
            chunks.push((
                blake3::hash(&data[off..off + l]).to_hex().to_string(),
                off,
                *l,
            ));
            off += l;
        }
        let spec: Vec<Value> = chunks
            .iter()
            .map(|(h, _, l)| json!({"hash": h, "size": l}))
            .collect();
        let (ok, v) = self
            .status(
                "POST",
                "/api/uploads",
                json!({"path": path, "size": data.len(), "chunks": spec}),
            )
            .await;
        if !ok {
            return false;
        }
        let id = v["id"].as_str().unwrap().to_string();
        for i in v["missing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap() as usize)
        {
            let (h, o, l) = &chunks[i];
            let (s, b, _) = self
                .t
                .raw(
                    "PUT",
                    &format!("/api/uploads/{id}/chunks/{h}"),
                    axum::body::Body::from(data[*o..o + l].to_vec()),
                    &[],
                )
                .await;
            assert!(s.is_success(), "chunk {s}: {}", String::from_utf8_lossy(&b));
        }
        self.status("POST", &format!("/api/uploads/{id}/commit"), json!({}))
            .await
            .0
    }

    async fn apply(&mut self, op: &Op) -> bool {
        match op {
            Op::Mkdir(p) => {
                self.status("POST", "/api/files/mkdir", json!({"path": p}))
                    .await
                    .0
            }
            Op::Upload(p, c) => self.upload(p, &c.bytes()).await,
            Op::Delete(p) => {
                self.status("POST", "/api/files/delete", json!({"paths": [p]}))
                    .await
                    .0
            }
            Op::Move(a, b) => {
                self.status("POST", "/api/files/move", json!({"from": a, "to": b}))
                    .await
                    .0
            }
            Op::Mount(p, store) => {
                self.next += 1;
                let base = format!("c{}-m{}", self.case, self.next);
                let (conn, mode) = match store {
                    Store::LocalRaw => (self.local, "raw"),
                    Store::LocalHashed => (self.local, "hashed"),
                    Store::S3Raw => (self.s3, "raw"),
                    Store::S3Hashed => (self.s3, "hashed"),
                };
                self.status(
                    "POST",
                    "/api/mounts",
                    json!({"path": p, "connection_id": conn, "base_path": base, "mode": mode}),
                )
                .await
                .0
            }
            Op::Unmount(p) => {
                let (_, list) = self.status("GET", "/api/mounts", json!(null)).await;
                let id = list
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["path"] == p.as_str())
                    .map(|m| m["id"].as_i64().unwrap());
                match id {
                    Some(id) => {
                        self.status("DELETE", &format!("/api/mounts/{id}"), json!(null))
                            .await
                            .0
                    }
                    None => false,
                }
            }
            Op::Gc => {
                background::gc(&self.t.st, 0).await.unwrap();
                true
            }
        }
    }

    /// The whole tree through the API: path -> (is_dir, size).
    async fn listing(&self) -> BTreeMap<String, (bool, i64)> {
        let mut out = BTreeMap::new();
        let mut todo = vec![String::new()];
        while let Some(dir) = todo.pop() {
            let (s, v) = self
                .t
                .get(&format!("/api/files/list?path={}", enc(&dir)))
                .await;
            assert!(s.is_success(), "list {dir:?}: {s} {v}");
            for e in v["entries"].as_array().unwrap() {
                let p = join(&dir, e["name"].as_str().unwrap());
                let is_dir = e["is_dir"].as_bool().unwrap();
                out.insert(p.clone(), (is_dir, e["size"].as_i64().unwrap()));
                if is_dir {
                    todo.push(p);
                }
            }
        }
        out
    }
}

struct Sut;

impl StateMachineTest for Sut {
    type SystemUnderTest = Option<Server>;
    type Reference = Tree;

    fn init_test(_: &Model) -> Option<Server> {
        let port = minio()?;
        rt().block_on(async {
            let t = app().await?;
            let local = t.ok("POST", "/api/connections", json!({"name": "disk", "config": {"kind": "local", "path": "data"}})).await;
            let s3 = t
                .ok(
                    "POST",
                    "/api/connections",
                    json!({"name": "s3", "config": {"kind": "s3", "bucket": "prop", "region": "us-east-1",
                        "endpoint": format!("http://127.0.0.1:{port}"), "access_key_id": "minioadmin", "secret_access_key": "minioadmin"}}),
                )
                .await;
            Some(Server {
                shadow: Model::new(),
                local: local["id"].as_i64().unwrap(),
                s3: s3["id"].as_i64().unwrap(),
                next: 0,
                case: rand::random(),
                t,
            })
        })
    }

    fn apply(sut: Option<Server>, m: &Model, op: Op) -> Option<Server> {
        let mut s = sut?;
        // Whether the step should work: the model, as of before it.
        let want = s.shadow.apply(&op).is_ok();
        let ok = rt().block_on(s.apply(&op));
        if std::env::var_os("STATE_MACHINE_TRACE").is_some() {
            eprintln!("STEP {} {:?}", if ok { "ok " } else { "err" }, op);
        }
        assert_eq!(
            ok,
            want,
            "{op:?}: the server {} but the model {}",
            if ok { "did it" } else { "refused" },
            if want { "allows it" } else { "refuses" }
        );
        let _ = m;
        Some(s)
    }

    fn check_invariants(sut: &Option<Server>, m: &Model) {
        let Some(s) = sut else { return };
        s.run(check(s, m));
    }
}

async fn check(s: &Server, m: &Model) {
    // 1. The listing is the model.
    let got = s.listing().await;
    let want = m.view();
    let got_kinds: BTreeMap<&String, bool> = got.iter().map(|(p, (d, _))| (p, *d)).collect();
    let want_kinds: BTreeMap<&String, bool> = want
        .iter()
        .map(|(p, i)| (p, matches!(i, Item::Dir)))
        .collect();
    assert_eq!(got_kinds, want_kinds, "listing differs from the model");
    // 2. Every file's bytes.
    for (p, item) in &want {
        if let Item::File(c) = item {
            assert_eq!(got[p].1, c.size as i64, "size of {p}");
            let bytes = s.t.download(p).await;
            assert!(
                blake3::hash(&bytes) == blake3::hash(&c.bytes()),
                "content of {p}"
            );
        }
    }
    let st = &s.t.st;
    // 3. No unreferenced blob older than the grace period (24 h here).
    let old: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM blobs b WHERE b.created_at < now() - interval '24 hours'
           AND NOT EXISTS (SELECT 1 FROM nodes n WHERE n.volume_id = b.volume_id AND n.chunks @> ARRAY[b.hash])
           AND NOT EXISTS (SELECT 1 FROM uploads u WHERE u.volume_id = b.volume_id AND u.chunks @> ARRAY[b.hash])",
    )
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(old, 0, "unreferenced blobs past the grace period");
    // 4. Every referenced chunk is on its backend (each hashed volume).
    let refs: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT DISTINCT n.volume_id, c FROM nodes n, unnest(n.chunks) c WHERE n.chunks IS NOT NULL",
    )
    .fetch_all(&st.db)
    .await
    .unwrap();
    for (vid, h) in refs {
        let store = store_of(s, vid).await;
        let h = Hash::from_slice(&h).unwrap();
        assert!(
            store.exists(&h).await.unwrap(),
            "chunk {} of volume {vid} missing on its backend",
            h.hex()
        );
    }
    // 5. Journal: gap-free sequence numbers, and a replay rebuilds each tree.
    journal_checks(s, m).await;
}

/// A hashed volume's blob store (through any mount of it, or a fresh one).
async fn store_of(s: &Server, vid: i64) -> strata_storage::hashed::HashedStore {
    let mounts = strata_server::mounts::user_mounts(&s.t.st, 1)
        .await
        .unwrap();
    if let Some(m) = mounts.iter().find(|m| m.volume_id == vid) {
        return strata_server::vfs::hashed_store(&s.t.st, m).await.unwrap();
    }
    // Unmounted: build the store from the volume row.
    let (cid, base): (i64, String) =
        sqlx::query_as("SELECT connection_id, base_path FROM volumes WHERE id = $1")
            .bind(vid)
            .fetch_one(&s.t.st.db)
            .await
            .unwrap();
    let op = s.t.st.operator(cid).await.unwrap();
    strata_storage::hashed::HashedStore::new(op, &base)
}

async fn journal_checks(s: &Server, m: &Model) {
    let db = &s.t.st.db;
    // Gap-free per scope.
    let vols: Vec<(i64, i64)> = sqlx::query_as("SELECT id, journal_seq FROM volumes")
        .fetch_all(db)
        .await
        .unwrap();
    for (vid, last) in &vols {
        let seqs: Vec<i64> =
            sqlx::query_scalar("SELECT seq FROM journal WHERE volume_id = $1 ORDER BY seq")
                .bind(vid)
                .fetch_all(db)
                .await
                .unwrap();
        assert_eq!(
            seqs,
            (1..=*last).collect::<Vec<_>>(),
            "journal seqs of volume {vid}"
        );
    }
    let users: Vec<(i64, i64)> = sqlx::query_as("SELECT id, journal_seq FROM users")
        .fetch_all(db)
        .await
        .unwrap();
    for (uid, last) in &users {
        let seqs: Vec<i64> =
            sqlx::query_scalar("SELECT seq FROM journal WHERE user_id = $1 ORDER BY seq")
                .bind(uid)
                .fetch_all(db)
                .await
                .unwrap();
        assert_eq!(
            seqs,
            (1..=*last).collect::<Vec<_>>(),
            "journal seqs of user {uid}"
        );
    }
    // Replay each mounted volume's journal from empty: its paths are the
    // model's.
    let mounts = strata_server::mounts::user_mounts(&s.t.st, 1)
        .await
        .unwrap();
    for (mp, &v) in &m.mounts {
        let Some(mount) = mounts.iter().find(|x| &x.path == mp) else {
            panic!("mount {mp:?} missing")
        };
        let rows: Vec<(String, String, Option<String>, bool)> = sqlx::query_as(
            "SELECT op, path, new_path, is_dir FROM journal WHERE volume_id = $1 ORDER BY seq",
        )
        .bind(mount.volume_id)
        .fetch_all(db)
        .await
        .unwrap();
        let mut tree: BTreeMap<String, bool> = BTreeMap::new();
        for (op, path, new_path, is_dir) in rows {
            match op.as_str() {
                "create" | "modify" => {
                    // (A create implies its parents.)
                    let mut p = path.as_str();
                    tree.insert(path.clone(), is_dir);
                    while !parent(p).is_empty() {
                        p = parent(p);
                        tree.entry(p.to_string()).or_insert(true);
                    }
                }
                "delete" => tree.retain(|q, _| !under(q, &path) || path.is_empty() && false),
                "move" => {
                    let to = new_path.unwrap();
                    let moved: Vec<(String, bool)> = tree
                        .iter()
                        .filter(|(q, _)| under(q, &path))
                        .map(|(q, d)| (q.clone(), *d))
                        .collect();
                    tree.retain(|q, _| !under(q, &path));
                    for (q, d) in moved {
                        tree.insert(join(&to, &rel_to(&q, &path)), d);
                    }
                    let mut p = to.as_str();
                    while !parent(p).is_empty() {
                        p = parent(p);
                        tree.entry(p.to_string()).or_insert(true);
                    }
                }
                other => panic!("unexpected op {other} in a volume journal"),
            }
        }
        let want: BTreeMap<String, bool> = m.vols[v]
            .1
            .tree
            .iter()
            .map(|(p, i)| (p.clone(), matches!(i, Item::Dir)))
            .collect();
        assert_eq!(tree, want, "journal replay of the volume mounted at {mp:?}");
    }
}

prop_state_machine! {
    #![proptest_config(Config {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|c| c.parse().ok()).unwrap_or(32),
        max_shrink_iters: 4096,
        failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct(
            "proptest-regressions/state_machine.txt",
        ))),
        // (Docker and Postgres per case: no forking.)
        fork: false,
        ..Config::default()
    })]

    #[test]
    fn file_tree_matches_the_model(sequential 1..24 => Sut);
}

/// After a GC with grace 0, no unreferenced blob is left in the database or
/// on its backend.
#[test]
fn gc_leaves_no_unreferenced_blobs() {
    let Some(_port) = minio() else { return };
    rt().block_on(async {
        let Some(t) = app().await else { return };
        t.upload("a.bin", &pseudo(SINGLE_BLOB_MAX + 1, 1)).await;
        t.upload("b.bin", &pseudo(1000, 2)).await;
        let gone: Vec<Vec<u8>> = sqlx::query_scalar("SELECT unnest(chunks) FROM nodes WHERE name = 'a.bin'")
            .fetch_all(&t.st.db)
            .await
            .unwrap();
        t.ok("POST", "/api/files/delete", json!({"paths": ["a.bin"]})).await;
        background::gc(&t.st, 0).await.unwrap();
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM blobs b WHERE NOT EXISTS
               (SELECT 1 FROM nodes n WHERE n.volume_id = b.volume_id AND n.chunks @> ARRAY[b.hash])",
        )
        .fetch_one(&t.st.db)
        .await
        .unwrap();
        assert_eq!(left, 0);
        let mounts = strata_server::mounts::user_mounts(&t.st, 1).await.unwrap();
        let store = strata_server::vfs::hashed_store(&t.st, &mounts[0]).await.unwrap();
        let kept: BTreeSet<Vec<u8>> = sqlx::query_scalar("SELECT unnest(chunks) FROM nodes")
            .fetch_all(&t.st.db)
            .await
            .unwrap()
            .into_iter()
            .collect();
        for h in gone.into_iter().filter(|h| !kept.contains(h)) {
            assert!(!store.exists(&Hash::from_slice(&h).unwrap()).await.unwrap(), "collected blob still on disk");
        }
    });
}

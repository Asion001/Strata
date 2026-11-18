//! Hashed store against a local folder, and against S3 (MinIO) when
//! `STRATA_TEST_S3_ENDPOINT` is set (see docs/DEVELOPMENT.md).
use bytes::Bytes;
use strata_storage::{ConnectionConfig, Hash, HashedStore};

async fn exercise(store: HashedStore) {
    let data = Bytes::from_static(b"hello strata");
    let h = Hash::of(&data);
    assert!(!store.exists(&h).await.unwrap());
    store.put(&h, data.clone()).await.unwrap();
    store.put(&h, data.clone()).await.unwrap(); // idempotent
    assert!(store.exists(&h).await.unwrap());
    assert_eq!(store.get(&h).await.unwrap(), data);
    assert_eq!(
        store.get_range(&h, 6..12).await.unwrap(),
        Bytes::from_static(b"strata")
    );
    assert!(store.verify(&h).await.unwrap());
    let x = store.blob_name(&h);
    assert!(
        store
            .blob_path(&h)
            .ends_with(&format!("blobs/{}/{}/{}", &x[..2], &x[2..4], x))
    );
    // wrong hash rejected
    assert!(store.put(&Hash::of(b"other"), data.clone()).await.is_err());
    store.delete(&h).await.unwrap();
    assert!(!store.exists(&h).await.unwrap());
}

#[tokio::test]
async fn local_hashed_store() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = ConnectionConfig::Local {
        path: "conn".into(),
    };
    cfg.create_local_root(dir.path()).unwrap();
    let op = cfg.operator(dir.path()).unwrap();
    exercise(HashedStore::new(op, "store")).await;
}

#[tokio::test]
async fn s3_hashed_store() {
    let Ok(endpoint) = std::env::var("STRATA_TEST_S3_ENDPOINT") else {
        eprintln!("skipping: STRATA_TEST_S3_ENDPOINT not set");
        return;
    };
    let cfg = ConnectionConfig::S3 {
        bucket: std::env::var("STRATA_TEST_S3_BUCKET").unwrap_or_else(|_| "strata-test".into()),
        region: None,
        endpoint: Some(endpoint),
        access_key_id: std::env::var("STRATA_TEST_S3_KEY").unwrap_or_else(|_| "minioadmin".into()),
        secret_access_key: std::env::var("STRATA_TEST_S3_SECRET")
            .unwrap_or_else(|_| "minioadmin".into()),
        root: Some(format!("/t{}", rand::random::<u32>())),
    };
    let op = cfg.operator(std::path::Path::new("/nonexistent")).unwrap();
    exercise(HashedStore::new(op, "")).await;
}

#[tokio::test]
async fn keyed_names_hide_content_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = ConnectionConfig::Local { path: "k".into() };
    cfg.create_local_root(dir.path()).unwrap();
    let op = cfg.operator(dir.path()).unwrap();
    let plain = HashedStore::new(op.clone(), "s");
    let keyed = HashedStore::new(op, "s").with_naming_key([7u8; 32]);
    let h = Hash::of(b"x");
    assert_ne!(plain.blob_path(&h), keyed.blob_path(&h));
    assert!(!keyed.blob_path(&h).contains(&h.hex()));
    exercise(keyed).await;
}

//! Content-addressed blob store used by HASHED mounts.
//!
//! Blobs are named by their BLAKE3 hash and sharded as `ab/cd/<hash>` under
//! `<base>/blobs/`. The folder tree lives only in PostgreSQL; this store only
//! knows about immutable blobs.

use std::fmt;
use std::ops::Range;
use std::str::FromStr;

use anyhow::{Result, bail};
use bytes::Bytes;
use opendal::{ErrorKind, Operator};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    pub fn of(data: &[u8]) -> Self {
        Hash(*blake3::hash(data).as_bytes())
    }
    pub fn hex(&self) -> String {
        hex::encode(self.0)
    }
    pub fn from_slice(s: &[u8]) -> Option<Self> {
        Some(Hash(s.try_into().ok()?))
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.hex())
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl FromStr for Hash {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let mut out = [0u8; 32];
        hex::decode_to_slice(s, &mut out)?;
        Ok(Hash(out))
    }
}

/// Identity of a whole file in a hashed mount: BLAKE3 over the chunk hashes.
/// Computed server-side so it never depends on what a client claims.
pub fn manifest_hash(chunks: &[Hash]) -> Hash {
    let mut h = blake3::Hasher::new();
    for c in chunks {
        h.update(&c.0);
    }
    Hash(*h.finalize().as_bytes())
}

#[derive(Clone)]
pub struct HashedStore {
    op: Operator,
    base: String,
    naming_key: Option<[u8; 32]>,
}

impl fmt::Debug for HashedStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HashedStore")
            .field("base", &self.base)
            .field("keyed", &self.naming_key.is_some())
            .finish()
    }
}

impl HashedStore {
    /// `base` is the folder inside the connection that holds the store.
    pub fn new(op: Operator, base: &str) -> Self {
        let base = base.trim_matches('/');
        let base = if base.is_empty() {
            String::new()
        } else {
            format!("{base}/")
        };
        Self {
            op,
            base,
            naming_key: None,
        }
    }

    /// Name blobs with BLAKE3 keyed mode instead of the bare content hash, so
    /// the backend cannot learn content hashes. Reserved for encryption at
    /// rest (per-connection key); stores without a key use plain hashes.
    pub fn with_naming_key(mut self, key: [u8; 32]) -> Self {
        self.naming_key = Some(key);
        self
    }

    /// The backend object name for a blob.
    pub fn blob_name(&self, h: &Hash) -> String {
        match &self.naming_key {
            Some(k) => blake3::keyed_hash(k, &h.0).to_hex().to_string(),
            None => h.hex(),
        }
    }

    pub fn blob_path(&self, h: &Hash) -> String {
        let x = self.blob_name(h);
        format!("{}blobs/{}/{}/{}", self.base, &x[..2], &x[2..4], x)
    }

    pub async fn exists(&self, h: &Hash) -> Result<bool> {
        match self.op.stat(&self.blob_path(h)).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Store a blob after checking its hash. Idempotent.
    pub async fn put(&self, h: &Hash, data: Bytes) -> Result<()> {
        if Hash::of(&data) != *h {
            bail!("blob content does not match hash {h}");
        }
        if self.exists(h).await? {
            return Ok(());
        }
        self.put_overwrite(h, data).await
    }

    /// Store a blob even if an object with its name exists (repairs an object
    /// a crashed GC may have half-deleted). Still checks the hash.
    pub async fn put_overwrite(&self, h: &Hash, data: Bytes) -> Result<()> {
        if Hash::of(&data) != *h {
            bail!("blob content does not match hash {h}");
        }
        let path = self.blob_path(h);
        if self.op.info().capability().rename {
            let tmp = format!(
                "{}tmp/{}.{}",
                self.base,
                self.blob_name(h),
                rand::random::<u32>()
            );
            self.op.write(&tmp, data).await?;
            if let Err(e) = self.op.rename(&tmp, &path).await {
                let _ = self.op.delete(&tmp).await;
                return Err(e.into());
            }
        } else {
            self.op.write(&path, data).await?;
        }
        Ok(())
    }

    pub async fn get(&self, h: &Hash) -> Result<Bytes> {
        Ok(self.op.read(&self.blob_path(h)).await?.to_bytes())
    }

    pub async fn get_range(&self, h: &Hash, range: Range<u64>) -> Result<Bytes> {
        Ok(self
            .op
            .read_with(&self.blob_path(h))
            .range(range)
            .await?
            .to_bytes())
    }

    /// Read a blob and check it against its name.
    pub async fn verify(&self, h: &Hash) -> Result<bool> {
        Ok(Hash::of(&self.get(h).await?) == *h)
    }

    pub async fn delete(&self, h: &Hash) -> Result<()> {
        self.op.delete(&self.blob_path(h)).await?;
        Ok(())
    }

    pub fn operator(&self) -> &Operator {
        &self.op
    }
}

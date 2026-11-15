//! Storage layer for Strata.
//!
//! * [`backend`]: connection configs (local folder, S3, FTP) turned into
//!   OpenDAL operators, plus capability detection.
//! * [`hashed`]: the content-addressed blob store used by HASHED mounts.
//! * [`chunker`]: FastCDC content-defined chunking (shared with the web client).
//! * [`crypto`]: encryption at rest for connection credentials.
//! * [`paths`]: path normalisation shared by the server.

pub mod backend;
pub mod chunker;
pub mod crypto;
pub mod hashed;
pub mod paths;

pub use backend::{Capabilities, ConnectionConfig, SnapshotKind};
pub use hashed::{Hash, HashedStore};

//! Storage layer for Strata.
//!
//! * [`backend`]: connection configs (local folder, S3, FTP) turned into
//!   OpenDAL operators.
//! * [`capabilities`]: what a connection supports (snapshots, rename, ...).
//! * [`probe`]: connection health probes.
//! * [`hashed`]: the content-addressed blob store used by HASHED mounts.
//! * [`chunker`]: FastCDC content-defined chunking (shared with the web client).
//! * [`crypto`]: encryption at rest for connection credentials.
//! * [`paths`]: path normalisation shared by the server.

pub mod backend;
pub mod capabilities;
pub mod chunker;
pub mod crypto;
pub mod hashed;
pub mod paths;
pub mod probe;
mod sigv4;

pub use backend::ConnectionConfig;
pub use capabilities::{Capabilities, SnapshotKind, Snapshots};
pub use hashed::{Hash, HashedStore};

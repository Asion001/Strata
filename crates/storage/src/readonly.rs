//! An OpenDAL layer that refuses every operation that could change data.
//!
//! Used for local folders Strata cannot write to (for example a folder
//! bind-mounted read-only into the container). Such a folder has no atomic
//! write folder, so a write that got through would land directly on the target
//! file. This layer makes sure none does, even if the filesystem later becomes
//! writable while the operator is still cached.

use std::sync::Arc;

use opendal::raw::*;
use opendal::{Capability, Error, ErrorKind, OperationContext, Result};

/// Message of every error this layer returns, so callers can tell a refused
/// write apart from other permission errors.
pub const READ_ONLY_MESSAGE: &str = "this connection is read-only";

fn denied(op: &'static str) -> Error {
    Error::new(ErrorKind::PermissionDenied, READ_ONLY_MESSAGE).with_operation(op)
}

/// Did this error come from [`ReadOnlyLayer`]?
pub fn is_read_only_refusal(e: &Error) -> bool {
    e.kind() == ErrorKind::PermissionDenied && e.message() == READ_ONLY_MESSAGE
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ReadOnlyLayer;

impl Layer for ReadOnlyLayer {
    fn apply_service(&self, inner: Servicer) -> Servicer {
        Arc::new(ReadOnlyService { inner })
    }
}

#[derive(Debug)]
pub struct ReadOnlyService {
    inner: Servicer,
}

impl Service for ReadOnlyService {
    type Reader = oio::Reader;
    type Writer = oio::Writer;
    type Lister = oio::Lister;
    type Deleter = oio::Deleter;
    type Copier = oio::Copier;
    type Composer = oio::Composer;

    fn info(&self) -> ServiceInfo {
        self.inner.info()
    }

    fn capability(&self) -> Capability {
        let mut c = self.inner.capability();
        c.write = false;
        c.write_can_append = false;
        c.write_can_multi = false;
        c.write_can_empty = false;
        c.create_dir = false;
        c.delete = false;
        c.copy = false;
        c.rename = false;
        c
    }

    async fn create_dir(
        &self,
        _: &OperationContext,
        _: &str,
        _: OpCreateDir,
    ) -> Result<RpCreateDir> {
        Err(denied("create_dir"))
    }

    async fn stat(&self, ctx: &OperationContext, path: &str, args: OpStat) -> Result<RpStat> {
        self.inner.stat(ctx, path, args).await
    }

    fn read(&self, ctx: &OperationContext, path: &str, args: OpRead) -> Result<Self::Reader> {
        self.inner.read(ctx, path, args)
    }

    fn write(&self, _: &OperationContext, _: &str, _: OpWrite) -> Result<Self::Writer> {
        Err(denied("write"))
    }

    fn delete(&self, _: &OperationContext) -> Result<Self::Deleter> {
        Err(denied("delete"))
    }

    fn list(&self, ctx: &OperationContext, path: &str, args: OpList) -> Result<Self::Lister> {
        self.inner.list(ctx, path, args)
    }

    fn copy(&self, _: &OperationContext, _: &str, _: &str, _: OpCopy) -> Result<Self::Copier> {
        Err(denied("copy"))
    }

    fn compose(&self, _: &OperationContext, _: &str, _: OpCompose) -> Result<Self::Composer> {
        Err(denied("compose"))
    }

    async fn rename(
        &self,
        _: &OperationContext,
        _: &str,
        _: &str,
        _: OpRename,
    ) -> Result<RpRename> {
        Err(denied("rename"))
    }

    async fn restore(&self, _: &OperationContext, _: &str, _: OpRestore) -> Result<RpRestore> {
        Err(denied("restore"))
    }

    async fn presign(&self, _: &OperationContext, _: &str, _: OpPresign) -> Result<RpPresign> {
        // A presigned URL could allow writes; local folders never presign anyway.
        Err(denied("presign"))
    }
}

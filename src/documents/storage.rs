//! Artifact storage — one of the few replaceable boundaries CLAUDE.md §0
//! sanctions a trait for. Development and single-node deployments use the
//! filesystem store below; a production object-storage adapter implements
//! the same two operations against an S3-compatible API (see
//! docs/OPERATIONS.md, "Document artifact storage") without touching the
//! worker or the download path.

use crate::shared::error::AppError;
use std::path::PathBuf;

pub(crate) trait DocumentStore: Clone + Send + Sync + 'static {
    /// Store `bytes` under a content-hash key; returns the opaque storage
    /// path recorded in `generated_document.storage_path`. Must be atomic:
    /// a reader never observes a partial artifact.
    async fn write(&self, hash_hex: &str, bytes: &[u8]) -> Result<String, AppError>;

    /// Fetch an artifact previously returned by `write`.
    async fn read(&self, storage_path: &str) -> Result<Vec<u8>, AppError>;
}

#[derive(Clone)]
pub struct FilesystemDocumentStore {
    root: PathBuf,
}

impl FilesystemDocumentStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl DocumentStore for FilesystemDocumentStore {
    async fn write(&self, hash_hex: &str, bytes: &[u8]) -> Result<String, AppError> {
        let prefix = &hash_hex[..2];
        let directory = self.root.join(prefix);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|_| AppError::Internal)?;

        let final_path = directory.join(format!("{hash_hex}.pdf"));
        let temporary_path = directory.join(format!(".{hash_hex}.tmp"));

        // tmp + rename: readers see the whole artifact or nothing.
        tokio::fs::write(&temporary_path, bytes)
            .await
            .map_err(|_| AppError::Internal)?;
        tokio::fs::rename(&temporary_path, &final_path)
            .await
            .map_err(|_| AppError::Internal)?;

        Ok(final_path.to_string_lossy().into_owned())
    }

    async fn read(&self, storage_path: &str) -> Result<Vec<u8>, AppError> {
        tokio::fs::read(storage_path).await.map_err(|error| {
            tracing::error!(?error, "stored document artifact could not be read");
            AppError::Internal
        })
    }
}

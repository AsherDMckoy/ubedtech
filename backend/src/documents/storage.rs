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
        // Defense in depth: storage paths come from the database, and this
        // store only ever wrote paths inside its root. A path that resolves
        // elsewhere (tampered row, migration bug) must never be served,
        // checksum or not.
        let root = tokio::fs::canonicalize(&self.root)
            .await
            .map_err(|_| AppError::Internal)?;
        let resolved = tokio::fs::canonicalize(storage_path)
            .await
            .map_err(|error| {
                tracing::error!(?error, "stored document artifact could not be resolved");
                AppError::Internal
            })?;
        if !resolved.starts_with(&root) {
            tracing::error!("artifact path escapes the document storage root");
            return Err(AppError::Integrity(
                "artifact path escapes the document storage root",
            ));
        }

        tokio::fs::read(&resolved).await.map_err(|error| {
            tracing::error!(?error, "stored document artifact could not be read");
            AppError::Internal
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[actix_web::test]
    async fn reads_are_confined_to_the_storage_root() {
        let root = std::env::temp_dir().join(format!("ubed-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let store = FilesystemDocumentStore::new(root.clone());

        let hash = "aa".repeat(32);
        let path = store.write(&hash, b"%PDF-artifact").await.unwrap();
        assert_eq!(store.read(&path).await.unwrap(), b"%PDF-artifact");

        // A file that exists but lives outside the root is refused, whether
        // addressed absolutely or by traversal from inside.
        let outside =
            std::env::temp_dir().join(format!("ubed-outside-{}.pdf", uuid::Uuid::new_v4()));
        std::fs::write(&outside, b"not yours").unwrap();
        let absolute = outside.to_string_lossy().into_owned();
        let traversal = format!(
            "{}/aa/../../{}",
            root.display(),
            outside.file_name().unwrap().to_string_lossy()
        );
        for hostile in [absolute, traversal] {
            let refused = store.read(&hostile).await;
            assert!(
                matches!(refused, Err(AppError::Integrity(_))),
                "{hostile} must be refused, got {refused:?}"
            );
        }
    }
}

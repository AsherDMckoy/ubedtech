use crate::shared::error::AppError;
use std::path::PathBuf;

#[derive(Clone)]
pub struct FilesystemDocumentStore {
    root: PathBuf,
}

impl FilesystemDocumentStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn write(&self, hash_hex: &str, bytes: &[u8]) -> Result<String, AppError> {
        let prefix = &hash_hex[..2];
        let directory = self.root.join(prefix);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|_| AppError::Internal)?;

        let final_path = directory.join(format!("{hash_hex}.pdf"));
        let temporary_path = directory.join(format!(".{hash_hex}.tmp"));

        tokio::fs::write(&temporary_path, bytes)
            .await
            .map_err(|_| AppError::Internal)?;
        tokio::fs::rename(&temporary_path, &final_path)
            .await
            .map_err(|_| AppError::Internal)?;

        Ok(final_path.to_string_lossy().into_owned())
    }
}

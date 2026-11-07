//! Windows stand-in for `catalog_review.rs`.
//!
//! The Linux version reviews and removes installed catalog directories through
//! pinned directory handles (`/proc/self/fd`, `O_PATH`, owner/mode checks).
//! Until that is ported, Windows exposes the same API and refuses the operations.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogReviewedFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub matches_catalog: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogDirectoryReview {
    pub directory: String,
    pub sha256: String,
    pub bytes: u64,
    pub files: Vec<CatalogReviewedFile>,
    pub missing_files: Vec<String>,
}

pub fn review_catalog_directory(_config_dir: &Path, _name: &str) -> Result<CatalogDirectoryReview> {
    bail!("reviewing catalog template directories is not supported on Windows yet")
}

pub fn remove_catalog_directory(
    _config_dir: &Path,
    _name: &str,
    _sha256: &str,
    _authorize: impl FnOnce() -> Result<()>,
) -> Result<()> {
    bail!("removing catalog template directories is not supported on Windows yet")
}

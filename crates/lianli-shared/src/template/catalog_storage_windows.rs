//! Windows stand-in for `catalog_storage.rs`.
//!
//! Catalog installs are refused on Windows for now (see `catalog::install_template`),
//! so there is never installed catalog storage to report.

use anyhow::Result;
use std::path::Path;

pub(super) const RECEIPT_NAME: &str = ".lianli-catalog.json";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CatalogStorageEntry {
    pub directory: String,
    pub bytes: u64,
    pub ownership_verified: bool,
    pub issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_ui: Option<crate::ui_message::UiMessage>,
    #[serde(default)]
    pub saved_references: Vec<String>,
    #[serde(default)]
    pub saved_references_checked: bool,
    #[serde(default)]
    pub saved_reference_count: u32,
    #[serde(default)]
    pub runtime_referenced: bool,
    #[serde(default)]
    pub runtime_references_checked: bool,
}

pub fn inspect_storage(_config_dir: &Path) -> Result<Vec<CatalogStorageEntry>> {
    Ok(Vec::new())
}

use std::{env, fs, fs::File, path::PathBuf};

use anyhow::{Context, Result};
use tempfile::{tempfile, tempfile_in};

/// Create an anonymous native scratch file, honoring the same optional root
/// used by the ComfyUI transport. The caller owns the handle; the operating
/// system removes the file automatically when that handle is dropped.
pub(crate) fn temporary_file(purpose: &str) -> Result<File> {
    let Some(root) = env::var_os("SEAMINGLY_EPIC_TEMP") else {
        return tempfile().with_context(|| format!("could not create temporary {purpose}"));
    };
    let root = PathBuf::from(root);
    fs::create_dir_all(&root).with_context(|| {
        format!(
            "could not create SEAMINGLY_EPIC_TEMP directory: {}",
            root.display()
        )
    })?;
    tempfile_in(&root)
        .with_context(|| format!("could not create temporary {purpose} in {}", root.display()))
}

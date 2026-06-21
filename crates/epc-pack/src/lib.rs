#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackRequest {
    pub source_dir: PathBuf,
    pub output_file: PathBuf,
}

impl PackRequest {
    pub fn new(source_dir: impl Into<PathBuf>, output_file: impl Into<PathBuf>) -> Self {
        Self {
            source_dir: source_dir.into(),
            output_file: output_file.into(),
        }
    }

    pub fn source_dir(&self) -> &Path {
        &self.source_dir
    }

    pub fn output_file(&self) -> &Path {
        &self.output_file
    }
}

use crate::skill_package::{LoadedPackageDescriptor, SkillPackageDescriptor};
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

pub(crate) struct SecurePackageSnapshot {
    pub descriptor: LoadedPackageDescriptor,
    pub content_hash: String,
    pub runtime_manifest: Option<Vec<u8>>,
    pub instructions_file: Option<Vec<u8>>,
    pub file_paths: BTreeSet<PathBuf>,
}

pub(crate) struct SecureTreeSnapshot {
    pub content_hash: String,
    pub(crate) descriptor_bytes: Option<Vec<u8>>,
    pub(crate) runtime_manifest: Option<Vec<u8>>,
    pub(crate) instructions_file: Option<Vec<u8>>,
    pub(crate) file_paths: BTreeSet<PathBuf>,
}

impl SecureTreeSnapshot {
    pub(crate) fn load_descriptor(&self, root: &Path) -> anyhow::Result<LoadedPackageDescriptor> {
        SkillPackageDescriptor::load_from_file_bytes(
            root,
            self.descriptor_bytes.clone(),
            self.runtime_manifest.clone(),
            self.instructions_file.clone(),
        )
    }
}

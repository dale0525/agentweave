use crate::skill_package::{LoadedPackageDescriptor, SkillPackageDescriptor};
use std::path::Path;

pub(crate) struct SecurePackageSnapshot {
    pub descriptor: LoadedPackageDescriptor,
    pub content_hash: String,
    pub runtime_manifest: Option<Vec<u8>>,
    pub instructions_file: Option<Vec<u8>>,
}

pub(crate) struct SecureTreeSnapshot {
    pub content_hash: String,
    pub(crate) descriptor_bytes: Option<Vec<u8>>,
    pub(crate) runtime_manifest: Option<Vec<u8>>,
    pub(crate) instructions_file: Option<Vec<u8>>,
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

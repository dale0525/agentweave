use crate::skill_package::SkillPackageId;
use crate::skill_store_fs::PackageLimits;
use chrono::{DateTime, Utc};
use std::path::PathBuf;

pub const DEFAULT_MAX_SKILL_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_MAX_SKILL_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_SKILL_ENTRIES: u64 = 4096;
pub const DEFAULT_MAX_SKILL_FILES: u64 = 2048;
pub const DEFAULT_MAX_SKILL_DIRECTORIES: u64 = 2048;
pub const DEFAULT_MAX_SKILL_DEPTH: u64 = 32;
pub const DEFAULT_MAX_SKILL_RELATIVE_PATH_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillStoreLimits {
    pub max_file_bytes: u64,
    pub max_package_bytes: u64,
    pub max_entries: u64,
    pub max_files: u64,
    pub max_directories: u64,
    pub max_depth: u64,
    pub max_relative_path_bytes: u64,
}

impl Default for SkillStoreLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_SKILL_FILE_BYTES,
            max_package_bytes: DEFAULT_MAX_SKILL_PACKAGE_BYTES,
            max_entries: DEFAULT_MAX_SKILL_ENTRIES,
            max_files: DEFAULT_MAX_SKILL_FILES,
            max_directories: DEFAULT_MAX_SKILL_DIRECTORIES,
            max_depth: DEFAULT_MAX_SKILL_DEPTH,
            max_relative_path_bytes: DEFAULT_MAX_SKILL_RELATIVE_PATH_BYTES,
        }
    }
}

impl SkillStoreLimits {
    pub(crate) fn package_limits(self) -> PackageLimits {
        PackageLimits {
            max_file_bytes: self.max_file_bytes,
            max_package_bytes: self.max_package_bytes,
            max_entries: self.max_entries,
            max_files: self.max_files,
            max_directories: self.max_directories,
            max_depth: self.max_depth,
            max_relative_path_bytes: self.max_relative_path_bytes,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoredSkillRevision {
    pub revision_id: String,
    pub package_id: SkillPackageId,
    pub path: PathBuf,
    pub content_hash: String,
    pub maintenance_issues: Vec<SkillStoreMaintenanceIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagingSkillFile {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillStoreMaintenanceIssue {
    pub revision_id: String,
    pub operation: String,
    pub path: PathBuf,
    pub message: String,
    pub recorded_at: DateTime<Utc>,
}

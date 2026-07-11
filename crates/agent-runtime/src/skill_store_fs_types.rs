use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub(crate) struct PackageLimits {
    pub max_file_bytes: u64,
    pub max_package_bytes: u64,
    pub max_entries: u64,
    pub max_files: u64,
    pub max_directories: u64,
    pub max_depth: u64,
    pub max_relative_path_bytes: u64,
}

pub(crate) struct PackageEntries {
    pub root_mode: u32,
    pub directories: Vec<PackageDirectory>,
    pub files: Vec<PackageFile>,
}

pub(crate) struct PackageDirectory {
    pub relative: PathBuf,
    pub mode: u32,
}

pub(crate) struct PackageFile {
    pub relative: PathBuf,
    pub expected_bytes: u64,
    pub mode: u32,
}

pub(crate) struct StoredFileContents {
    pub bytes: Vec<u8>,
    pub mode: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AtomicReplaceCommitState {
    NotCommitted,
    Committed,
}

pub(crate) struct AtomicReplaceFailure {
    pub state: AtomicReplaceCommitState,
    pub temp_path: Option<PathBuf>,
    pub error: anyhow::Error,
}

impl AtomicReplaceFailure {
    pub(crate) fn into_error(self) -> anyhow::Error {
        self.error.context(format!(
            "atomic staging replace ended in {:?} state",
            self.state
        ))
    }
}

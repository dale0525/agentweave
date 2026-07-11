use crate::skill_source::canonical_relative_path;
use crate::skill_store_execution::is_portable_absolute_path;
use std::path::{Path, PathBuf};

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ManifestEntryArgKind {
    Opaque,
    PackagedRelative(PathBuf),
    UnsafeRelative,
}

pub(crate) fn classify_manifest_entry_arg(arg: &str) -> ManifestEntryArgKind {
    if arg.trim().is_empty() || arg.starts_with('-') {
        return ManifestEntryArgKind::Opaque;
    }
    if is_portable_absolute_path(arg) || has_windows_anchored_prefix(arg) {
        return ManifestEntryArgKind::Opaque;
    }
    if !looks_like_resource(arg) {
        return ManifestEntryArgKind::Opaque;
    }
    if arg.contains('\0') {
        return ManifestEntryArgKind::UnsafeRelative;
    }

    let mut normalized = PathBuf::new();
    for component in arg.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => return ManifestEntryArgKind::UnsafeRelative,
            component => normalized.push(component),
        }
    }
    if normalized.as_os_str().is_empty() || canonical_relative_path(&normalized).is_err() {
        return ManifestEntryArgKind::UnsafeRelative;
    }

    ManifestEntryArgKind::PackagedRelative(normalized)
}

fn has_windows_anchored_prefix(arg: &str) -> bool {
    let bytes = arg.as_bytes();
    arg.starts_with('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn looks_like_resource(arg: &str) -> bool {
    arg.contains(['/', '\\']) || Path::new(arg).extension().is_some()
}

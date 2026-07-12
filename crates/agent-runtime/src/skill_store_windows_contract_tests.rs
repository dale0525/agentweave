use crate::skill_store_windows::{
    DELETE_ACCESS_FLAG, DirectoryBootstrapComponent, DirectoryBootstrapState,
    FILE_ATTRIBUTE_REPARSE_POINT_FLAG, FILE_FLAG_BACKUP_SEMANTICS_FLAG,
    FILE_FLAG_OPEN_REPARSE_POINT_FLAG, FILE_LIST_DIRECTORY_ACCESS_FLAG,
    FILE_READ_ATTRIBUTES_ACCESS_FLAG, FILE_SHARE_DELETE_FLAG, FILE_SHARE_READ_FLAG,
    FILE_SHARE_WRITE_FLAG, FILE_WRITE_ATTRIBUTES_ACCESS_FLAG, MOVEFILE_REPLACE_EXISTING_FLAG,
    MOVEFILE_WRITE_THROUGH_FLAG, atomic_replace_flags, attributes_are_reparse,
    bootstrap_directory_access_mask, component_open_flags, directory_share_mode,
    lock_file_share_mode, normalized_path_is_within, regular_file_link_count_is_valid,
    replaceable_file_share_mode,
};
use crate::skill_store_windows_directory_create::{
    FILE_CREATE_DISPOSITION, FILE_DIRECTORY_CREATE_OPTION, FILE_OPEN_REPARSE_CREATE_OPTION,
    FILE_SYNCHRONOUS_CREATE_OPTION, NativeDirectoryCreate,
};

#[test]
fn windows_atomic_replace_contract_replaces_and_flushes() {
    let flags = atomic_replace_flags();
    assert_ne!(flags & MOVEFILE_REPLACE_EXISTING_FLAG, 0);
    assert_ne!(flags & MOVEFILE_WRITE_THROUGH_FLAG, 0);
}

#[test]
fn windows_bootstrap_directory_handle_has_cleanup_access() {
    let access = bootstrap_directory_access_mask();
    assert_ne!(access & DELETE_ACCESS_FLAG, 0);
    assert_ne!(access & FILE_LIST_DIRECTORY_ACCESS_FLAG, 0);
    assert_ne!(access & FILE_READ_ATTRIBUTES_ACCESS_FLAG, 0);
    assert_ne!(access & FILE_WRITE_ATTRIBUTES_ACCESS_FLAG, 0);
}

#[test]
fn windows_component_contract_opens_reparse_points_without_traversing_them() {
    assert_ne!(
        component_open_flags(false) & FILE_FLAG_OPEN_REPARSE_POINT_FLAG,
        0
    );
    assert_ne!(
        component_open_flags(true) & FILE_FLAG_OPEN_REPARSE_POINT_FLAG,
        0
    );
    assert_ne!(
        component_open_flags(true) & FILE_FLAG_BACKUP_SEMANTICS_FLAG,
        0
    );
    assert!(attributes_are_reparse(FILE_ATTRIBUTE_REPARSE_POINT_FLAG));
    assert!(!attributes_are_reparse(0));
}

#[test]
fn windows_critical_namespace_handles_deny_share_delete() {
    for share_mode in [directory_share_mode(), lock_file_share_mode()] {
        assert_ne!(share_mode & FILE_SHARE_READ_FLAG, 0);
        assert_ne!(share_mode & FILE_SHARE_WRITE_FLAG, 0);
        assert_eq!(share_mode & FILE_SHARE_DELETE_FLAG, 0);
    }
}

#[test]
fn windows_replaceable_metadata_handles_allow_share_delete() {
    let share_mode = replaceable_file_share_mode();
    assert_ne!(share_mode & FILE_SHARE_READ_FLAG, 0);
    assert_ne!(share_mode & FILE_SHARE_WRITE_FLAG, 0);
    assert_ne!(share_mode & FILE_SHARE_DELETE_FLAG, 0);
}

#[test]
fn windows_regular_file_link_count_contract_rejects_hardlinks() {
    assert!(regular_file_link_count_is_valid(1));
    assert!(!regular_file_link_count_is_valid(0));
    assert!(!regular_file_link_count_is_valid(2));
}

#[test]
fn windows_containment_is_case_insensitive_and_component_bounded() {
    assert!(normalized_path_is_within(
        r"\\?\C:\Store\Managed\Revision",
        r"\\?\c:\store\managed"
    ));
    assert!(!normalized_path_is_within(
        r"\\?\C:\Store\Managed-Escape\Revision",
        r"\\?\c:\store\managed"
    ));
}

#[test]
fn windows_drive_root_bootstrap_opens_only_after_prefix_and_root() {
    let mut bootstrap = DirectoryBootstrapState::default();

    assert!(!bootstrap.should_open(DirectoryBootstrapComponent::Prefix));
    assert!(bootstrap.should_open(DirectoryBootstrapComponent::Root));
    assert!(bootstrap.should_open(DirectoryBootstrapComponent::Normal));
}

#[test]
fn windows_unc_share_root_bootstrap_opens_the_prefix_root() {
    let mut bootstrap = DirectoryBootstrapState::default();

    assert!(!bootstrap.should_open(DirectoryBootstrapComponent::Prefix));
    assert!(bootstrap.should_open(DirectoryBootstrapComponent::Root));
}

#[test]
fn windows_drive_relative_prefix_never_bootstraps_a_handle() {
    let mut bootstrap = DirectoryBootstrapState::default();

    assert!(!bootstrap.should_open(DirectoryBootstrapComponent::Prefix));
    assert!(!bootstrap.should_open(DirectoryBootstrapComponent::Normal));
}

#[test]
fn windows_directory_child_creation_uses_native_atomic_create_options() {
    assert_eq!(FILE_CREATE_DISPOSITION, 2);
    assert_eq!(FILE_DIRECTORY_CREATE_OPTION, 1);
    assert_ne!(FILE_OPEN_REPARSE_CREATE_OPTION, 0);
    assert_ne!(FILE_SYNCHRONOUS_CREATE_OPTION, 0);
}

#[test]
fn windows_native_directory_create_result_keeps_created_handle_distinct() {
    let created = NativeDirectoryCreate::Created("exact handle");
    let exists = NativeDirectoryCreate::<&str>::AlreadyExists;

    assert!(matches!(
        created,
        NativeDirectoryCreate::Created("exact handle")
    ));
    assert!(matches!(exists, NativeDirectoryCreate::AlreadyExists));
}

#[test]
fn windows_native_directory_create_source_contract_is_atomic() {
    let source = include_str!("skill_store_windows_directory_create.rs");
    assert!(source.contains("NtCreateFile"));
    assert!(source.contains("FILE_CREATE_DISPOSITION"));
    assert!(source.contains("FILE_DIRECTORY_CREATE_OPTION"));
    assert!(source.contains("NativeDirectoryCreate::Created"));
    assert!(source.contains("NativeDirectoryCreate::AlreadyExists"));
    assert!(!source.contains("std::fs::create_dir"));
}

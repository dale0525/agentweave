use crate::skill_store_windows::{
    DirectoryBootstrapComponent, DirectoryBootstrapState, FILE_ATTRIBUTE_REPARSE_POINT_FLAG,
    FILE_FLAG_BACKUP_SEMANTICS_FLAG, FILE_FLAG_OPEN_REPARSE_POINT_FLAG, FILE_SHARE_DELETE_FLAG,
    FILE_SHARE_READ_FLAG, FILE_SHARE_WRITE_FLAG, MOVEFILE_REPLACE_EXISTING_FLAG,
    MOVEFILE_WRITE_THROUGH_FLAG, atomic_replace_flags, attributes_are_reparse,
    component_open_flags, directory_share_mode, finish_directory_child_creation,
    lock_file_share_mode, normalized_path_is_within,
};

#[test]
fn windows_atomic_replace_contract_replaces_and_flushes() {
    let flags = atomic_replace_flags();
    assert_ne!(flags & MOVEFILE_REPLACE_EXISTING_FLAG, 0);
    assert_ne!(flags & MOVEFILE_WRITE_THROUGH_FLAG, 0);
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
fn windows_directory_child_creation_opens_after_a_concurrent_winner() {
    let mut opened = false;
    let child = finish_directory_child_creation(
        Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists)),
        || {
            opened = true;
            Ok("opened child")
        },
    )
    .unwrap();

    assert!(opened);
    assert_eq!(child, "opened child");
}

#[test]
fn windows_directory_child_creation_does_not_hide_other_create_errors() {
    let error = finish_directory_child_creation::<(), _>(
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        || panic!("open must not run after a non-concurrent create failure"),
    )
    .unwrap_err();

    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

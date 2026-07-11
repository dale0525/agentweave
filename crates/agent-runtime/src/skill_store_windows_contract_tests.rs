use crate::skill_store_windows::{
    MOVEFILE_REPLACE_EXISTING_FLAG, MOVEFILE_WRITE_THROUGH_FLAG, atomic_replace_flags,
    normalized_path_is_within,
};

#[test]
fn windows_atomic_replace_contract_replaces_and_flushes() {
    let flags = atomic_replace_flags();
    assert_ne!(flags & MOVEFILE_REPLACE_EXISTING_FLAG, 0);
    assert_ne!(flags & MOVEFILE_WRITE_THROUGH_FLAG, 0);
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

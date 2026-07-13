#![cfg(windows)]

use windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION;

fn metadata_has_one_link(metadata: &BY_HANDLE_FILE_INFORMATION) -> bool {
    Some(metadata.nNumberOfLinks) == Some(1)
}

#[test]
fn metadata_link_count_contract_uses_the_optional_windows_api() {
    let check: fn(&BY_HANDLE_FILE_INFORMATION) -> bool = metadata_has_one_link;
    let _ = check;
}

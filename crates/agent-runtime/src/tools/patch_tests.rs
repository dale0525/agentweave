use super::*;
use std::{path::PathBuf, time::Instant};
#[tokio::test]
async fn apply_patch_adds_file_inside_workspace() {
    let root = unique_test_dir("patch-add");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({
            "patch": "*** Begin Patch\n*** Add File: notes/example.txt\n+hello\n+world\n*** End Patch\n"
        }),
        Instant::now(),
    )
    .await;
    assert!(result.ok, "{result:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("notes/example.txt")).unwrap(),
        "hello\nworld\n"
    );
    assert_eq!(
        result.data.unwrap()["changed_files"],
        json!([{ "path": "notes/example.txt", "action": "add", "added_lines": 2, "removed_lines": 0 }])
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn add_file_without_patch_trailing_newline_writes_trailing_newline() {
    let root = unique_test_dir("patch-add-no-final-newline");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** Add File: note.txt\n+hello\n*** End Patch" }),
        Instant::now(),
    )
    .await;
    assert!(result.ok, "{result:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "hello\n"
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn apply_patch_updates_file_with_context_hunk() {
    let root = unique_test_dir("patch-update");
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("notes/example.txt"), "alpha\nold\nomega\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({
            "patch": "*** Begin Patch\n*** Update File: notes/example.txt\n@@\n alpha\n-old\n+new\n omega\n*** End Patch\n"
        }),
        Instant::now(),
    )
    .await;
    assert!(result.ok, "{result:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("notes/example.txt")).unwrap(),
        "alpha\nnew\nomega\n"
    );
    assert_eq!(
        result.data.unwrap()["changed_files"],
        json!([{ "path": "notes/example.txt", "action": "update", "added_lines": 1, "removed_lines": 1 }])
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn apply_patch_deletes_file_inside_workspace() {
    let root = unique_test_dir("patch-delete");
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("notes/remove.txt"), "bye\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({
            "patch": "*** Begin Patch\n*** Delete File: notes/remove.txt\n*** End Patch\n"
        }),
        Instant::now(),
    )
    .await;
    assert!(result.ok, "{result:?}");
    assert!(!root.join("notes/remove.txt").exists());
    assert_eq!(
        result.data.unwrap()["changed_files"],
        json!([{ "path": "notes/remove.txt", "action": "delete", "added_lines": 0, "removed_lines": 1 }])
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn apply_patch_rejects_outside_workspace_add() {
    let root = unique_test_dir("patch-outside-add");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({
            "patch": "*** Begin Patch\n*** Add File: ../escape.txt\n+nope\n*** End Patch\n"
        }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "path_escape");
    assert!(!root.parent().unwrap().join("escape.txt").exists());
    remove_test_dir(root);
}
#[tokio::test]
async fn apply_patch_rejects_hunk_that_does_not_match() {
    let root = unique_test_dir("patch-hunk-mismatch");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("example.txt");
    std::fs::write(&path, "alpha\nactual\nomega\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({
            "patch": "*** Begin Patch\n*** Update File: example.txt\n@@\n alpha\n-missing\n+new\n omega\n*** End Patch\n"
        }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "patch_apply_failed");
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "alpha\nactual\nomega\n"
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn add_existing_returns_path_exists() {
    let root = unique_test_dir("patch-add-existing");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("example.txt"), "already\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({
            "patch": "*** Begin Patch\n*** Add File: example.txt\n+new\n*** End Patch\n"
        }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "path_exists");
    assert_eq!(
        std::fs::read_to_string(root.join("example.txt")).unwrap(),
        "already\n"
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn update_non_utf8_returns_path_not_text() {
    let root = unique_test_dir("patch-non-utf8");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("binary.bin"), [0xff, 0xfe, b'\n']).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({
            "patch": "*** Begin Patch\n*** Update File: binary.bin\n@@\n-old\n+new\n*** End Patch\n"
        }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "path_not_text");
    remove_test_dir(root);
}
#[tokio::test]
async fn invalid_patch_returns_invalid_patch() {
    let root = unique_test_dir("patch-invalid");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** Update File: missing-hunk.txt\n*** End Patch\n" }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "invalid_patch");
    remove_test_dir(root);
}
#[tokio::test]
async fn malformed_update_hunk_with_empty_line_returns_invalid_patch() {
    let root = unique_test_dir("patch-empty-hunk-line");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("example.txt"), "alpha\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** Update File: example.txt\n@@\n\n*** End Patch\n" }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "invalid_patch");
    remove_test_dir(root);
}
#[tokio::test]
async fn malformed_update_hunk_with_non_ascii_leading_char_returns_invalid_patch() {
    let root = unique_test_dir("patch-non-ascii-hunk-line");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("example.txt"), "alpha\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** Update File: example.txt\n@@\n你bad\n*** End Patch\n" }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "invalid_patch");
    remove_test_dir(root);
}
#[tokio::test]
async fn apply_patch_rejects_unknown_arguments() {
    let root = unique_test_dir("patch-unknown-args");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** End Patch\n", "extra": true }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "invalid_arguments");
    remove_test_dir(root);
}
#[tokio::test]
async fn empty_patch_returns_invalid_patch() {
    let root = unique_test_dir("patch-empty");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** End Patch\n" }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "invalid_patch");
    remove_test_dir(root);
}
#[tokio::test]
async fn update_hunk_with_only_added_lines_returns_patch_apply_failed() {
    let root = unique_test_dir("patch-no-anchor");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("example.txt"), "alpha\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** Update File: example.txt\n@@\n+inserted\n*** End Patch\n" }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "patch_apply_failed");
    assert_eq!(
        std::fs::read_to_string(root.join("example.txt")).unwrap(),
        "alpha\n"
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn repeated_updates_same_file_are_rejected_or_applied_sequentially_without_losing_first_change()
 {
    let root = unique_test_dir("patch-repeat-update");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("example.txt"), "one\ntwo\nthree\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** Update File: example.txt\n@@\n-one\n+ONE\n*** Update File: example.txt\n@@\n-three\n+THREE\n*** End Patch\n" }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "patch_apply_failed");
    assert_eq!(
        std::fs::read_to_string(root.join("example.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn ambiguous_duplicate_context_returns_patch_apply_failed() {
    let root = unique_test_dir("patch-ambiguous-context");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("example.txt"), "same\nold\nsame\nold\n").unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** Update File: example.txt\n@@\n same\n-old\n+new\n*** End Patch\n" }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "patch_apply_failed");
    assert_eq!(
        std::fs::read_to_string(root.join("example.txt")).unwrap(),
        "same\nold\nsame\nold\n"
    );
    remove_test_dir(root);
}
#[tokio::test]
async fn multi_add_parent_child_conflict_rejected_without_partial_write() {
    let root = unique_test_dir("patch-parent-child");
    std::fs::create_dir_all(&root).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({ "patch": "*** Begin Patch\n*** Add File: dir\n+file\n*** Add File: dir/file.txt\n+nested\n*** End Patch\n" }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "patch_apply_failed");
    assert!(!root.join("dir").exists());
    remove_test_dir(root);
}
#[cfg(unix)]
#[tokio::test]
async fn add_through_symlink_parent_escape_rejected_and_outside_target_not_written() {
    let root = unique_test_dir("patch-symlink-root");
    let outside = unique_test_dir("patch-symlink-outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let config = RuntimeConfig::workspace_write(&root, &root);
    let result = execute(
        &config,
        "call-1",
        json!({
            "patch": "*** Begin Patch\n*** Add File: link/escape.txt\n+nope\n*** End Patch\n"
        }),
        Instant::now(),
    )
    .await;
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "path_escape");
    assert!(!outside.join("escape.txt").exists());
    remove_test_dir(root);
    remove_test_dir(outside);
}
fn unique_test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("agentweave-{name}-{}", uuid::Uuid::new_v4()))
}

fn remove_test_dir(path: PathBuf) {
    if path.exists() {
        std::fs::remove_dir_all(path).unwrap();
    }
}

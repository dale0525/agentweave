use super::*;

#[tokio::test]
async fn next_turn_uses_the_newly_published_snapshot() {
    let root = tempdir().unwrap();
    let package_root = root.path().join("runtime");
    write_turn_runtime_package(&package_root, "first_tool").await;
    let manager = turn_skill_manager(root.path()).await;
    let workspace = test_workspace("snapshot-next-turn");
    let runner = TurnRunner::new_with_manager_and_config(
        SnapshotSwapModel {
            calls: AtomicUsize::new(0),
            manager: manager.clone(),
            package_root,
            fail_reload: false,
        },
        manager,
        RuntimeConfig::workspace_write(workspace.clone(), workspace.clone()),
    );

    runner.run("first turn").await.unwrap();
    let events = runner.run("second turn").await.unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallStarted { name, .. } if name == "second_tool"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallFinished { result, .. }
            if result["ok"] == true && result["tool"] == "second_tool"
    )));
    remove_workspace(&workspace);
}

#[tokio::test]
async fn failed_reload_does_not_change_the_running_turn_snapshot() {
    let root = tempdir().unwrap();
    let package_root = root.path().join("runtime");
    write_turn_runtime_package(&package_root, "first_tool").await;
    let manager = turn_skill_manager(root.path()).await;
    let initial = manager.current_snapshot();
    let workspace = test_workspace("snapshot-failed-reload");
    let runner = TurnRunner::new_with_manager_and_config(
        SnapshotSwapModel {
            calls: AtomicUsize::new(0),
            manager: manager.clone(),
            package_root,
            fail_reload: true,
        },
        manager.clone(),
        RuntimeConfig::workspace_write(workspace.clone(), workspace.clone()),
    );

    let events = runner.run("use the tool").await.unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallStarted { name, .. } if name == "first_tool"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallFinished { result, .. }
            if result["ok"] == true && result["tool"] == "first_tool"
    )));
    assert!(Arc::ptr_eq(&initial, &manager.current_snapshot()));
    remove_workspace(&workspace);
}

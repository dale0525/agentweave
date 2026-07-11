use crate::skill_state::SkillStateStore;
use crate::skill_store::{SkillRevisionStore, SkillStorePaths};
use crate::skill_store_locks::acquire_os_revision_lock;
use crate::storage::Storage;
use std::time::Duration;
use std::{env, process::Stdio};
use tempfile::tempdir;

#[tokio::test]
async fn os_revision_lock_serializes_independent_file_descriptions_and_releases_on_drop() {
    let app = tempdir().unwrap();
    let cache = tempdir().unwrap();
    let paths = SkillStorePaths::prepare(app.path(), cache.path())
        .await
        .unwrap();
    let revision_id = SkillStateStore::allocate_revision_id();
    let first = acquire_os_revision_lock(&paths.identity, &revision_id)
        .await
        .unwrap();
    let identity = paths.identity.clone();
    let second_revision = revision_id.clone();
    let second =
        tokio::spawn(async move { acquire_os_revision_lock(&identity, &second_revision).await });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(!second.is_finished());

    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(second);

    acquire_os_revision_lock(&paths.identity, &revision_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn revision_lock_rejects_replaced_managed_or_locks_root() {
    for replace_locks in [false, true] {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let replaced = if replace_locks {
            paths.managed.join(".locks")
        } else {
            paths.managed.clone()
        };
        let old = replaced.with_extension("old");
        tokio::fs::rename(&replaced, &old).await.unwrap();
        tokio::fs::create_dir(&replaced).await.unwrap();

        let error = match acquire_os_revision_lock(
            &paths.identity,
            &SkillStateStore::allocate_revision_id(),
        )
        .await
        {
            Ok(_) => panic!("replaced store root must be rejected"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("identity"));
    }
}

#[test]
#[ignore]
fn subprocess_revision_lock_helper() {
    let app = env::var_os("GENERAL_AGENT_TEST_APP_ROOT").unwrap();
    let cache = env::var_os("GENERAL_AGENT_TEST_CACHE_ROOT").unwrap();
    let revision_id = env::var("GENERAL_AGENT_TEST_REVISION_ID").unwrap();
    let marker = env::var_os("GENERAL_AGENT_TEST_LOCK_MARKER").unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let paths = SkillStorePaths::prepare(app.as_ref(), cache.as_ref())
            .await
            .unwrap();
        let _guard = acquire_os_revision_lock(&paths.identity, &revision_id)
            .await
            .unwrap();
        tokio::fs::write(marker, b"locked").await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
}

#[test]
#[ignore]
fn subprocess_store_operation_helper() {
    let app = env::var_os("GENERAL_AGENT_TEST_APP_ROOT").unwrap();
    let cache = env::var_os("GENERAL_AGENT_TEST_CACHE_ROOT").unwrap();
    let database = env::var("GENERAL_AGENT_TEST_DATABASE_URL").unwrap();
    let revision_id = env::var("GENERAL_AGENT_TEST_REVISION_ID").unwrap();
    let operation = env::var("GENERAL_AGENT_TEST_OPERATION").unwrap();
    let result_path = env::var_os("GENERAL_AGENT_TEST_RESULT").unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let paths = SkillStorePaths::prepare(app.as_ref(), cache.as_ref())
            .await
            .unwrap();
        let state = SkillStateStore::new(Storage::connect(&database).await.unwrap());
        let store = SkillRevisionStore::new(paths, state);
        let result = match operation.as_str() {
            "write" => store
                .write_staging_file(
                    &revision_id,
                    std::path::Path::new("SKILL.md"),
                    b"---\nname: subprocess\ndescription: acknowledged edit\n---\nedited\n",
                )
                .await
                .map(|_| ()),
            "promote" => store.promote_revision(&revision_id).await.map(|_| ()),
            _ => panic!("unknown subprocess operation"),
        };
        let result = result
            .map(|_| "ok".to_string())
            .unwrap_or_else(|error| format!("error:{error:#}"));
        tokio::fs::write(result_path, result).await.unwrap();
    });
}

#[tokio::test]
async fn subprocess_lock_contention_releases_after_holder_is_killed() {
    let app = tempdir().unwrap();
    let cache = tempdir().unwrap();
    let revision_id = SkillStateStore::allocate_revision_id();
    let first_marker = app.path().join("first.locked");
    let second_marker = app.path().join("second.locked");
    let mut first = spawn_lock_helper(app.path(), cache.path(), &revision_id, &first_marker);
    wait_for_path(&first_marker).await;
    let mut second = spawn_lock_helper(app.path(), cache.path(), &revision_id, &second_marker);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!second_marker.exists());

    first.kill().unwrap();
    first.wait().unwrap();
    wait_for_path(&second_marker).await;
    second.kill().unwrap();
    second.wait().unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn subprocess_write_then_promote_preserves_acknowledged_edit() {
    let app = tempdir().unwrap();
    let cache = tempdir().unwrap();
    let database_path = app.path().join("state.db");
    let database = format!("sqlite://{}?mode=rwc", database_path.display());
    let paths = SkillStorePaths::prepare(app.path(), cache.path())
        .await
        .unwrap();
    let state = SkillStateStore::new(Storage::connect(&database).await.unwrap());
    let store = SkillRevisionStore::new(paths.clone(), state.clone());
    let package = write_subprocess_package().await;
    let staged = store
        .create_staging_revision(package.path(), "owner-1")
        .await
        .unwrap();
    let marker = app.path().join("writer.locked");
    let release = app.path().join("writer.release");
    let writer_result = app.path().join("writer.result");
    let promote_result = app.path().join("promote.result");
    let mut writer = spawn_store_helper(
        app.path(),
        cache.path(),
        &database,
        &staged.revision_id,
        "write",
        &writer_result,
        Some((&marker, &release)),
    );
    wait_for_path(&marker).await;
    let mut promoter = spawn_store_helper(
        app.path(),
        cache.path(),
        &database,
        &staged.revision_id,
        "promote",
        &promote_result,
        None,
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!promote_result.exists());
    tokio::fs::write(&release, b"go").await.unwrap();
    assert!(writer.wait().unwrap().success());
    assert!(promoter.wait().unwrap().success());
    assert_eq!(
        tokio::fs::read_to_string(writer_result).await.unwrap(),
        "ok"
    );
    assert_eq!(
        tokio::fs::read_to_string(promote_result).await.unwrap(),
        "ok"
    );

    let record = state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record.status,
        crate::skill_state::SkillRevisionStatus::Managed
    );
    assert_eq!(
        record.content_hash,
        crate::skill_source::hash_package_tree(std::path::Path::new(&record.storage_path))
            .await
            .unwrap()
    );
    assert!(
        tokio::fs::read_to_string(std::path::Path::new(&record.storage_path).join("SKILL.md"))
            .await
            .unwrap()
            .contains("acknowledged edit")
    );
}

fn spawn_lock_helper(
    app: &std::path::Path,
    cache: &std::path::Path,
    revision_id: &str,
    marker: &std::path::Path,
) -> std::process::Child {
    std::process::Command::new(env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "skill_store_lock_tests::subprocess_revision_lock_helper",
            "--nocapture",
        ])
        .env("GENERAL_AGENT_TEST_APP_ROOT", app)
        .env("GENERAL_AGENT_TEST_CACHE_ROOT", cache)
        .env("GENERAL_AGENT_TEST_REVISION_ID", revision_id)
        .env("GENERAL_AGENT_TEST_LOCK_MARKER", marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn spawn_store_helper(
    app: &std::path::Path,
    cache: &std::path::Path,
    database: &str,
    revision_id: &str,
    operation: &str,
    result: &std::path::Path,
    gate: Option<(&std::path::Path, &std::path::Path)>,
) -> std::process::Child {
    let mut command = std::process::Command::new(env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "skill_store_lock_tests::subprocess_store_operation_helper",
            "--nocapture",
        ])
        .env("GENERAL_AGENT_TEST_APP_ROOT", app)
        .env("GENERAL_AGENT_TEST_CACHE_ROOT", cache)
        .env("GENERAL_AGENT_TEST_DATABASE_URL", database)
        .env("GENERAL_AGENT_TEST_REVISION_ID", revision_id)
        .env("GENERAL_AGENT_TEST_OPERATION", operation)
        .env("GENERAL_AGENT_TEST_RESULT", result)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some((marker, release)) = gate {
        command
            .env("GENERAL_AGENT_TEST_AFTER_LOCK_MARKER", marker)
            .env("GENERAL_AGENT_TEST_AFTER_LOCK_RELEASE", release);
    }
    command.spawn().unwrap()
}

async fn wait_for_path(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn write_subprocess_package() -> tempfile::TempDir {
    let root = tempdir().unwrap();
    tokio::fs::write(
        root.path().join("general-agent.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "id": "com.example.subprocess",
            "version": "1.0.0",
            "displayName": "subprocess",
            "kind": "instruction_only",
            "package": {"includeInstructions": true, "includeRuntime": false}
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("SKILL.md"),
        "---\nname: subprocess\ndescription: initial\n---\ninitial\n",
    )
    .await
    .unwrap();
    root
}

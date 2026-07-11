use crate::skill_state::SkillStateStore;
use crate::skill_store::SkillStorePaths;
use crate::skill_store_locks::acquire_os_revision_lock;
use std::time::Duration;
use tempfile::tempdir;

#[cfg(unix)]
#[tokio::test]
async fn os_revision_lock_serializes_independent_file_descriptions_and_releases_on_drop() {
    let app = tempdir().unwrap();
    let cache = tempdir().unwrap();
    let paths = SkillStorePaths::prepare(app.path(), cache.path())
        .await
        .unwrap();
    let revision_id = SkillStateStore::allocate_revision_id();
    let first = acquire_os_revision_lock(&paths.managed, &revision_id)
        .await
        .unwrap();
    let managed = paths.managed.clone();
    let second_revision = revision_id.clone();
    let second =
        tokio::spawn(async move { acquire_os_revision_lock(&managed, &second_revision).await });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(!second.is_finished());

    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(second);

    acquire_os_revision_lock(&paths.managed, &revision_id)
        .await
        .unwrap();
}

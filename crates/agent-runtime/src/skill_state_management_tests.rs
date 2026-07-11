use crate::skill_package::SkillPackageId;
use crate::skill_state::{NewSkillRevision, SkillStateStore};
use crate::storage::Storage;
use serde_json::json;

const NOW: &str = "2026-01-01T00:00:00Z";

#[tokio::test]
async fn managed_installation_view_rejects_dangling_active_revision() {
    let (storage, state) = fixture().await;
    insert_installation(
        &storage,
        "com.example.dangling",
        Some(&uuid::Uuid::new_v4().to_string()),
    )
    .await;

    assert_consistency_error(state.list_managed_installations_with_revisions().await);
}

#[tokio::test]
async fn managed_installation_view_rejects_cross_package_active_revision() {
    let (storage, state) = fixture().await;
    let revision_id = insert_revision(&state, "com.example.source").await;
    insert_installation(&storage, "com.example.target", Some(&revision_id)).await;

    assert_consistency_error(state.list_managed_installations_with_revisions().await);
}

#[tokio::test]
async fn managed_installation_view_rejects_non_managed_active_revision_lifecycle() {
    let (storage, state) = fixture().await;
    let package_id = SkillPackageId::parse("com.example.staging").unwrap();
    let revision_id = uuid::Uuid::new_v4().to_string();
    state
        .create_staging_revision_record(&revision_id, new_revision(package_id))
        .await
        .unwrap();
    insert_installation(&storage, "com.example.staging", Some(&revision_id)).await;

    assert_consistency_error(state.list_managed_installations_with_revisions().await);
}

#[tokio::test]
async fn managed_installation_without_active_revision_keeps_empty_display_semantics() {
    let (storage, state) = fixture().await;
    insert_installation(&storage, "com.example.inactive", None).await;

    let rows = state
        .list_managed_installations_with_revisions()
        .await
        .unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].active_version, None);
    assert_eq!(rows[0].installation.active_revision_id, None);
}

#[tokio::test]
async fn staging_compensation_delete_refuses_a_changed_record() {
    let (storage, state) = fixture().await;
    let package_id = SkillPackageId::parse("com.example.compensation").unwrap();
    let revision_id = uuid::Uuid::new_v4().to_string();
    let record = state
        .create_staging_revision_record(&revision_id, new_revision(package_id))
        .await
        .unwrap();
    sqlx::query("UPDATE skill_revisions SET validation_json = ? WHERE revision_id = ?")
        .bind(json!({"changed": true}).to_string())
        .bind(&revision_id)
        .execute(storage.pool())
        .await
        .unwrap();

    let error = state
        .delete_staging_revision_record_if_matches(&record)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("changed before compensation"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM skill_revisions WHERE revision_id = ?")
            .bind(&revision_id)
            .fetch_one(storage.pool())
            .await
            .unwrap(),
        1
    );
}

async fn fixture() -> (Storage, SkillStateStore) {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    (storage, state)
}

async fn insert_revision(state: &SkillStateStore, package: &str) -> String {
    state
        .create_revision(new_revision(SkillPackageId::parse(package).unwrap()))
        .await
        .unwrap()
        .revision_id
}

fn new_revision(package_id: SkillPackageId) -> NewSkillRevision {
    NewSkillRevision {
        package_id,
        version: "1.0.0".into(),
        content_hash: "hash".into(),
        storage_path: "/managed/revision".into(),
        descriptor_json: json!({}),
        validation_json: json!({}),
        created_by: "owner-1".into(),
    }
}

async fn insert_installation(storage: &Storage, package: &str, revision_id: Option<&str>) {
    let mut connection = storage.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES (?, 'managed', ?, ?, 'approved', ?, ?, ?)"#,
    )
    .bind(package)
    .bind(revision_id)
    .bind(i64::from(revision_id.is_some()))
    .bind(if revision_id.is_some() {
        "active"
    } else {
        "inactive"
    })
    .bind(NOW)
    .bind(NOW)
    .execute(&mut *connection)
    .await
    .unwrap();
}

fn assert_consistency_error<T: std::fmt::Debug>(result: anyhow::Result<T>) {
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("managed installation consistency error"),
        "{message}"
    );
}

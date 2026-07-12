use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    NewSkillRevision, SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionPromotion,
    SkillRevisionStatus, SkillStateBoundaryError, SkillStateStore,
};
use crate::storage::Storage;
use serde_json::json;

async fn staging_revision() -> (SkillStateStore, crate::skill_state::SkillRevisionRecord) {
    let state = SkillStateStore::new(Storage::connect("sqlite::memory:").await.unwrap());
    let revision_id = SkillStateStore::allocate_revision_id();
    let record = state
        .create_staging_revision_record(
            &revision_id,
            NewSkillRevision {
                package_id: SkillPackageId::parse("com.example.cas").unwrap(),
                version: "1.0.0".into(),
                content_hash: "old-hash".into(),
                storage_path: format!("staging/{revision_id}"),
                descriptor_json: json!({"version": "1.0.0"}),
                validation_json: json!({"status": "pending"}),
                created_by: "owner-1".into(),
            },
        )
        .await
        .unwrap();
    (state, record)
}

#[tokio::test]
async fn promotion_cas_rejects_metadata_changed_after_observation() {
    let (state, observed) = staging_revision().await;
    state
        .refresh_staging_revision_metadata(
            &observed.revision_id,
            SkillRevisionMetadata {
                version: "2.0.0".into(),
                content_hash: "new-hash".into(),
                descriptor_json: json!({"version": "2.0.0"}),
                validation_json: json!({"status": "valid"}),
            },
        )
        .await
        .unwrap();

    let error = state
        .promote_revision_record_with_metadata_cas(
            &observed.revision_id,
            SkillRevisionExpectation::from(&observed),
            SkillRevisionPromotion {
                version: "1.0.0".into(),
                content_hash: "old-hash".into(),
                storage_path: format!("managed/{}", observed.revision_id),
                descriptor_json: observed.descriptor_json.clone(),
                validation_json: json!({"status": "valid"}),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillStateBoundaryError>(),
        Some(SkillStateBoundaryError::Conflict(_))
    ));
    assert!(!error.to_string().contains(&observed.revision_id));
}

#[tokio::test]
async fn quarantine_cas_rejects_metadata_changed_after_observation() {
    let (state, observed) = staging_revision().await;
    state
        .refresh_staging_revision_metadata(
            &observed.revision_id,
            SkillRevisionMetadata {
                version: "2.0.0".into(),
                content_hash: "new-hash".into(),
                descriptor_json: json!({"version": "2.0.0"}),
                validation_json: json!({"status": "valid"}),
            },
        )
        .await
        .unwrap();

    let error = state
        .quarantine_revision_record_cas(
            &observed.revision_id,
            &format!("quarantine/{}", observed.revision_id),
            "stale",
            SkillRevisionExpectation::from(&observed),
            None,
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillStateBoundaryError>(),
        Some(SkillStateBoundaryError::Conflict(_))
    ));
    assert!(!error.to_string().contains(&observed.revision_id));
}

#[tokio::test]
async fn staging_metadata_cas_rejects_metadata_changed_after_observation() {
    let (state, observed) = staging_revision().await;
    state
        .refresh_staging_revision_metadata(
            &observed.revision_id,
            SkillRevisionMetadata {
                version: "2.0.0".into(),
                content_hash: "external-hash".into(),
                descriptor_json: json!({"version": "2.0.0"}),
                validation_json: json!({"status": "external"}),
            },
        )
        .await
        .unwrap();

    let error = state
        .refresh_staging_revision_metadata_cas(
            &observed.revision_id,
            SkillRevisionExpectation::from(&observed),
            SkillRevisionMetadata {
                version: "3.0.0".into(),
                content_hash: "writer-hash".into(),
                descriptor_json: json!({"version": "3.0.0"}),
                validation_json: json!({"status": "valid"}),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillStateBoundaryError>(),
        Some(SkillStateBoundaryError::Conflict(_))
    ));
    assert!(!error.to_string().contains(&observed.revision_id));
    let record = state
        .get_revision(&observed.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.content_hash, "external-hash");
}

#[tokio::test]
async fn staging_metadata_cas_rejects_non_staging_expectation_before_sql() {
    let (state, record) = staging_revision().await;
    let mut expected = SkillRevisionExpectation::from(&record);
    expected.status = SkillRevisionStatus::Managed;

    let error = state
        .refresh_staging_revision_metadata_cas(
            &record.revision_id,
            expected,
            SkillRevisionMetadata {
                version: record.version.clone(),
                content_hash: record.content_hash.clone(),
                descriptor_json: record.descriptor_json.clone(),
                validation_json: json!({"status": "valid"}),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillStateBoundaryError>(),
        Some(SkillStateBoundaryError::Conflict(_))
    ));
    assert!(!error.to_string().contains(&record.revision_id));
}

#[tokio::test]
async fn staging_refresh_cas_rejects_validation_only_change() {
    let (state, observed) = staging_revision().await;
    state
        .update_revision_validation(&observed.revision_id, json!({"status": "reviewed"}))
        .await
        .unwrap();

    let error = state
        .refresh_staging_revision_metadata_cas(
            &observed.revision_id,
            SkillRevisionExpectation::from(&observed),
            SkillRevisionMetadata {
                version: observed.version.clone(),
                content_hash: "writer-hash".into(),
                descriptor_json: observed.descriptor_json.clone(),
                validation_json: json!({"status": "valid"}),
            },
        )
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("changed since operation observation"));
}

#[tokio::test]
async fn promotion_cas_rejects_validation_only_change() {
    let (state, observed) = staging_revision().await;
    state
        .update_revision_validation(&observed.revision_id, json!({"status": "reviewed"}))
        .await
        .unwrap();

    let error = state
        .promote_revision_record_with_metadata_cas(
            &observed.revision_id,
            SkillRevisionExpectation::from(&observed),
            SkillRevisionPromotion {
                version: observed.version.clone(),
                content_hash: observed.content_hash.clone(),
                storage_path: format!("managed/{}", observed.revision_id),
                descriptor_json: observed.descriptor_json.clone(),
                validation_json: json!({"status": "valid"}),
            },
        )
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("changed since operation observation"));
}

#[tokio::test]
async fn quarantine_cas_rejects_validation_only_change() {
    let (state, observed) = staging_revision().await;
    state
        .update_revision_validation(&observed.revision_id, json!({"status": "reviewed"}))
        .await
        .unwrap();

    let error = state
        .quarantine_revision_record_cas(
            &observed.revision_id,
            &format!("quarantine/{}", observed.revision_id),
            "stale",
            SkillRevisionExpectation::from(&observed),
            None,
        )
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("changed since operation observation"));
}

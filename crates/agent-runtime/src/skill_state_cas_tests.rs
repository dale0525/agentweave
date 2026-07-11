use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    NewSkillRevision, SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionPromotion,
    SkillStateStore,
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

    assert!(
        error
            .to_string()
            .contains("revision changed since operation observation")
    );
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

    assert!(
        error
            .to_string()
            .contains("revision changed since operation observation")
    );
}

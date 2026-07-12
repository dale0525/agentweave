use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_management::SkillManagementError;
use crate::skill_store_public_types::SkillStoreBoundaryError;
use serde_json::json;

#[tokio::test]
async fn revision_drift_on_the_inspection_path_maps_to_conflict() {
    let fixture = AuthoringFixture::new().await;
    let draft = fixture.draft().await;
    let observed = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    fixture
        .state
        .update_revision_validation(
            &draft.revision_id,
            json!({"ok": true, "errors": [], "warnings": []}),
        )
        .await
        .unwrap();

    let inspection_error = match fixture.store.inspect_revision_content(&observed).await {
        Ok(_) => panic!("stale inspection unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(matches!(
        inspection_error.downcast_ref::<SkillStoreBoundaryError>(),
        Some(SkillStoreBoundaryError::Conflict(_))
    ));
    assert!(matches!(
        SkillManagementError::from_store(
            "inspect skill revision",
            "skill revision",
            inspection_error,
        ),
        SkillManagementError::Conflict { .. }
    ));
}

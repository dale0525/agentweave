use crate::tenant_attempt::{AttemptFaultPoint, CleanupTestAction, TenantAttemptJournal};
use std::path::{Path, PathBuf};

struct JournalFixture {
    _root: tempfile::TempDir,
    control: PathBuf,
    quarantine: PathBuf,
    tenants: PathBuf,
}

impl JournalFixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let control = root.path().join("control");
        let quarantine = root.path().join("quarantine");
        let tenants = root.path().join("tenants");
        for path in [&control, &quarantine, &tenants] {
            tokio::fs::create_dir(path).await.unwrap();
        }
        let control = tokio::fs::canonicalize(control).await.unwrap();
        let quarantine = tokio::fs::canonicalize(quarantine).await.unwrap();
        let tenants = tokio::fs::canonicalize(tenants).await.unwrap();
        Self {
            _root: root,
            control,
            quarantine,
            tenants,
        }
    }

    async fn begin(&self) -> anyhow::Result<TenantAttemptJournal> {
        TenantAttemptJournal::begin(
            self.control.clone(),
            self.quarantine.clone(),
            "tenant-a",
            vec![self.tenants.clone()],
        )
        .await
    }

    fn tenant(&self) -> PathBuf {
        self.tenants.join("tenant-a")
    }
}

#[tokio::test]
async fn every_resource_publication_crash_point_recovers_and_retries_cleanly() {
    for point in [
        AttemptFaultPoint::PlanDurable,
        AttemptFaultPoint::ObjectDurable,
        AttemptFaultPoint::PreparedJournalStored,
        AttemptFaultPoint::PublishedObjectDurable,
        AttemptFaultPoint::PublishedJournalStored,
    ] {
        let fixture = JournalFixture::new().await;
        {
            let mut crashed = fixture.begin().await.unwrap();
            crashed.fail_once_at_for_test(point);
            assert!(crashed.ensure_directory(&fixture.tenant()).await.is_err());
        }

        let mut retry = fixture.begin().await.unwrap();
        retry.ensure_directory(&fixture.tenant()).await.unwrap();
        retry.commit().await.unwrap();
        assert!(fixture.tenant().is_dir(), "retry failed after {point:?}");
    }
}

#[tokio::test]
async fn quarantine_publication_crash_points_recover_and_retry_cleanly() {
    for point in [
        AttemptFaultPoint::QuarantinePlanDurable,
        AttemptFaultPoint::QuarantineObjectStored,
        AttemptFaultPoint::QuarantinePublished,
    ] {
        let fixture = JournalFixture::new().await;
        let tenant = fixture.tenant();
        {
            let mut crashed = fixture.begin().await.unwrap();
            crashed.ensure_directory(&tenant).await.unwrap();
            crashed.fail_once_at_for_test(point);
            assert!(crashed.cleanup().await.is_err());
        }

        let mut retry = fixture.begin().await.unwrap();
        retry.ensure_directory(&tenant).await.unwrap();
        retry.commit().await.unwrap();
        assert!(tenant.is_dir(), "retry failed after {point:?}");
    }
}

#[tokio::test]
async fn stale_same_identity_with_wrong_object_token_is_preserved() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    let database = tenant.join("state.db");
    {
        let mut crashed = fixture.begin().await.unwrap();
        crashed.ensure_directory(&tenant).await.unwrap();
        crashed.create_owned_file(&database).await.unwrap();
        crashed
            .replace_object_token_for_test(&database, "foreign-object-token")
            .await
            .unwrap();
    }

    assert!(fixture.begin().await.is_err());
    assert!(database.exists());
    assert!(TenantAttemptJournal::journal_exists_for_test(
        &fixture.control,
        "tenant-a"
    ));
}

#[tokio::test]
async fn occupied_quarantine_preserves_canonical_resource_and_journal() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    let database = tenant.join("state.db");
    let mut attempt = fixture.begin().await.unwrap();
    attempt.ensure_directory(&tenant).await.unwrap();
    attempt.create_owned_file(&database).await.unwrap();
    attempt
        .occupy_quarantine_destination_for_test(&database)
        .await
        .unwrap();

    assert!(attempt.cleanup().await.is_err());
    assert!(database.exists());
    assert!(attempt.journal_exists_for_current_attempt_for_test());
}

#[tokio::test]
async fn post_validation_replacement_is_not_deleted_and_journal_is_retained() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    let database = tenant.join("state.db");
    let mut attempt = fixture.begin().await.unwrap();
    attempt.ensure_directory(&tenant).await.unwrap();
    attempt.create_owned_file(&database).await.unwrap();
    attempt.set_cleanup_action_for_test(
        database.clone(),
        CleanupTestAction::ReplaceQuarantineBeforeDelete,
    );

    assert!(attempt.cleanup().await.is_err());

    let replacement = attempt.quarantine_destination_for_test(&database).unwrap();
    assert_eq!(
        tokio::fs::read(replacement).await.unwrap(),
        b"foreign replacement"
    );
    assert!(attempt.journal_exists_for_current_attempt_for_test());
}

#[tokio::test]
async fn partial_cleanup_persists_only_unresolved_ownership_records() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    let first = tenant.join("first.db");
    let second = tenant.join("second.db");
    let mut attempt = fixture.begin().await.unwrap();
    attempt.ensure_directory(&tenant).await.unwrap();
    attempt.create_owned_file(&first).await.unwrap();
    attempt.create_owned_file(&second).await.unwrap();
    attempt
        .replace_object_token_for_test(&second, "foreign-object-token")
        .await
        .unwrap();

    assert!(attempt.cleanup().await.is_err());

    assert!(!first.exists());
    assert!(second.exists());
    assert_eq!(attempt.resource_paths_for_test(), vec![tenant, second]);
    assert!(attempt.journal_exists_for_current_attempt_for_test());
}

#[tokio::test]
async fn crash_after_root_move_is_recovered_from_external_journal() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    {
        let mut crashed = fixture.begin().await.unwrap();
        crashed.ensure_directory(&tenant).await.unwrap();
        crashed.set_cleanup_action_for_test(tenant.clone(), CleanupTestAction::CrashAfterMove);
        assert!(crashed.cleanup().await.is_err());
        assert!(!tenant.exists());
    }

    let mut retry = fixture.begin().await.unwrap();
    retry.ensure_directory(&tenant).await.unwrap();
    retry.commit().await.unwrap();
    assert!(tenant.is_dir());
}

#[allow(dead_code)]
fn assert_path_is_under(path: &Path, parent: &Path) {
    assert!(path.starts_with(parent));
}

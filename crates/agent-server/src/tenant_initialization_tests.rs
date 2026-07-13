use crate::tenant_attempt::TenantAttemptJournal;

#[tokio::test]
async fn failed_attempt_quarantines_owned_file_and_allows_clean_retry() {
    let root = tempfile::tempdir().unwrap();
    let tenant = root.path().join("tenant");
    tokio::fs::create_dir(&tenant).await.unwrap();
    let mut attempt = TenantAttemptJournal::begin(tenant.clone(), true)
        .await
        .unwrap();
    let database = tenant.join("state.db");
    attempt.create_owned_file(&database).await.unwrap();
    tokio::fs::write(&database, b"partial sqlite")
        .await
        .unwrap();

    attempt.cleanup().await;

    assert!(!tenant.exists());
    tokio::fs::create_dir(&tenant).await.unwrap();
    let retry = TenantAttemptJournal::begin(tenant.clone(), true)
        .await
        .unwrap();
    retry.commit().await.unwrap();
    assert!(tenant.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn cleanup_preserves_foreign_replacement_at_original_path() {
    let root = tempfile::tempdir().unwrap();
    let tenant = root.path().join("tenant");
    tokio::fs::create_dir(&tenant).await.unwrap();
    let mut attempt = TenantAttemptJournal::begin(tenant.clone(), true)
        .await
        .unwrap();
    let database = tenant.join("state.db");
    attempt.create_owned_file(&database).await.unwrap();
    tokio::fs::remove_file(&database).await.unwrap();
    tokio::fs::write(&database, b"foreign replacement")
        .await
        .unwrap();

    attempt.cleanup().await;

    assert_eq!(
        tokio::fs::read(&database).await.unwrap(),
        b"foreign replacement"
    );
    assert!(tenant.exists());
}

#[tokio::test]
async fn cleanup_fails_closed_when_persistent_attempt_token_changes() {
    let root = tempfile::tempdir().unwrap();
    let tenant = root.path().join("tenant");
    tokio::fs::create_dir(&tenant).await.unwrap();
    let mut attempt = TenantAttemptJournal::begin(tenant.clone(), true)
        .await
        .unwrap();
    let database = tenant.join("state.db");
    attempt.create_owned_file(&database).await.unwrap();
    attempt
        .replace_marker_token_for_test("foreign-token")
        .await
        .unwrap();

    attempt.cleanup().await;

    assert!(database.exists());
    assert!(tenant.exists());
}

#[tokio::test]
async fn next_attempt_recovers_crash_stale_marker_before_initializing() {
    let root = tempfile::tempdir().unwrap();
    let tenant = root.path().join("tenant");
    tokio::fs::create_dir(&tenant).await.unwrap();
    let database = tenant.join("state.db");
    {
        let mut crashed = TenantAttemptJournal::begin(tenant.clone(), true)
            .await
            .unwrap();
        crashed.create_owned_file(&database).await.unwrap();
        tokio::fs::write(&database, b"partial sqlite")
            .await
            .unwrap();
    }

    let retry = TenantAttemptJournal::begin(tenant.clone(), false)
        .await
        .unwrap();

    assert!(!database.exists());
    retry.commit().await.unwrap();
    assert!(tenant.exists());
}

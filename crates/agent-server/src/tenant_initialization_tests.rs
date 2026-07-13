use crate::tenant_attempt::windows_open_contract_for_test;
use crate::tenant_attempt::{AttemptFaultPoint, CleanupTestAction, TenantAttemptJournal};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::path::{Path, PathBuf};
use std::str::FromStr;

struct JournalFixture {
    _root: tempfile::TempDir,
    control: PathBuf,
    quarantine: PathBuf,
    tenants: PathBuf,
}

#[test]
fn windows_access_contract_flushes_and_deletes_with_share_delete_denied() {
    let (share, flags, sync_access, write_access, cleanup_access) =
        windows_open_contract_for_test();
    assert_ne!(sync_access & 0x4000_0000, 0);
    assert_ne!(write_access & 0x4000_0000, 0);
    assert_ne!(cleanup_access & 0x0001_0000, 0);
    assert_eq!(share & 0x0000_0004, 0);
    assert_ne!(flags & 0x0200_0000, 0);
    assert_ne!(flags & 0x0020_0000, 0);
}

#[cfg(windows)]
#[tokio::test]
async fn windows_publication_flush_cleanup_commit_and_stale_retry() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    let database = tenant.join("state.db");
    {
        let mut crashed = fixture.begin().await.unwrap();
        crashed.ensure_directory(&tenant).await.unwrap();
        crashed.create_owned_file(&database).await.unwrap();
    }
    let mut retry = fixture.begin().await.unwrap();
    assert!(!tenant.exists());
    retry.ensure_directory(&tenant).await.unwrap();
    retry.create_owned_file(&database).await.unwrap();
    retry.commit().await.unwrap();
    assert!(database.is_file());
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
async fn replaced_temporary_file_is_rejected_after_publish() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    let database = tenant.join("state.db");
    let mut attempt = fixture.begin().await.unwrap();
    attempt.ensure_directory(&tenant).await.unwrap();
    attempt.replace_temporary_source_for_test(database.clone());

    assert!(attempt.create_owned_file(&database).await.is_err());
    assert_eq!(
        tokio::fs::read(&database).await.unwrap(),
        b"foreign replacement"
    );
    assert!(attempt.journal_exists_for_current_attempt_for_test());
    assert!(fixture.begin().await.is_err());
}

#[tokio::test]
async fn replaced_temporary_directory_is_rejected_after_publish() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    let mut attempt = fixture.begin().await.unwrap();
    attempt.replace_temporary_source_for_test(tenant.clone());

    assert!(attempt.ensure_directory(&tenant).await.is_err());
    tokio::fs::write(tenant.join("foreign.txt"), b"foreign")
        .await
        .unwrap();
    assert!(attempt.journal_exists_for_current_attempt_for_test());
    assert!(fixture.begin().await.is_err());
    assert_eq!(
        tokio::fs::read(tenant.join("foreign.txt")).await.unwrap(),
        b"foreign"
    );
}

#[tokio::test]
async fn stale_root_recovery_skips_foreign_canonical_and_removes_owned_sidecar_unit() {
    let fixture = JournalFixture::new().await;
    let tenant = fixture.tenant();
    let database = tenant.join("state.db");
    let sidecar = tenant.join("state.db-wal");
    let quarantine;
    {
        let mut crashed = fixture.begin().await.unwrap();
        crashed.ensure_directory(&tenant).await.unwrap();
        crashed.create_owned_file(&database).await.unwrap();
        tokio::fs::write(&sidecar, b"sqlite crash residue")
            .await
            .unwrap();
        quarantine = crashed.quarantine_destination_for_test(&tenant).unwrap();
        crashed.set_cleanup_action_for_test(tenant.clone(), CleanupTestAction::CrashAfterMove);
        assert!(crashed.cleanup().await.is_err());
        assert!(quarantine.join("state.db-wal").exists());
    }
    tokio::fs::create_dir(&tenant).await.unwrap();
    tokio::fs::write(tenant.join("foreign.txt"), b"preserve")
        .await
        .unwrap();

    let mut retry = fixture.begin().await.unwrap();
    assert!(!quarantine.exists());
    assert_eq!(
        tokio::fs::read(tenant.join("foreign.txt")).await.unwrap(),
        b"preserve"
    );
    assert!(!retry.ensure_directory(&tenant).await.unwrap());
    retry.commit().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn real_process_crash_recovers_sqlite_sidecars_and_preserves_foreign_canonical() {
    let fixture = JournalFixture::new().await;
    let marker = fixture.control.join("sqlite-crash-ready");
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .arg("tenant_initialization_tests::subprocess_crashes_after_sqlite_root_move")
        .arg("--exact")
        .arg("--nocapture")
        .env("GENERAL_AGENT_TEST_ATTEMPT_CONTROL", &fixture.control)
        .env("GENERAL_AGENT_TEST_ATTEMPT_QUARANTINE", &fixture.quarantine)
        .env("GENERAL_AGENT_TEST_ATTEMPT_TENANTS", &fixture.tenants)
        .env("GENERAL_AGENT_TEST_ATTEMPT_CRASH_READY", &marker)
        .spawn()
        .unwrap();
    wait_for_path(&marker).await;
    assert!(!child.wait().await.unwrap().success());
    let quarantine = PathBuf::from(tokio::fs::read_to_string(&marker).await.unwrap());
    assert!(quarantine.exists());

    let tenant = fixture.tenant();
    tokio::fs::create_dir(&tenant).await.unwrap();
    tokio::fs::write(tenant.join("foreign.txt"), b"preserve")
        .await
        .unwrap();
    let mut retry = fixture.begin().await.unwrap();
    assert!(!quarantine.exists());
    assert_eq!(
        tokio::fs::read(tenant.join("foreign.txt")).await.unwrap(),
        b"preserve"
    );
    retry.commit().await.unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn subprocess_crashes_after_sqlite_root_move() {
    let Some(control) = std::env::var_os("GENERAL_AGENT_TEST_ATTEMPT_CONTROL") else {
        return;
    };
    let quarantine =
        PathBuf::from(std::env::var_os("GENERAL_AGENT_TEST_ATTEMPT_QUARANTINE").unwrap());
    let tenants = PathBuf::from(std::env::var_os("GENERAL_AGENT_TEST_ATTEMPT_TENANTS").unwrap());
    let marker = PathBuf::from(std::env::var_os("GENERAL_AGENT_TEST_ATTEMPT_CRASH_READY").unwrap());
    let tenant = tenants.join("tenant-a");
    let database = tenant.join("state.db");
    let mut attempt = TenantAttemptJournal::begin(
        PathBuf::from(control),
        quarantine,
        "tenant-a",
        vec![tenants],
    )
    .await
    .unwrap();
    attempt.ensure_directory(&tenant).await.unwrap();
    attempt.create_owned_file(&database).await.unwrap();
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", database.display()))
        .unwrap()
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE crash_evidence (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO crash_evidence (value) VALUES ('sidecar')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(tenant.join("state.db-wal").exists());
    let moved = attempt.quarantine_destination_for_test(&tenant).unwrap();
    attempt.set_cleanup_action_for_test(tenant, CleanupTestAction::CrashAfterMove);
    assert!(attempt.cleanup().await.is_err());
    assert!(moved.join("state.db-wal").exists() || moved.join("state.db-shm").exists());
    std::fs::write(marker, moved.to_string_lossy().as_bytes()).unwrap();
    std::process::abort();
}

async fn wait_for_path(path: &Path) {
    let started = std::time::Instant::now();
    while !path.exists() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
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

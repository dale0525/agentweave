use crate::skill_package::SkillPackageId;
use crate::skill_source::{ManagedSkillSource, SkillSource};
use crate::skill_state::{SkillLayerRecord, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use serde_json::json;
use std::path::Path;
use tempfile::{TempDir, tempdir};

struct LimitsFixture {
    _app: TempDir,
    _cache: TempDir,
    state: SkillStateStore,
    paths: SkillStorePaths,
    authoring_store: SkillRevisionStore,
}

impl LimitsFixture {
    async fn new() -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let state = SkillStateStore::new(Storage::connect("sqlite::memory:").await.unwrap());
        let authoring_store = SkillRevisionStore::new(paths.clone(), state.clone());
        Self {
            _app: app,
            _cache: cache,
            state,
            paths,
            authoring_store,
        }
    }

    async fn active(&self, id: &str) -> crate::skill_store::StoredSkillRevision {
        let package = write_package(id).await;
        let staged = self
            .authoring_store
            .create_staging_revision(package.path(), "owner-1")
            .await
            .unwrap();
        let managed = self
            .authoring_store
            .promote_revision(&staged.revision_id)
            .await
            .unwrap();
        self.state
            .activate_revision(
                &SkillPackageId::parse(id).unwrap(),
                &managed.revision_id,
                SkillLayerRecord::Managed,
                "owner-1",
            )
            .await
            .unwrap();
        managed
    }

    fn bounded_source(&self) -> ManagedSkillSource {
        ManagedSkillSource::from_store(SkillRevisionStore::with_limits(
            self.paths.clone(),
            self.state.clone(),
            SkillStoreLimits {
                max_file_bytes: 400,
                max_package_bytes: 2000,
                max_entries: 5,
                max_files: 10,
                max_directories: 3,
                max_depth: 3,
                max_relative_path_bytes: 80,
            },
        ))
    }
}

#[tokio::test]
async fn managed_discovery_bounds_each_invalid_peer_without_blocking_valid_peer() {
    let fixture = LimitsFixture::new().await;
    let valid = fixture.active("com.example.limit-valid").await;
    let oversized = fixture.active("com.example.limit-oversized").await;
    make_tree_writable(&oversized.path).await;
    tokio::fs::write(oversized.path.join("oversized.bin"), vec![0_u8; 401])
        .await
        .unwrap();
    let entries = fixture.active("com.example.limit-entries").await;
    make_tree_writable(&entries.path).await;
    for index in 0..4 {
        tokio::fs::write(entries.path.join(format!("zero-{index}")), [])
            .await
            .unwrap();
    }
    let deep = fixture.active("com.example.limit-deep").await;
    make_tree_writable(&deep.path).await;
    tokio::fs::create_dir_all(deep.path.join("a/b/c"))
        .await
        .unwrap();
    tokio::fs::write(deep.path.join("a/b/c/deep"), [])
        .await
        .unwrap();
    let long_path = fixture.active("com.example.limit-long-path").await;
    make_tree_writable(&long_path.path).await;
    tokio::fs::write(long_path.path.join("x".repeat(81)), [])
        .await
        .unwrap();
    let source = fixture.bounded_source();

    let discovered = source.discover().await.unwrap();

    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].root, valid.path);
    let reasons = source
        .issues()
        .into_iter()
        .map(|issue| issue.reason)
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "file exceeds 400 byte limit",
        "entry count exceeds 5 limit",
        "path depth exceeds 3 component limit",
        "relative path exceeds 80 byte limit",
    ] {
        assert!(reasons.contains(expected), "missing {expected}: {reasons}");
    }
}

#[tokio::test]
async fn managed_source_does_not_validate_or_quarantine_other_layers() {
    let fixture = LimitsFixture::new().await;
    let package = write_package("com.example.builtin-record").await;
    let staged = fixture
        .authoring_store
        .create_staging_revision(package.path(), "owner-1")
        .await
        .unwrap();
    let managed = fixture
        .authoring_store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();
    let package_id = SkillPackageId::parse("com.example.builtin-record").unwrap();
    fixture
        .state
        .activate_revision(
            &package_id,
            &managed.revision_id,
            SkillLayerRecord::Builtin,
            "owner-1",
        )
        .await
        .unwrap();
    let source = fixture.bounded_source();

    assert!(source.discover().await.unwrap().is_empty());
    assert!(source.issues().is_empty());
    assert_eq!(
        fixture
            .state
            .get_revision(&managed.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        crate::skill_state::SkillRevisionStatus::Managed
    );
    assert!(managed.path.is_dir());
}

#[tokio::test]
async fn transient_managed_validation_error_skips_without_quarantine_and_keeps_valid_peer() {
    let fixture = LimitsFixture::new().await;
    let transient = fixture.active("com.example.alpha-transient").await;
    let valid = fixture.active("com.example.zeta-valid").await;
    let faults = SkillStoreTestFaults::default();
    faults.fail_once(SkillStoreFaultPoint::ManagedDiscoveryTransientIo);
    let source = ManagedSkillSource::from_store(SkillRevisionStore::with_test_faults(
        fixture.paths.clone(),
        fixture.state.clone(),
        SkillStoreLimits::default(),
        faults,
    ));

    let discovered = source.discover().await.unwrap();

    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].root, valid.path);
    let issues = source.issues();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].revision_id, transient.revision_id);
    assert!(issues[0].reason.contains("transient managed discovery I/O"));
    let record = fixture
        .state
        .get_revision(&transient.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record.status,
        crate::skill_state::SkillRevisionStatus::Managed
    );
    assert!(transient.path.is_dir());
}

async fn write_package(id: &str) -> TempDir {
    let root = tempdir().unwrap();
    let name = id.rsplit('.').next().unwrap();
    tokio::fs::write(
        root.path().join("general-agent.json"),
        json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "displayName": name,
            "kind": "instruction_only",
            "package": {
                "includeInstructions": true,
                "includeRuntime": false
            }
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name}\n---\n{name}\n"),
    )
    .await
    .unwrap();
    root
}

async fn make_tree_writable(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = tokio::fs::symlink_metadata(&path).await.unwrap();
        let is_directory = metadata.is_dir();
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | if is_directory { 0o700 } else { 0o600 });
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        tokio::fs::set_permissions(&path, permissions)
            .await
            .unwrap();
        if is_directory {
            let mut entries = tokio::fs::read_dir(&path).await.unwrap();
            while let Some(entry) = entries.next_entry().await.unwrap() {
                stack.push(entry.path());
            }
        }
    }
}

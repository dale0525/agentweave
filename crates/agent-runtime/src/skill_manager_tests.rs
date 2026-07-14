use crate::platform::{CapabilitySet, PlatformId};
use crate::skill::SkillRegistry;
use crate::skill_catalog::{
    SkillCatalog, SkillCatalogEntry, SkillInstructionDocument, SkillSummary,
};
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_source::{DirectorySkillSource, DiscoveredSkillPackage, SkillLayer, SkillSource};
use async_trait::async_trait;
use semver::Version;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tempfile::tempdir;

#[tokio::test]
async fn reload_publishes_a_new_generation_atomically() {
    let root = tempdir().unwrap();
    write_instruction_package(
        root.path(),
        "first",
        "com.example.first",
        "First",
        "First body",
    )
    .await;
    let manager = test_manager(root.path()).await;
    let first = manager.current_snapshot();

    write_instruction_package(
        root.path(),
        "second",
        "com.example.second",
        "Second",
        "Second body",
    )
    .await;
    let report = manager.reload().await.unwrap();
    let second = manager.current_snapshot();

    assert_eq!(first.generation(), 1);
    assert_eq!(second.generation(), 2);
    assert_eq!(report.previous_generation, 1);
    assert_eq!(report.active_generation, 2);
    assert_eq!(first.packages().len(), 1);
    assert_eq!(second.packages().len(), 2);
}

#[tokio::test]
async fn failed_reload_keeps_the_previous_snapshot() {
    let root = tempdir().unwrap();
    write_instruction_package(
        root.path(),
        "first",
        "com.example.first",
        "First",
        "First body",
    )
    .await;
    let manager = test_manager(root.path()).await;
    let previous = manager.current_snapshot();
    tokio::fs::create_dir_all(root.path().join("broken"))
        .await
        .unwrap();
    tokio::fs::write(root.path().join("broken/agentweave.json"), "{invalid")
        .await
        .unwrap();

    assert!(manager.reload().await.is_err());
    let current = manager.current_snapshot();
    assert_eq!(current.generation(), previous.generation());
    assert!(Arc::ptr_eq(&current, &previous));
}

#[tokio::test]
async fn pre_publish_check_failure_keeps_the_previous_snapshot() {
    let root = tempdir().unwrap();
    write_instruction_package(
        root.path(),
        "first",
        "com.example.first",
        "First",
        "First body",
    )
    .await;
    let manager = test_manager(root.path()).await;
    let previous = manager.current_snapshot();
    write_instruction_package(
        root.path(),
        "second",
        "com.example.second",
        "Second",
        "Second body",
    )
    .await;

    let error = manager
        .reload_with_pre_publish(|_| async {
            Err::<(), _>(anyhow::anyhow!("diagnostics unavailable"))
        })
        .await
        .unwrap_err();

    assert!(error.to_string().contains("diagnostics unavailable"));
    assert!(Arc::ptr_eq(&previous, &manager.current_snapshot()));
}

#[tokio::test]
async fn published_snapshot_keeps_instruction_content_after_source_changes() {
    let root = tempdir().unwrap();
    let package_root = write_instruction_package(
        root.path(),
        "planning",
        "com.example.planning",
        "planning",
        "Original body",
    )
    .await;
    let manager = test_manager(root.path()).await;
    let snapshot = manager.current_snapshot();

    tokio::fs::write(
        package_root.join("SKILL.md"),
        skill_document("planning", "Changed body"),
    )
    .await
    .unwrap();
    let changed = snapshot
        .catalog()
        .load_instruction_documents(&["planning".into()], usize::MAX)
        .await
        .unwrap();
    tokio::fs::remove_file(package_root.join("SKILL.md"))
        .await
        .unwrap();
    let deleted = snapshot
        .catalog()
        .load_instruction_documents(&["planning".into()], usize::MAX)
        .await
        .unwrap();

    assert!(changed[0].content.contains("Original body"));
    assert_eq!(changed, deleted);
}

#[tokio::test]
async fn instruction_byte_limit_preserves_utf8_boundaries_and_metadata() {
    let root = tempdir().unwrap();
    let package_root = write_instruction_package(
        root.path(),
        "unicode",
        "com.example.unicode",
        "unicode",
        "中文内容",
    )
    .await;
    let catalog = SkillCatalog::from_entries(vec![
        SkillCatalog::read_package_entry(&package_root)
            .await
            .unwrap(),
    ])
    .unwrap();
    let full = catalog
        .load_instruction_documents(&["unicode".into()], usize::MAX)
        .await
        .unwrap();
    let boundary = full[0].content.find('中').unwrap() + 1;

    let limited = catalog
        .load_instruction_documents(&["unicode".into()], boundary)
        .await
        .unwrap();

    assert!(limited[0].truncated);
    assert!(limited[0].read_bytes < boundary);
    assert_eq!(limited[0].read_bytes, limited[0].content.len());
    assert_eq!(limited[0].original_bytes, full[0].content.len());
    assert!(
        limited[0]
            .content
            .is_char_boundary(limited[0].content.len())
    );
}

#[test]
fn catalog_rejects_mismatched_entry_names() {
    let mut entry = complete_catalog_entry("planning", "Plan safely.");
    entry.document.name = "other".into();

    let error = SkillCatalog::from_entries(vec![entry]).unwrap_err();

    assert!(error.to_string().contains("summary name"));
    assert!(error.to_string().contains("document name"));
}

#[test]
fn catalog_rejects_mismatched_entry_sources() {
    let mut entry = complete_catalog_entry("planning", "Plan safely.");
    entry.document.source = PathBuf::from("other/SKILL.md");

    let error = SkillCatalog::from_entries(vec![entry]).unwrap_err();

    assert!(error.to_string().contains("summary source"));
    assert!(error.to_string().contains("document source"));
}

#[test]
fn catalog_rejects_forged_complete_document_metadata() {
    let mut truncated = complete_catalog_entry("truncated", "content");
    truncated.document.truncated = true;
    assert!(
        SkillCatalog::from_entries(vec![truncated])
            .unwrap_err()
            .to_string()
            .contains("truncated")
    );

    let mut short_read = complete_catalog_entry("short-read", "content");
    short_read.document.read_bytes = 0;
    assert!(
        SkillCatalog::from_entries(vec![short_read])
            .unwrap_err()
            .to_string()
            .contains("read_bytes")
    );

    let mut forged_original = complete_catalog_entry("forged-original", "content");
    forged_original.document.original_bytes = 0;
    assert!(
        SkillCatalog::from_entries(vec![forged_original])
            .unwrap_err()
            .to_string()
            .contains("original_bytes")
    );
}

#[tokio::test]
async fn zero_instruction_limit_returns_no_content() {
    let catalog =
        SkillCatalog::from_entries(vec![complete_catalog_entry("planning", "Plan safely.")])
            .unwrap();

    let documents = catalog
        .load_instruction_documents(&["planning".into()], 0)
        .await
        .unwrap();

    assert!(documents[0].content.is_empty());
    assert!(documents[0].truncated);
    assert_eq!(documents[0].read_bytes, 0);
    assert_eq!(documents[0].original_bytes, "Plan safely.".len());
}

#[cfg(unix)]
#[tokio::test]
async fn read_package_entry_rejects_symlink_escape() {
    let package_root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    tokio::fs::write(
        outside.path().join("SKILL.md"),
        skill_document("outside", "Outside"),
    )
    .await
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("SKILL.md"),
        package_root.path().join("SKILL.md"),
    )
    .unwrap();

    let error = SkillCatalog::read_package_entry(package_root.path())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("symlink"));
}

#[cfg(unix)]
#[tokio::test]
async fn manager_new_rejects_runtime_package_root_symlink() {
    let root = tempdir().unwrap();
    let links = tempdir().unwrap();
    let (package_root, mut package) = discovered_runtime_package(root.path()).await;
    let linked_root = links.path().join("runtime");
    std::os::unix::fs::symlink(&package_root, &linked_root).unwrap();
    package.root = linked_root;
    let source = Arc::new(MutableSource::new(SkillLayer::Builtin, vec![package]));

    let error = SkillManager::new(config(vec![source])).await.unwrap_err();

    assert!(error.to_string().contains("symlink"));
}

#[cfg(unix)]
#[tokio::test]
async fn manager_new_rejects_runtime_manifest_symlink_escape() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let (package_root, package) = discovered_runtime_package(root.path()).await;
    replace_with_external_symlink(
        &package_root.join("skill.json"),
        &outside.path().join("skill.json"),
    )
    .await;
    let source = Arc::new(MutableSource::new(SkillLayer::Builtin, vec![package]));

    let error = SkillManager::new(config(vec![source])).await.unwrap_err();

    assert!(error.to_string().contains("symlink"));
}

#[cfg(unix)]
#[tokio::test]
async fn manager_new_rejects_runtime_entry_resource_symlink_escape() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let (package_root, package) = discovered_runtime_package(root.path()).await;
    replace_with_external_symlink(
        &package_root.join("index.js"),
        &outside.path().join("index.js"),
    )
    .await;
    let source = Arc::new(MutableSource::new(SkillLayer::Builtin, vec![package]));

    let error = SkillManager::new(config(vec![source])).await.unwrap_err();

    assert!(error.to_string().contains("symlink"));
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_resource_reload_failure_keeps_the_previous_snapshot() {
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let (package_root, package) = discovered_runtime_package(root.path()).await;
    let source = Arc::new(MutableSource::new(SkillLayer::Builtin, vec![package]));
    let manager = SkillManager::new(config(vec![source])).await.unwrap();
    let previous = manager.current_snapshot();
    replace_with_external_symlink(
        &package_root.join("index.js"),
        &outside.path().join("index.js"),
    )
    .await;

    let error = manager.reload().await.unwrap_err();

    assert!(error.to_string().contains("symlink"));
    assert!(Arc::ptr_eq(&previous, &manager.current_snapshot()));
}

#[tokio::test]
async fn snapshot_builds_only_active_packages() {
    let builtin_root = tempdir().unwrap();
    let managed_root = tempdir().unwrap();
    write_instruction_package(
        builtin_root.path(),
        "shared",
        "com.example.shared",
        "shared",
        "Built-in",
    )
    .await;
    let managed_package = write_instruction_package(
        managed_root.path(),
        "shared",
        "com.example.shared",
        "shared",
        "Managed",
    )
    .await;
    tokio::fs::write(managed_package.join("SKILL.md"), "invalid instructions")
        .await
        .unwrap();
    let manager = SkillManager::new(config(vec![
        Arc::new(DirectorySkillSource::new(
            SkillLayer::Builtin,
            builtin_root.path(),
        )),
        Arc::new(DirectorySkillSource::new(
            SkillLayer::Managed,
            managed_root.path(),
        )),
    ]))
    .await
    .unwrap();

    let snapshot = manager.current_snapshot();
    assert_eq!(snapshot.packages().len(), 1);
    assert_eq!(snapshot.inactive().len(), 1);
    assert_eq!(snapshot.catalog().summaries()[0].name, "shared");
}

#[tokio::test]
async fn package_targets_control_registry_and_catalog() {
    let root = tempdir().unwrap();
    let instruction = write_instruction_package(
        root.path(),
        "instructions",
        "com.example.instructions",
        "instructions",
        "Instructions",
    )
    .await;
    tokio::fs::write(instruction.join("skill.json"), "{invalid")
        .await
        .unwrap();
    let runtime =
        write_runtime_package(root.path(), "runtime", "com.example.runtime", "shared_tool").await;
    tokio::fs::write(
        runtime.join("SKILL.md"),
        skill_document("hidden", "Not included"),
    )
    .await
    .unwrap();

    let manager = test_manager(root.path()).await;
    let snapshot = manager.current_snapshot();

    assert_eq!(snapshot.registry().tools()[0].name, "shared_tool");
    assert_eq!(snapshot.catalog().summaries()[0].name, "instructions");
    assert_eq!(snapshot.catalog().summaries().len(), 1);
}

#[tokio::test]
async fn dynamic_android_snapshots_filter_runtime_manifest_capabilities_after_reload() {
    let root = tempdir().unwrap();
    write_runtime_package_with_capabilities(
        root.path(),
        "undeclared",
        "com.example.undeclared",
        "undeclared_tool",
        None,
    )
    .await;
    write_runtime_package_with_capabilities(
        root.path(),
        "network",
        "com.example.network",
        "network_tool",
        Some(&["network.http"]),
    )
    .await;
    let manager = SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(DirectorySkillSource::new(
            SkillLayer::Builtin,
            root.path(),
        ))],
        platform: PlatformId::Android,
        capabilities: CapabilitySet::android_mvp(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: Version::new(0, 3, 0),
    })
    .await
    .unwrap();

    assert!(manager.current_snapshot().registry().tools().is_empty());

    write_runtime_package_with_capabilities(
        root.path(),
        "undeclared",
        "com.example.undeclared",
        "undeclared_tool_v2",
        None,
    )
    .await;
    write_runtime_package_with_capabilities(
        root.path(),
        "network",
        "com.example.network",
        "network_tool_v2",
        Some(&["network.http"]),
    )
    .await;
    manager.reload().await.unwrap();

    assert_eq!(manager.current_snapshot().generation(), 2);
    assert!(manager.current_snapshot().registry().tools().is_empty());
}

#[tokio::test]
async fn android_descriptor_platform_resolution_is_stable_across_reload() {
    let root = tempdir().unwrap();
    write_instruction_package_with_platforms(
        root.path(),
        "android",
        "com.example.android",
        "android-only",
        "Android v1",
        &["android"],
    )
    .await;
    write_instruction_package_with_platforms(
        root.path(),
        "desktop",
        "com.example.desktop",
        "desktop-only",
        "Desktop v1",
        &["desktop"],
    )
    .await;
    let manager = SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(DirectorySkillSource::new(
            SkillLayer::Builtin,
            root.path(),
        ))],
        platform: PlatformId::Android,
        capabilities: CapabilitySet::android_mvp(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: Version::new(0, 3, 0),
    })
    .await
    .unwrap();

    assert_eq!(
        manager.current_snapshot().catalog().summaries()[0].name,
        "android-only"
    );
    assert_eq!(manager.current_snapshot().catalog().summaries().len(), 1);

    write_instruction_package_with_platforms(
        root.path(),
        "android",
        "com.example.android",
        "android-only",
        "Android v2",
        &["android"],
    )
    .await;
    write_instruction_package_with_platforms(
        root.path(),
        "desktop",
        "com.example.desktop",
        "desktop-only",
        "Desktop v2",
        &["desktop"],
    )
    .await;
    manager.reload().await.unwrap();

    let documents = manager
        .current_snapshot()
        .catalog()
        .load_instruction_documents(&["android-only".into()], usize::MAX)
        .await
        .unwrap();
    assert_eq!(manager.current_snapshot().generation(), 2);
    assert_eq!(manager.current_snapshot().catalog().summaries().len(), 1);
    assert!(documents[0].content.contains("Android v2"));
}

#[tokio::test]
async fn duplicate_instruction_names_reject_candidate_snapshot() {
    let root = tempdir().unwrap();
    write_instruction_package(
        root.path(),
        "first",
        "com.example.first",
        "duplicate",
        "First",
    )
    .await;
    write_instruction_package(
        root.path(),
        "second",
        "com.example.second",
        "duplicate",
        "Second",
    )
    .await;

    let error = SkillManager::new(config(vec![Arc::new(DirectorySkillSource::new(
        SkillLayer::Builtin,
        root.path(),
    ))]))
    .await
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("duplicate instruction skill name")
    );
}

#[tokio::test]
async fn duplicate_runtime_local_names_keep_distinct_canonical_snapshot_tools() {
    let root = tempdir().unwrap();
    write_runtime_package(root.path(), "first", "com.example.first", "duplicate_tool").await;
    write_runtime_package(
        root.path(),
        "second",
        "com.example.second",
        "duplicate_tool",
    )
    .await;

    let manager = SkillManager::new(config(vec![Arc::new(DirectorySkillSource::new(
        SkillLayer::Builtin,
        root.path(),
    ))]))
    .await
    .unwrap();
    let snapshot = manager.current_snapshot();
    let registry = snapshot.registry();
    let tools = registry.tools_with_runtime_sources();

    assert!(
        tools
            .iter()
            .any(|tool| tool.canonical_id == "com.example.first/duplicate_tool")
    );
    assert!(
        tools
            .iter()
            .any(|tool| tool.canonical_id == "com.example.second/duplicate_tool")
    );
    assert!(registry.resolve_runtime_tool("duplicate_tool").is_none());
}

#[tokio::test]
async fn reload_rejects_source_layer_mismatch_and_keeps_snapshot() {
    let root = tempdir().unwrap();
    write_instruction_package(root.path(), "first", "com.example.first", "first", "First").await;
    let directory = DirectorySkillSource::new(SkillLayer::Builtin, root.path());
    let packages = directory.discover().await.unwrap();
    let source = Arc::new(MutableSource::new(SkillLayer::Builtin, packages.clone()));
    let manager = SkillManager::new(config(vec![source.clone()]))
        .await
        .unwrap();
    let previous = manager.current_snapshot();
    let mut mismatched = packages;
    mismatched[0].layer = SkillLayer::Managed;
    source.replace(mismatched);

    let error = manager.reload().await.unwrap_err();

    assert!(error.to_string().contains("source layer"));
    assert!(Arc::ptr_eq(&previous, &manager.current_snapshot()));
}

#[tokio::test]
async fn concurrent_reloads_publish_monotonic_generations() {
    let root = tempdir().unwrap();
    write_instruction_package(root.path(), "first", "com.example.first", "first", "First").await;
    let manager = test_manager(root.path()).await;

    let (first, second) = tokio::join!(manager.reload(), manager.reload());
    let mut transitions = [first.unwrap(), second.unwrap()];
    transitions.sort_by_key(|report| report.active_generation);

    assert_eq!(transitions[0].previous_generation, 1);
    assert_eq!(transitions[0].active_generation, 2);
    assert_eq!(transitions[1].previous_generation, 2);
    assert_eq!(transitions[1].active_generation, 3);
    assert_eq!(manager.current_snapshot().generation(), 3);
}

#[tokio::test]
async fn static_manager_is_synchronous_and_clearly_not_reloadable() {
    let root = tempdir().unwrap();
    write_instruction_package(
        root.path(),
        "instructions",
        "com.example.instructions",
        "instructions",
        "Static instructions",
    )
    .await;
    write_runtime_package(root.path(), "runtime", "com.example.runtime", "static_tool").await;
    let registry = SkillRegistry::load_development(root.path()).await.unwrap();
    let catalog = SkillCatalog::load_development(root.path()).await.unwrap();

    let manager = SkillManager::from_registry_and_catalog(registry, catalog);
    let snapshot = manager.current_snapshot();
    let error = manager.reload().await.unwrap_err();

    assert_eq!(snapshot.generation(), 1);
    assert_eq!(snapshot.registry().tools()[0].name, "static_tool");
    assert_eq!(snapshot.catalog().summaries()[0].name, "instructions");
    assert!(
        error
            .to_string()
            .contains("static skill manager cannot reload")
    );
    assert!(Arc::ptr_eq(&snapshot, &manager.current_snapshot()));
}

#[test]
fn static_manager_factory_does_not_require_a_tokio_runtime() {
    let manager =
        SkillManager::from_registry_and_catalog(SkillRegistry::empty(), SkillCatalog::empty());

    assert_eq!(manager.current_snapshot().generation(), 1);
}

#[tokio::test]
async fn managers_expose_immutable_runtime_context() {
    let dynamic_root = tempdir().unwrap();
    let dynamic = SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(DirectorySkillSource::new(
            SkillLayer::Builtin,
            dynamic_root.path(),
        ))],
        platform: PlatformId::Android,
        capabilities: CapabilitySet::android_mvp(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: Version::new(0, 3, 0),
    })
    .await
    .unwrap();
    let dynamic_context = dynamic.runtime_context().unwrap();
    assert_eq!(dynamic_context.platform(), PlatformId::Android);
    assert_eq!(
        dynamic_context.capabilities(),
        &CapabilitySet::android_mvp()
    );

    let contextless =
        SkillManager::from_registry_and_catalog(SkillRegistry::empty(), SkillCatalog::empty());
    assert!(contextless.runtime_context().is_none());

    let static_root = tempdir().unwrap();
    write_runtime_package(
        static_root.path(),
        "runtime",
        "com.example.runtime",
        "unsafe_tool",
    )
    .await;
    let registry = SkillRegistry::load_development(static_root.path())
        .await
        .unwrap();
    let contextual = SkillManager::from_registry_and_catalog_with_context(
        registry,
        SkillCatalog::empty(),
        PlatformId::Android,
        CapabilitySet::android_mvp(),
    );
    let static_context = contextual.runtime_context().unwrap();
    assert_eq!(static_context.platform(), PlatformId::Android);
    assert_eq!(static_context.capabilities(), &CapabilitySet::android_mvp());
    assert!(contextual.current_snapshot().registry().tools().is_empty());
}

struct MutableSource {
    layer: SkillLayer,
    packages: RwLock<Vec<DiscoveredSkillPackage>>,
}

impl MutableSource {
    fn new(layer: SkillLayer, packages: Vec<DiscoveredSkillPackage>) -> Self {
        Self {
            layer,
            packages: RwLock::new(packages),
        }
    }

    fn replace(&self, packages: Vec<DiscoveredSkillPackage>) {
        *self.packages.write().unwrap() = packages;
    }
}

#[async_trait]
impl SkillSource for MutableSource {
    fn layer(&self) -> SkillLayer {
        self.layer
    }

    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        Ok(self.packages.read().unwrap().clone())
    }
}

async fn test_manager(root: &Path) -> SkillManager {
    SkillManager::new(config(vec![Arc::new(DirectorySkillSource::new(
        SkillLayer::Builtin,
        root,
    ))]))
    .await
    .unwrap()
}

fn config(sources: Vec<Arc<dyn SkillSource>>) -> SkillManagerConfig {
    SkillManagerConfig {
        sources,
        platform: PlatformId::Desktop,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: Version::new(0, 3, 0),
    }
}

async fn write_instruction_package(
    root: &Path,
    folder: &str,
    id: &str,
    name: &str,
    body: &str,
) -> std::path::PathBuf {
    let package_root = root.join(folder);
    tokio::fs::create_dir_all(&package_root).await.unwrap();
    tokio::fs::write(
        package_root.join("agentweave.json"),
        serde_json::json!({
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
    tokio::fs::write(package_root.join("SKILL.md"), skill_document(name, body))
        .await
        .unwrap();
    package_root
}

async fn write_instruction_package_with_platforms(
    root: &Path,
    folder: &str,
    id: &str,
    name: &str,
    body: &str,
    platforms: &[&str],
) -> std::path::PathBuf {
    let package_root = write_instruction_package(root, folder, id, name, body).await;
    tokio::fs::write(
        package_root.join("agentweave.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "displayName": name,
            "kind": "instruction_only",
            "package": {
                "includeInstructions": true,
                "includeRuntime": false
            },
            "compatibility": {
                "platforms": platforms
            }
        })
        .to_string(),
    )
    .await
    .unwrap();
    package_root
}

async fn write_runtime_package(
    root: &Path,
    folder: &str,
    id: &str,
    tool_name: &str,
) -> std::path::PathBuf {
    write_runtime_package_with_capabilities(root, folder, id, tool_name, None).await
}

async fn write_runtime_package_with_capabilities(
    root: &Path,
    folder: &str,
    id: &str,
    tool_name: &str,
    capabilities: Option<&[&str]>,
) -> std::path::PathBuf {
    let package_root = root.join(folder);
    tokio::fs::create_dir_all(&package_root).await.unwrap();
    tokio::fs::write(
        package_root.join("agentweave.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "displayName": folder,
            "kind": "native_runtime",
            "package": {
                "includeInstructions": false,
                "includeRuntime": true
            }
        })
        .to_string(),
    )
    .await
    .unwrap();
    let mut manifest = serde_json::json!({
        "name": folder,
        "description": "Runtime skill.",
        "version": "1.0.0",
        "entry": {
            "type": "command",
            "command": "node",
            "args": ["index.js"]
        },
        "tools": [{
            "name": tool_name,
            "description": "Test tool.",
            "input_schema": { "type": "object" }
        }]
    });
    if let Some(requires) = capabilities {
        manifest["capabilities"] = serde_json::json!({ "requires": requires });
    }
    tokio::fs::write(package_root.join("skill.json"), manifest.to_string())
        .await
        .unwrap();
    tokio::fs::write(package_root.join("index.js"), "process.stdin.resume();\n")
        .await
        .unwrap();
    package_root
}

async fn discovered_runtime_package(root: &Path) -> (std::path::PathBuf, DiscoveredSkillPackage) {
    let package_root =
        write_runtime_package(root, "runtime", "com.example.runtime", "runtime_tool").await;
    let mut packages = DirectorySkillSource::new(SkillLayer::Builtin, root)
        .discover()
        .await
        .unwrap();
    (package_root, packages.pop().unwrap())
}

#[cfg(unix)]
async fn replace_with_external_symlink(path: &Path, outside: &Path) {
    let content = tokio::fs::read(path).await.unwrap();
    tokio::fs::write(outside, content).await.unwrap();
    tokio::fs::remove_file(path).await.unwrap();
    std::os::unix::fs::symlink(outside, path).unwrap();
}

fn complete_catalog_entry(name: &str, content: &str) -> SkillCatalogEntry {
    let source = PathBuf::from(format!("{name}/SKILL.md"));
    SkillCatalogEntry {
        summary: SkillSummary {
            name: name.into(),
            description: "Test instructions.".into(),
            aliases: Vec::new(),
            source: source.clone(),
        },
        document: SkillInstructionDocument {
            name: name.into(),
            source,
            content: content.into(),
            truncated: false,
            read_bytes: content.len(),
            original_bytes: content.len(),
        },
    }
}

fn skill_document(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: Test instructions.\n---\n\n# {name}\n{body}\n")
}

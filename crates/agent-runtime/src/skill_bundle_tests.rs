use crate::platform::PlatformId;
#[cfg(unix)]
use crate::skill_bundle::gate_bundle_metadata_after_inspection;
use crate::skill_bundle::{
    BuildSkillBundleRequest, BundleSkillSource, SkillBundleCurrent, SkillBundleLock,
    build_skill_bundle, gate_bundle_after_inspection,
};
use semver::Version;
use std::path::{Path, PathBuf};

struct BundleFixture {
    temp: tempfile::TempDir,
    source_roots: Vec<PathBuf>,
    output_root: PathBuf,
}

impl BundleFixture {
    async fn with_packages(ids: &[&str]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        tokio::fs::create_dir_all(&source_root).await.unwrap();
        for id in ids {
            write_runtime_package(&source_root.join(id), id).await;
        }
        let output_root = temp.path().join("bundle");
        Self {
            temp,
            source_roots: vec![source_root],
            output_root,
        }
    }

    fn request(&self) -> BuildSkillBundleRequest {
        BuildSkillBundleRequest {
            source_roots: self.source_roots.clone(),
            output_root: self.output_root.clone(),
            platform: PlatformId::Desktop,
            runtime_version: Version::new(0, 1, 0),
            generated_at: "2026-01-02T03:04:05Z".to_string(),
        }
    }
}

#[tokio::test]
async fn bundle_manifest_and_lock_are_deterministic() {
    let fixture = BundleFixture::with_packages(&["com.example.beta", "com.example.alpha"]).await;
    let first = build_skill_bundle(fixture.request()).await.unwrap();
    let first_manifest = first.manifest_bytes.clone();
    let first_lock = first.lock_bytes.clone();
    let second = build_skill_bundle(fixture.request()).await.unwrap();

    assert_eq!(first_manifest, second.manifest_bytes);
    assert_eq!(first_lock, second.lock_bytes);
    assert!(first.manifest_bytes.ends_with(b"\n"));
    assert!(first.lock_bytes.ends_with(b"\n"));
    assert!(String::from_utf8_lossy(&first.manifest_bytes).contains("2026-01-02T03:04:05Z"));
    assert!(!String::from_utf8_lossy(&first.lock_bytes).contains("generatedAt"));
    let lock: SkillBundleLock = serde_json::from_slice(&first.lock_bytes).unwrap();
    assert_eq!(lock.packages[0].id.as_str(), "com.example.alpha");
    assert_eq!(lock.packages[1].id.as_str(), "com.example.beta");
    assert_eq!(first.package_count, 2);
    assert_eq!(first.root, fixture.output_root);
    assert!(fixture.temp.path().exists());
}

#[tokio::test]
async fn source_root_order_does_not_change_bundle_bytes() {
    let fixture = BundleFixture::with_packages(&[]).await;
    let first_root = fixture.temp.path().join("one");
    let second_root = fixture.temp.path().join("two");
    write_runtime_package(&first_root.join("com.example.alpha"), "com.example.alpha").await;
    write_runtime_package(&second_root.join("com.example.beta"), "com.example.beta").await;
    let mut request = fixture.request();
    request.source_roots = vec![first_root.clone(), second_root.clone()];
    let first = build_skill_bundle(request.clone()).await.unwrap();
    request.source_roots.reverse();
    let second = build_skill_bundle(request).await.unwrap();

    assert_eq!(first.manifest_bytes, second.manifest_bytes);
    assert_eq!(first.lock_bytes, second.lock_bytes);
}

#[tokio::test]
async fn packaged_source_rejects_modified_content() {
    let fixture = BundleFixture::with_packages(&["com.example.alpha"]).await;
    let output = build_skill_bundle(fixture.request()).await.unwrap();
    let generation = active_generation(&output.root).await;
    tokio::fs::write(generation.join("com.example.alpha/skill.json"), "changed")
        .await
        .unwrap();

    let error = BundleSkillSource::open(output.root).await.unwrap_err();

    assert!(error.to_string().contains("content hash mismatch"));
}

#[cfg(unix)]
#[tokio::test]
async fn bundle_open_rejects_manifest_replaced_by_symlink_after_inspection() {
    use std::os::unix::fs as unix_fs;

    let fixture = BundleFixture::with_packages(&["com.example.alpha"]).await;
    build_skill_bundle(fixture.request()).await.unwrap();
    let manifest = active_generation(&fixture.output_root)
        .await
        .join("skill-bundle.json");
    let inspected_path = tokio::fs::canonicalize(&manifest).await.unwrap();
    let outside = fixture.temp.path().join("outside-manifest.json");
    let gate = gate_bundle_metadata_after_inspection(&inspected_path);
    let bundle_root = fixture.output_root.clone();
    let opening = tokio::spawn(async move { BundleSkillSource::open(bundle_root).await });
    gate.wait_entered().await;
    tokio::fs::rename(&manifest, &outside).await.unwrap();
    unix_fs::symlink(&outside, &manifest).unwrap();
    gate.release().await;

    let error = opening.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("bundle metadata"));
}

#[tokio::test]
async fn builder_rejects_duplicate_ids_across_roots() {
    let fixture = BundleFixture::with_packages(&[]).await;
    let first = fixture.temp.path().join("first");
    let second = fixture.temp.path().join("second");
    write_runtime_package(&first.join("one"), "com.example.duplicate").await;
    write_runtime_package(&second.join("two"), "com.example.duplicate").await;
    let mut request = fixture.request();
    request.source_roots = vec![first, second];

    let error = build_skill_bundle(request).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("duplicate package id com.example.duplicate")
    );
}

#[tokio::test]
async fn builder_rejects_an_unresolved_package_set() {
    let fixture = BundleFixture::with_packages(&["com.example.alpha"]).await;
    let descriptor_path = fixture.source_roots[0].join("com.example.alpha/general-agent.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["requires"]["packages"] = serde_json::json!(["com.example.missing"]);
    tokio::fs::write(
        descriptor_path,
        serde_json::to_vec_pretty(&descriptor).unwrap(),
    )
    .await
    .unwrap();

    let error = build_skill_bundle(fixture.request()).await.unwrap_err();

    assert!(
        error.to_string().contains(
            "inactive package com.example.alpha: missing dependency: com.example.missing"
        )
    );
}

#[tokio::test]
async fn builder_rejects_source_output_overlap_in_both_directions() {
    let fixture = BundleFixture::with_packages(&["com.example.alpha"]).await;
    let mut nested_output = fixture.request();
    nested_output.output_root = fixture.source_roots[0].join("dist");
    let nested_error = build_skill_bundle(nested_output).await.unwrap_err();

    let outer_output = fixture.temp.path().join("outer");
    let nested_source = outer_output.join("source");
    write_runtime_package(&nested_source.join("com.example.beta"), "com.example.beta").await;
    let mut outer_request = fixture.request();
    outer_request.source_roots = vec![nested_source];
    outer_request.output_root = outer_output;
    let outer_error = build_skill_bundle(outer_request).await.unwrap_err();

    assert!(nested_error.to_string().contains("must not overlap"));
    assert!(outer_error.to_string().contains("must not overlap"));
}

#[tokio::test]
async fn builder_rejects_content_excluded_by_include_flags() {
    let fixture = BundleFixture::with_packages(&["com.example.alpha"]).await;
    let descriptor_path = fixture.source_roots[0].join("com.example.alpha/general-agent.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["package"]["includeRuntime"] = serde_json::Value::Bool(false);
    descriptor["kind"] = serde_json::Value::String("instruction_only".into());
    descriptor["package"]["includeInstructions"] = serde_json::Value::Bool(true);
    descriptor["requires"]["capabilities"] = serde_json::json!([]);
    tokio::fs::write(
        &descriptor_path,
        serde_json::to_vec_pretty(&descriptor).unwrap(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        fixture.source_roots[0].join("com.example.alpha/SKILL.md"),
        "---\nname: alpha\ndescription: Alpha.\n---\n",
    )
    .await
    .unwrap();

    let error = build_skill_bundle(fixture.request()).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("runtime include flag does not match")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn builder_rejects_symlinks_hardlinks_and_special_files() {
    use std::os::unix::fs as unix_fs;

    let symlink_fixture = BundleFixture::with_packages(&["com.example.symlink"]).await;
    unix_fs::symlink(
        "skill.json",
        symlink_fixture.source_roots[0].join("com.example.symlink/alias.json"),
    )
    .unwrap();
    let symlink_error = build_skill_bundle(symlink_fixture.request())
        .await
        .unwrap_err();

    let hardlink_fixture = BundleFixture::with_packages(&["com.example.hardlink"]).await;
    std::fs::hard_link(
        hardlink_fixture.source_roots[0].join("com.example.hardlink/skill.json"),
        hardlink_fixture.source_roots[0].join("com.example.hardlink/alias.json"),
    )
    .unwrap();
    let hardlink_error = build_skill_bundle(hardlink_fixture.request())
        .await
        .unwrap_err();

    let special_fixture = BundleFixture::with_packages(&["com.example.special"]).await;
    let fifo = special_fixture.source_roots[0].join("com.example.special/pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let special_error = build_skill_bundle(special_fixture.request())
        .await
        .unwrap_err();

    assert!(
        symlink_error
            .to_string()
            .contains("cannot contain symlinks")
    );
    assert!(
        hardlink_error
            .to_string()
            .contains("cannot contain hard links")
    );
    assert!(
        special_error
            .to_string()
            .contains("cannot contain special files")
    );
}

#[tokio::test]
async fn source_mutation_aborts_without_replacing_previous_output() {
    let fixture = BundleFixture::with_packages(&["com.example.alpha"]).await;
    build_skill_bundle(fixture.request()).await.unwrap();
    let previous_current = tokio::fs::read(fixture.output_root.join("current"))
        .await
        .unwrap();
    let gate = gate_bundle_after_inspection(&fixture.output_root);
    let request = fixture.request();
    let build = tokio::spawn(async move { build_skill_bundle(request).await });
    gate.wait_entered().await;
    tokio::fs::write(
        fixture.source_roots[0].join("com.example.alpha/index.js"),
        "changed\n",
    )
    .await
    .unwrap();
    gate.release().await;

    let error = build.await.unwrap().unwrap_err();

    assert!(
        format!("{error:#}").contains("changed"),
        "unexpected error: {error:#}"
    );
    assert_eq!(
        tokio::fs::read(fixture.output_root.join("current"))
            .await
            .unwrap(),
        previous_current
    );
}

#[tokio::test]
async fn bundle_open_rejects_schema_set_path_descriptor_and_hash_mismatches() {
    let schema = BundleFixture::with_packages(&["com.example.alpha"]).await;
    build_skill_bundle(schema.request()).await.unwrap();
    let schema_generation = active_generation(&schema.output_root).await;
    mutate_json(&schema_generation.join("skill-bundle.json"), |value| {
        value["schemaVersion"] = serde_json::json!(99);
    })
    .await;
    assert!(
        BundleSkillSource::open(&schema.output_root)
            .await
            .unwrap_err()
            .to_string()
            .contains("unsupported skill bundle manifest schema version")
    );

    let set = BundleFixture::with_packages(&["com.example.alpha"]).await;
    build_skill_bundle(set.request()).await.unwrap();
    let set_generation = active_generation(&set.output_root).await;
    mutate_json(&set_generation.join("skill-bundle.lock"), |value| {
        value["packages"] = serde_json::json!([]);
    })
    .await;
    assert!(
        BundleSkillSource::open(&set.output_root)
            .await
            .unwrap_err()
            .to_string()
            .contains("package sets do not match")
    );

    let path = BundleFixture::with_packages(&["com.example.alpha"]).await;
    build_skill_bundle(path.request()).await.unwrap();
    let path_generation = active_generation(&path.output_root).await;
    mutate_json(&path_generation.join("skill-bundle.json"), |value| {
        value["packages"][0]["path"] = serde_json::json!("../outside");
    })
    .await;
    let path_error = BundleSkillSource::open(&path.output_root)
        .await
        .unwrap_err();
    assert!(
        path_error.to_string().contains("relative normal"),
        "unexpected error: {path_error:#}"
    );

    let descriptor = BundleFixture::with_packages(&["com.example.alpha"]).await;
    build_skill_bundle(descriptor.request()).await.unwrap();
    let descriptor_generation = active_generation(&descriptor.output_root).await;
    mutate_json(&descriptor_generation.join("skill-bundle.json"), |value| {
        value["packages"][0]["displayName"] = serde_json::json!("wrong")
    })
    .await;
    assert!(
        BundleSkillSource::open(&descriptor.output_root)
            .await
            .unwrap_err()
            .to_string()
            .contains("descriptor does not match")
    );

    let hash = BundleFixture::with_packages(&["com.example.alpha"]).await;
    build_skill_bundle(hash.request()).await.unwrap();
    let hash_generation = active_generation(&hash.output_root).await;
    mutate_json(&hash_generation.join("skill-bundle.lock"), |value| {
        value["packages"][0]["contentHash"] = serde_json::json!("00");
    })
    .await;
    assert!(
        BundleSkillSource::open(&hash.output_root)
            .await
            .unwrap_err()
            .to_string()
            .contains("manifest and lock disagree")
    );
}

#[tokio::test]
async fn bundle_open_rejects_extra_unlocked_trees() {
    let fixture = BundleFixture::with_packages(&["com.example.alpha"]).await;
    build_skill_bundle(fixture.request()).await.unwrap();
    tokio::fs::create_dir(
        active_generation(&fixture.output_root)
            .await
            .join("com.example.extra"),
    )
    .await
    .unwrap();

    let error = BundleSkillSource::open(&fixture.output_root)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("unlocked content"));
}

#[tokio::test]
async fn bundle_open_rejects_noncanonical_package_order() {
    let fixture = BundleFixture::with_packages(&["com.example.alpha", "com.example.beta"]).await;
    build_skill_bundle(fixture.request()).await.unwrap();
    let generation = active_generation(&fixture.output_root).await;
    mutate_json(&generation.join("skill-bundle.json"), |value| {
        value["packages"].as_array_mut().unwrap().reverse();
    })
    .await;
    mutate_json(&generation.join("skill-bundle.lock"), |value| {
        value["packages"].as_array_mut().unwrap().reverse();
    })
    .await;

    let error = BundleSkillSource::open(&fixture.output_root)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("packages must be sorted"));
}

async fn mutate_json(path: &Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let mut value: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
    mutate(&mut value);
    let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
    bytes.push(b'\n');
    tokio::fs::write(path, bytes).await.unwrap();
}

async fn active_generation(output: &Path) -> PathBuf {
    let current: SkillBundleCurrent =
        serde_json::from_slice(&tokio::fs::read(output.join("current")).await.unwrap()).unwrap();
    output.join("generations").join(current.generation)
}

async fn write_runtime_package(root: &Path, id: &str) {
    tokio::fs::create_dir_all(root).await.unwrap();
    let descriptor = serde_json::json!({
        "schemaVersion": 1,
        "id": id,
        "version": "0.1.0",
        "displayName": id,
        "kind": "native_runtime",
        "package": {
            "includeInstructions": false,
            "includeRuntime": true
        },
        "compatibility": {
            "platforms": ["desktop", "server"]
        },
        "requires": {
            "packages": [],
            "capabilities": ["shell.process"],
            "runtimeTools": [],
            "connectors": []
        }
    });
    tokio::fs::write(
        root.join("general-agent.json"),
        serde_json::to_vec_pretty(&descriptor).unwrap(),
    )
    .await
    .unwrap();
    let runtime = serde_json::json!({
        "name": id,
        "description": "Fixture runtime skill.",
        "version": "0.1.0",
        "entry": { "type": "command", "command": "node", "args": ["index.js"] },
        "tools": [{
            "name": "echo",
            "description": "Echo input.",
            "input_schema": { "type": "object" }
        }]
    });
    tokio::fs::write(
        root.join("skill.json"),
        serde_json::to_vec(&runtime).unwrap(),
    )
    .await
    .unwrap();
    tokio::fs::write(root.join("index.js"), "process.stdout.write('{}')\n")
        .await
        .unwrap();
}

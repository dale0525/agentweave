use super::*;
use crate::skill_source::hash_package_tree;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

struct ResourceFixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    reader: SkillResourceReader,
}

impl ResourceFixture {
    async fn new(files: &[(&str, &[u8])]) -> Self {
        let known_files = files
            .iter()
            .map(|(path, _)| PathBuf::from(path))
            .collect::<Vec<_>>();
        Self::with_known_files(files, known_files).await
    }

    async fn with_known_files(files: &[(&str, &[u8])], known_files: Vec<PathBuf>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        for (relative, bytes) in files {
            let path = root.join(relative);
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(path, bytes).await.unwrap();
        }
        let content_hash = hash_package_tree(&root).await.unwrap();
        let revision = SkillResourceRevision::from_verified_revision(
            17,
            SkillPackageId::parse("dev.example.resources").unwrap(),
            "revision-1",
            root.clone(),
            content_hash,
            known_files,
            SkillStoreLimits::default(),
        )
        .unwrap();
        let reader = SkillResourceReader::new(revision, SkillResourceLimits::default()).unwrap();
        Self {
            _directory: directory,
            root,
            reader,
        }
    }
}

#[test]
fn resource_paths_are_strictly_portable_and_relative() {
    for path in [
        "",
        "/references/a.md",
        "../references/a.md",
        "references/../a.md",
        r"references\a.md",
        r"C:\references\a.md",
        "references/CON",
        "references/a?.md",
        "./references/a.md",
    ] {
        assert!(SkillResourcePath::parse(path).is_err(), "accepted {path}");
    }

    let path = SkillResourcePath::parse("references/指南.md").unwrap();
    assert_eq!(path.canonical(), "references/指南.md");
}

#[tokio::test]
async fn reads_revision_bound_reference_with_stable_metadata_and_hash() {
    let fixture = ResourceFixture::new(&[("references/guide.md", "hello 世界".as_bytes())]).await;
    let path = SkillResourcePath::parse("references/guide.md").unwrap();
    let resource = fixture
        .reader
        .read(SkillResourceKind::Reference, &path)
        .await
        .unwrap();

    assert_eq!(resource.metadata().snapshot_generation, 17);
    assert_eq!(
        resource.metadata().package_id.as_str(),
        "dev.example.resources"
    );
    assert_eq!(resource.metadata().revision_id, "revision-1");
    assert_eq!(resource.metadata().path, "references/guide.md");
    assert_eq!(resource.metadata().byte_len, 12);
    assert_eq!(
        resource.metadata().sha256,
        hex::encode(Sha256::digest("hello 世界".as_bytes()))
    );
    assert!(resource.metadata().media.is_none());
    assert_eq!(
        resource.content(),
        &SkillResourceContent::Text("hello 世界".into())
    );
}

#[tokio::test]
async fn resource_kind_is_bound_to_its_package_directory() {
    let fixture = ResourceFixture::new(&[
        ("references/guide.md", b"guide"),
        ("scripts/helper.py", b"print('ok')"),
        ("assets/blob.bin", b"\x00\xff"),
    ])
    .await;

    let reference = SkillResourcePath::parse("references/guide.md").unwrap();
    let error = fixture
        .reader
        .read(SkillResourceKind::Script, &reference)
        .await
        .unwrap_err();
    assert!(matches!(error, SkillResourceError::KindPathMismatch { .. }));

    let script = SkillResourcePath::parse("scripts/helper.py").unwrap();
    let script = fixture
        .reader
        .read(SkillResourceKind::Script, &script)
        .await
        .unwrap();
    assert!(matches!(script.content(), SkillResourceContent::Text(_)));

    let asset = SkillResourcePath::parse("assets/blob.bin").unwrap();
    let asset = fixture
        .reader
        .read(SkillResourceKind::Asset, &asset)
        .await
        .unwrap();
    assert_eq!(asset.content(), &SkillResourceContent::Binary(vec![0, 255]));
}

#[tokio::test]
async fn rejects_files_not_captured_by_the_verified_revision() {
    let fixture =
        ResourceFixture::with_known_files(&[("references/new.md", b"unverified")], Vec::new())
            .await;
    let path = SkillResourcePath::parse("references/new.md").unwrap();
    let error = fixture
        .reader
        .read(SkillResourceKind::Reference, &path)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SkillResourceError::ResourceNotInRevision { .. }
    ));
}

#[tokio::test]
async fn rejects_invalid_utf8_and_nul_in_text_resources() {
    let fixture = ResourceFixture::new(&[
        ("references/bad.md", b"\xff"),
        ("scripts/zero.py", b"print('a')\0print('b')"),
    ])
    .await;
    let invalid = SkillResourcePath::parse("references/bad.md").unwrap();
    assert!(matches!(
        fixture
            .reader
            .read(SkillResourceKind::Reference, &invalid)
            .await
            .unwrap_err(),
        SkillResourceError::InvalidUtf8 { .. }
    ));
    let nul = SkillResourcePath::parse("scripts/zero.py").unwrap();
    assert!(matches!(
        fixture
            .reader
            .read(SkillResourceKind::Script, &nul)
            .await
            .unwrap_err(),
        SkillResourceError::TextContainsNul(_)
    ));
}

#[tokio::test]
async fn enforces_per_kind_byte_and_character_limits() {
    let fixture = ResourceFixture::new(&[("references/large.md", b"abcdef")]).await;
    let revision = fixture.reader.revision().clone();
    let limits = SkillResourceLimits {
        max_reference_bytes: 5,
        ..SkillResourceLimits::default()
    };
    let reader = SkillResourceReader::new(revision.clone(), limits).unwrap();
    let path = SkillResourcePath::parse("references/large.md").unwrap();
    assert!(matches!(
        reader
            .read(SkillResourceKind::Reference, &path)
            .await
            .unwrap_err(),
        SkillResourceError::ByteLimitExceeded { .. }
    ));

    let limits = SkillResourceLimits {
        max_reference_chars: 5,
        ..SkillResourceLimits::default()
    };
    let reader = SkillResourceReader::new(revision, limits).unwrap();
    assert!(matches!(
        reader
            .read(SkillResourceKind::Reference, &path)
            .await
            .unwrap_err(),
        SkillResourceError::CharacterLimitExceeded { .. }
    ));
}

#[tokio::test]
async fn rejects_revision_content_drift_before_reading() {
    let fixture = ResourceFixture::new(&[("references/guide.md", b"original")]).await;
    tokio::fs::write(fixture.root.join("references/guide.md"), b"changed")
        .await
        .unwrap();
    let path = SkillResourcePath::parse("references/guide.md").unwrap();
    let error = fixture
        .reader
        .read(SkillResourceKind::Reference, &path)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SkillResourceError::RevisionContentMismatch { .. }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlinked_resource_leafs_and_parent_escapes() {
    use std::os::unix::fs::symlink;

    let leaf = ResourceFixture::new(&[("references/guide.md", b"original")]).await;
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::remove_file(leaf.root.join("references/guide.md")).unwrap();
    symlink(outside.path(), leaf.root.join("references/guide.md")).unwrap();
    let path = SkillResourcePath::parse("references/guide.md").unwrap();
    assert!(matches!(
        leaf.reader
            .read(SkillResourceKind::Reference, &path)
            .await
            .unwrap_err(),
        SkillResourceError::RevisionInspection { .. }
    ));

    let parent = ResourceFixture::new(&[("references/guide.md", b"original")]).await;
    let outside_dir = tempfile::tempdir().unwrap();
    std::fs::write(outside_dir.path().join("guide.md"), b"outside").unwrap();
    std::fs::remove_dir_all(parent.root.join("references")).unwrap();
    symlink(outside_dir.path(), parent.root.join("references")).unwrap();
    assert!(matches!(
        parent
            .reader
            .read(SkillResourceKind::Reference, &path)
            .await
            .unwrap_err(),
        SkillResourceError::RevisionInspection { .. }
    ));
}

#[tokio::test]
async fn returns_bounded_media_metadata_and_rejects_oversized_images() {
    let png = png_header(640, 480);
    let fixture = ResourceFixture::new(&[("assets/image.png", &png)]).await;
    let path = SkillResourcePath::parse("assets/image.png").unwrap();
    let resource = fixture
        .reader
        .read(SkillResourceKind::Asset, &path)
        .await
        .unwrap();
    assert_eq!(
        resource.metadata().media,
        Some(SkillMediaMetadata {
            mime_type: "image/png".into(),
            image_dimensions: Some(SkillImageDimensions {
                width: 640,
                height: 480,
            }),
        })
    );

    let limits = SkillResourceLimits {
        max_image_dimension: 500,
        ..SkillResourceLimits::default()
    };
    let reader = SkillResourceReader::new(fixture.reader.revision().clone(), limits).unwrap();
    assert!(matches!(
        reader
            .read(SkillResourceKind::Asset, &path)
            .await
            .unwrap_err(),
        SkillResourceError::ImageDimensionsExceeded { .. }
    ));
}

#[tokio::test]
async fn disabled_helper_executor_fails_closed_without_a_shell_fallback() {
    let fixture = ResourceFixture::new(&[("scripts/helper.py", b"print('ok')")]).await;
    let path = SkillResourcePath::parse("scripts/helper.py").unwrap();
    let script = fixture
        .reader
        .read(SkillResourceKind::Script, &path)
        .await
        .unwrap();
    let request = SandboxSkillHelperRequest::new(
        SandboxSkillHelperRuntime::Python,
        script,
        vec!["--format=json".into()],
        Vec::new(),
        1_000,
        1024,
        SandboxSkillHelperLimits::default(),
    )
    .unwrap();
    assert_eq!(request.runtime(), SandboxSkillHelperRuntime::Python);
    assert_eq!(
        DisabledSandboxSkillHelperExecutor
            .execute(&request)
            .await
            .unwrap_err(),
        SandboxSkillHelperError::Disabled
    );
}

#[tokio::test]
async fn helper_request_rejects_runtime_mismatch_and_binary_assets() {
    let fixture = ResourceFixture::new(&[
        ("scripts/helper.py", b"print('ok')"),
        ("assets/blob.bin", b"binary"),
    ])
    .await;
    let script_path = SkillResourcePath::parse("scripts/helper.py").unwrap();
    let script = fixture
        .reader
        .read(SkillResourceKind::Script, &script_path)
        .await
        .unwrap();
    assert!(matches!(
        SandboxSkillHelperRequest::new(
            SandboxSkillHelperRuntime::JavaScript,
            script,
            Vec::new(),
            Vec::new(),
            1_000,
            1024,
            SandboxSkillHelperLimits::default(),
        ),
        Err(SandboxSkillHelperError::InvalidRequest(_))
    ));

    let asset_path = SkillResourcePath::parse("assets/blob.bin").unwrap();
    let asset = fixture
        .reader
        .read(SkillResourceKind::Asset, &asset_path)
        .await
        .unwrap();
    assert!(matches!(
        SandboxSkillHelperRequest::new(
            SandboxSkillHelperRuntime::Python,
            asset,
            Vec::new(),
            Vec::new(),
            1_000,
            1024,
            SandboxSkillHelperLimits::default(),
        ),
        Err(SandboxSkillHelperError::InvalidRequest(_))
    ));
}

#[test]
fn helper_output_requires_an_enforced_combined_cap() {
    assert!(SandboxSkillHelperOutput::bounded(0, vec![1; 4], vec![2; 4], 8).is_ok());
    assert_eq!(
        SandboxSkillHelperOutput::bounded(0, vec![1; 5], vec![2; 4], 8).unwrap_err(),
        SandboxSkillHelperError::InvalidOutput("helper output exceeds 8 bytes".into())
    );
}

fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes
}

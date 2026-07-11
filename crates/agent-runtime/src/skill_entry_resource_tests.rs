use crate::skill::{SkillManifest, manifest_entry_resources, validate_manifest_semantics};
use serde_json::json;
use std::path::PathBuf;

#[test]
fn manifest_entry_resources_normalize_both_portable_separators() {
    let manifest = runtime_manifest_with_args(vec![
        "index.js",
        "dir/file",
        r"dir\file",
        "./dir/./file",
        r".\dir/.\file",
    ]);

    assert_eq!(
        manifest_entry_resources(&manifest).collect::<Vec<_>>(),
        vec![
            PathBuf::from("index.js"),
            PathBuf::from("dir/file"),
            PathBuf::from("dir/file"),
            PathBuf::from("dir/file"),
            PathBuf::from("dir/file"),
        ]
    );
}

#[test]
fn manifest_entry_resources_keep_host_process_data_opaque() {
    let manifest = runtime_manifest_with_args(vec![
        "",
        "--config",
        "value",
        "/host/config.json",
        r"\host\config.json",
        r"C:host\config.json",
        r"C:\host\config.json",
        r"\\server\share\config.json",
        r"\\?\C:\host\config.json",
        r"\\?\UNC\server\share\config.json",
    ]);

    validate_manifest_semantics(&manifest).unwrap();
    assert!(manifest_entry_resources(&manifest).next().is_none());
}

#[test]
fn manifest_semantics_reject_parent_nul_and_empty_relative_resources() {
    for arg in [
        "../x",
        r"..\x",
        "dir/../x",
        r"dir\..\x",
        r"dir/..\x",
        "./",
        r".\",
        "bad\0.js",
    ] {
        let error =
            validate_manifest_semantics(&runtime_manifest_with_args(vec![arg])).expect_err(arg);

        assert!(
            error
                .to_string()
                .contains(&format!("unsafe skill entry resource path: {arg}")),
            "{arg}: {error:#}"
        );
    }
}

fn runtime_manifest_with_args(args: Vec<&str>) -> SkillManifest {
    serde_json::from_value(json!({
        "name": "portable-resources",
        "description": "portable resource test",
        "version": "1.0.0",
        "entry": {"type": "command", "command": "node", "args": args},
        "tools": [{
            "name": "verified_tool",
            "description": "verified tool",
            "input_schema": {"type": "object"}
        }]
    }))
    .unwrap()
}

use agent_runtime::{
    platform::PlatformId,
    skill_bundle::{BuildSkillBundleRequest, build_skill_bundle},
};
use std::path::PathBuf;

const USAGE: &str = "Usage: bundle-skills --source <path> [--source <path> ...] --output <path> --platform <desktop|server|android|ios|web>";

#[derive(Debug)]
struct BundleArgs {
    source_roots: Vec<PathBuf>,
    output_root: PathBuf,
    platform: PlatformId,
}

#[derive(Debug)]
enum ParseResult {
    Args(BundleArgs),
    Help,
}

#[tokio::main]
async fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(ParseResult::Args(args)) => args,
        Ok(ParseResult::Help) => {
            println!("{USAGE}");
            return;
        }
        Err(error) => {
            eprintln!("bundle-skills: {error}");
            std::process::exit(2);
        }
    };
    let request = BuildSkillBundleRequest {
        source_roots: args.source_roots,
        output_root: args.output_root,
        platform: args.platform,
        runtime_version: env!("CARGO_PKG_VERSION")
            .parse()
            .expect("package version must be semver"),
        generated_at: chrono::Utc::now().to_rfc3339(),
    };
    match build_skill_bundle(request).await {
        Ok(result) => println!(
            "bundled {} package(s) into {}",
            result.package_count,
            result.root.display()
        ),
        Err(error) => {
            eprintln!("bundle-skills: {error:#}");
            std::process::exit(1);
        }
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<ParseResult> {
    let mut args = args.into_iter();
    let mut sources = Vec::new();
    let mut output = None;
    let mut platform = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--source" => sources.push(PathBuf::from(required_value(
                &mut args, "--source", "a path",
            )?)),
            "--output" => {
                anyhow::ensure!(output.is_none(), "--output may be provided only once");
                output = Some(PathBuf::from(required_value(
                    &mut args, "--output", "a path",
                )?));
            }
            "--platform" => {
                anyhow::ensure!(platform.is_none(), "--platform may be provided only once");
                let value = required_value(&mut args, "--platform", "a value")?;
                platform = Some(parse_platform(&value)?);
            }
            "--help" | "-h" => return Ok(ParseResult::Help),
            other if other.starts_with('-') => anyhow::bail!("unknown argument: {other}"),
            other => anyhow::bail!("unexpected positional argument: {other}"),
        }
    }
    anyhow::ensure!(!sources.is_empty(), "missing required --source <path>");
    let output_root = output.context("missing required --output <path>")?;
    let platform =
        platform.context("missing required --platform <desktop|server|android|ios|web>")?;
    Ok(ParseResult::Args(BundleArgs {
        source_roots: sources,
        output_root,
        platform,
    }))
}

fn required_value(
    args: &mut impl Iterator<Item = String>,
    option: &str,
    label: &str,
) -> anyhow::Result<String> {
    let value = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| anyhow::anyhow!("{option} requires {label}"))?;
    anyhow::ensure!(!value.is_empty(), "{option} requires {label}");
    Ok(value)
}

fn parse_platform(value: &str) -> anyhow::Result<PlatformId> {
    match value {
        "desktop" => Ok(PlatformId::Desktop),
        "server" => Ok(PlatformId::Server),
        "android" => Ok(PlatformId::Android),
        "ios" => Ok(PlatformId::Ios),
        "web" => Ok(PlatformId::Web),
        other => anyhow::bail!("unsupported platform: {other}"),
    }
}

use anyhow::Context as _;

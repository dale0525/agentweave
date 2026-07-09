use agent_server::dev_skills::{DevSkillPackage, SkillPackageReleaseError, check_skill_packages};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = skills_root_from_args()?;

    match check_skill_packages(&root).await {
        Ok(inventory) => {
            println!(
                "Skill release check passed: {} package(s) ready under {}",
                inventory.packages.len(),
                inventory.root
            );
            Ok(())
        }
        Err(error) => {
            print_release_error(&error);
            std::process::exit(1);
        }
    }
}

fn skills_root_from_args() -> anyhow::Result<PathBuf> {
    let mut args = std::env::args().skip(1);
    let mut root = PathBuf::from("skills");

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => {
                let Some(value) = args.next() else {
                    anyhow::bail!("--root requires a path");
                };
                root = PathBuf::from(value);
            }
            "--help" | "-h" => {
                println!("Usage: check-skills [--root <skills-root>]");
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    Ok(root)
}

fn print_release_error(error: &SkillPackageReleaseError) {
    eprintln!("{error}");
    for package in error
        .inventory
        .packages
        .iter()
        .filter(|package| !package.release_ready)
    {
        print_package_issues(package);
    }
}

fn print_package_issues(package: &DevSkillPackage) {
    eprintln!("- {} ({})", package.id, package.package_kind_label());
    for error in &package.validation.errors {
        eprintln!("  validation: {error}");
    }
    for issue in &package.readiness_issues {
        eprintln!("  readiness: {issue}");
    }
}

trait PackageKindLabel {
    fn package_kind_label(&self) -> &'static str;
}

impl PackageKindLabel for DevSkillPackage {
    fn package_kind_label(&self) -> &'static str {
        match self.package_kind {
            agent_server::dev_skills::DevSkillPackageKind::Runtime => "runtime",
            agent_server::dev_skills::DevSkillPackageKind::Instruction => "instruction",
            agent_server::dev_skills::DevSkillPackageKind::Combined => "combined",
            agent_server::dev_skills::DevSkillPackageKind::Empty => "empty",
            agent_server::dev_skills::DevSkillPackageKind::Invalid => "invalid",
        }
    }
}

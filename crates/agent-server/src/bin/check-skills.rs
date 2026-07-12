use agent_server::skill_release::{SkillReleaseDiagnostic, validate_skill_roots};
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let roots = match roots_from_args(std::env::args().skip(1)) {
        Ok(Some(roots)) => roots,
        Ok(None) => return,
        Err(error) => {
            eprintln!("check-skills: {error}");
            std::process::exit(2);
        }
    };
    let report = validate_skill_roots(&roots).await;
    for warning in &report.warnings {
        eprintln!(
            "warning: {}: {}",
            warning.message,
            diagnostic_location(warning)
        );
    }
    if report.is_ready() {
        println!(
            "Skill release check passed: {} package(s) across {} root(s)",
            report.package_count,
            report.roots.len()
        );
        return;
    }
    eprintln!("skill release check failed");
    for error in &report.errors {
        eprintln!("- {}", format_diagnostic(error));
    }
    std::process::exit(1);
}

fn roots_from_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Vec<PathBuf>>> {
    let mut args = args.into_iter();
    let mut roots = Vec::new();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => {
                let value = args
                    .next()
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| anyhow::anyhow!("--root requires a path"))?;
                roots.push(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!("Usage: check-skills [--root <skills-root> ...]");
                return Ok(None);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    if roots.is_empty() {
        roots.push(PathBuf::from("skills"));
    }
    Ok(Some(roots))
}

fn format_diagnostic(diagnostic: &SkillReleaseDiagnostic) -> String {
    match &diagnostic.package_id {
        Some(package) => format!(
            "{} at {}: {}",
            package,
            diagnostic.path.display(),
            diagnostic.message
        ),
        None => format!("{}: {}", diagnostic.path.display(), diagnostic.message),
    }
}

fn diagnostic_location(diagnostic: &SkillReleaseDiagnostic) -> String {
    match &diagnostic.package_id {
        Some(package) => format!("{} at {}", package, diagnostic.path.display()),
        None => diagnostic.path.display().to_string(),
    }
}

use agent_runtime::credential::{CredentialScope, SecretId, SecretMaterial, SecretStore};
use agent_runtime::credential_file::EncryptedFileSecretStore;
use std::io::Read;

const USAGE: &str = "Usage: store-secret --app-id <id> --secret-id <id> [--tenant-id <id>] [--user-id <id>] [--rotate]";

#[derive(Debug, PartialEq, Eq)]
struct Args {
    app_id: String,
    tenant_id: String,
    user_id: String,
    secret_id: String,
    rotate: bool,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(std::env::args().skip(1)).await {
        eprintln!("store-secret: {error}");
        std::process::exit(1);
    }
}

async fn run(args: impl IntoIterator<Item = String>) -> anyhow::Result<()> {
    let args = parse_args(args)?;
    let root = std::env::var("AGENTWEAVE_SECRET_ROOT")
        .map_err(|_| anyhow::anyhow!("AGENTWEAVE_SECRET_ROOT is required"))?;
    let key = std::env::var("AGENTWEAVE_SECRET_MASTER_KEY_HEX")
        .map_err(|_| anyhow::anyhow!("AGENTWEAVE_SECRET_MASTER_KEY_HEX is required"))?;
    let key = hex::decode(key)?;
    anyhow::ensure!(key.len() == 32, "secret master key must decode to 32 bytes");
    let scope = CredentialScope {
        app_id: args.app_id,
        tenant_id: args.tenant_id,
        user_id: args.user_id,
    };
    scope.validate()?;
    let secret_id = SecretId::parse(&args.secret_id)?;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        bytes.pop();
    }
    anyhow::ensure!(!bytes.is_empty(), "secret value is required on stdin");
    anyhow::ensure!(bytes.len() <= 64 * 1024, "secret value is too large");
    let store = EncryptedFileSecretStore::new(root, SecretMaterial::new(key)?)?;
    let value = SecretMaterial::new(bytes)?;
    if args.rotate {
        store.rotate(&scope, &secret_id, value).await?;
    } else {
        store.save(&scope, &secret_id, value).await?;
    }
    println!("Stored opaque secret reference {}", secret_id.as_str());
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<Args> {
    let mut app_id = None;
    let mut tenant_id = "local".to_string();
    let mut user_id = "local-user".to_string();
    let mut secret_id = None;
    let mut rotate = false;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let target = match argument.as_str() {
            "--app-id" => &mut app_id,
            "--tenant-id" => {
                tenant_id = required_value(&mut args, "--tenant-id")?;
                continue;
            }
            "--user-id" => {
                user_id = required_value(&mut args, "--user-id")?;
                continue;
            }
            "--secret-id" => &mut secret_id,
            "--rotate" => {
                rotate = true;
                continue;
            }
            "--help" | "-h" => anyhow::bail!(USAGE),
            other => anyhow::bail!("unknown argument '{other}'\n{USAGE}"),
        };
        anyhow::ensure!(target.is_none(), "{argument} may be provided only once");
        *target = Some(required_value(&mut args, &argument)?);
    }
    Ok(Args {
        app_id: app_id.ok_or_else(|| anyhow::anyhow!("--app-id is required\n{USAGE}"))?,
        tenant_id,
        user_id,
        secret_id: secret_id.ok_or_else(|| anyhow::anyhow!("--secret-id is required\n{USAGE}"))?,
        rotate,
    })
}

fn required_value(args: &mut impl Iterator<Item = String>, option: &str) -> anyhow::Result<String> {
    let value = args
        .next()
        .filter(|value| !value.trim().is_empty() && !value.starts_with('-'))
        .ok_or_else(|| anyhow::anyhow!("{option} requires a value"))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_defaults_local_scope_and_supports_rotation() {
        let args = parse_args([
            "--app-id".into(),
            "com.example.secretary".into(),
            "--secret-id".into(),
            "mail.primary.password".into(),
            "--rotate".into(),
        ])
        .unwrap();
        assert_eq!(args.tenant_id, "local");
        assert_eq!(args.user_id, "local-user");
        assert!(args.rotate);
    }
}

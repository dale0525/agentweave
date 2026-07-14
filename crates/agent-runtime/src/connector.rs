use crate::connector_ledger::{
    ConnectorActionLedger, ConnectorLedgerEntry, InMemoryConnectorActionLedger,
};
use crate::credential::{CredentialScope, CredentialVault, SecretMaterial};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorTransportKind {
    LocalHost,
    McpStdio,
    McpStreamableHttp,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorApprovalMode {
    Auto,
    Prompt,
    Writes,
    Explicit,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorToolRisk {
    Read,
    SensitiveRead,
    PersistentWrite,
    Write,
    DestructiveWrite,
}

impl ConnectorToolRisk {
    pub fn is_write(self) -> bool {
        matches!(
            self,
            Self::PersistentWrite | Self::Write | Self::DestructiveWrite
        )
    }

    pub fn requires_external_approval(self) -> bool {
        matches!(self, Self::Write | Self::DestructiveWrite)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConnectorToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub risk: ConnectorToolRisk,
    #[serde(default)]
    pub required_scopes: BTreeSet<String>,
    #[serde(default)]
    pub parallel_safe: bool,
    #[serde(default)]
    pub supports_idempotency: bool,
}

impl ConnectorToolSpec {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_name(&self.name, "connector tool name")?;
        anyhow::ensure!(
            !self.description.trim().is_empty(),
            "tool description is required"
        );
        anyhow::ensure!(
            self.input_schema.get("type").and_then(Value::as_str) == Some("object"),
            "connector tool input schema must describe an object"
        );
        anyhow::ensure!(
            !self.risk.is_write() || self.supports_idempotency,
            "write connector tools must declare idempotency support"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConnectorDescriptor {
    pub id: String,
    pub name: String,
    pub version: String,
    pub instructions: Option<String>,
    pub transport: ConnectorTransportKind,
    pub required_startup: bool,
    pub account_required: bool,
    pub approval_mode: ConnectorApprovalMode,
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,
    #[serde(default)]
    pub denied_tools: BTreeSet<String>,
}

impl ConnectorDescriptor {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_name(&self.id, "connector id")?;
        anyhow::ensure!(!self.name.trim().is_empty(), "connector name is required");
        self.version
            .parse::<semver::Version>()
            .map_err(|_| anyhow::anyhow!("connector version must be semantic"))?;
        anyhow::ensure!(
            self.allowed_tools.is_disjoint(&self.denied_tools),
            "connector allow and deny lists overlap"
        );
        Ok(())
    }

    fn tool_is_enabled(&self, tool: &str) -> bool {
        !self.denied_tools.contains(tool)
            && (self.allowed_tools.is_empty() || self.allowed_tools.contains(tool))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorHealth {
    Ready,
    Degraded,
    Unavailable,
}

pub struct ConnectorTransportCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub account_id: Option<String>,
    pub credential: Option<SecretMaterial>,
    pub idempotency_key: Option<String>,
    pub cancellation: CancellationToken,
}

impl std::fmt::Debug for ConnectorTransportCall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectorTransportCall")
            .field("call_id", &self.call_id)
            .field("tool_name", &self.tool_name)
            .field("arguments", &self.arguments)
            .field("account_id", &self.account_id)
            .field("credential", &self.credential)
            .field("idempotency_key", &self.idempotency_key)
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait ConnectorTransport: Send + Sync {
    async fn start(&self) -> anyhow::Result<()>;
    async fn list_tools(&self) -> anyhow::Result<Vec<ConnectorToolSpec>>;
    async fn call(&self, request: ConnectorTransportCall) -> anyhow::Result<Value>;
    async fn health(&self) -> anyhow::Result<ConnectorHealth>;
    async fn stop(&self) -> anyhow::Result<()>;
}

#[derive(Clone)]
struct RegisteredConnector {
    descriptor: ConnectorDescriptor,
    transport: Arc<dyn ConnectorTransport>,
    tools: BTreeMap<String, ConnectorToolSpec>,
}

#[derive(Clone, Debug)]
pub struct ConnectorCallContext {
    pub call_id: String,
    pub credential_scope: CredentialScope,
    pub account_id: Option<String>,
    pub approved_action_hash: Option<String>,
    pub idempotency_key: Option<String>,
    pub timeout: Duration,
    pub cancellation: CancellationToken,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ConnectorExecutionResult {
    pub connector_id: String,
    pub tool_name: String,
    pub action_hash: String,
    pub replayed: bool,
    pub output: Value,
}

#[derive(Clone)]
pub struct ConnectorRuntime {
    connectors: Arc<RwLock<BTreeMap<String, RegisteredConnector>>>,
    vault: Option<CredentialVault>,
    ledger: Arc<dyn ConnectorActionLedger>,
    max_output_bytes: usize,
}

impl ConnectorRuntime {
    pub fn new(vault: Option<CredentialVault>, max_output_bytes: usize) -> anyhow::Result<Self> {
        Self::new_with_ledger(
            vault,
            Arc::new(InMemoryConnectorActionLedger::default()),
            max_output_bytes,
        )
    }

    pub fn new_with_ledger(
        vault: Option<CredentialVault>,
        ledger: Arc<dyn ConnectorActionLedger>,
        max_output_bytes: usize,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            max_output_bytes > 0,
            "connector output limit must be positive"
        );
        Ok(Self {
            connectors: Arc::new(RwLock::new(BTreeMap::new())),
            vault,
            ledger,
            max_output_bytes,
        })
    }

    pub async fn register(
        &self,
        descriptor: ConnectorDescriptor,
        transport: Arc<dyn ConnectorTransport>,
    ) -> anyhow::Result<()> {
        descriptor.validate()?;
        if let Err(error) = transport.start().await
            && descriptor.required_startup
        {
            return Err(error);
        }
        let tools = load_tools(&descriptor, transport.list_tools().await?)?;
        let mut connectors = self
            .connectors
            .write()
            .expect("connector registry lock poisoned");
        anyhow::ensure!(
            !connectors.contains_key(&descriptor.id),
            "duplicate connector id"
        );
        connectors.insert(
            descriptor.id.clone(),
            RegisteredConnector {
                descriptor,
                transport,
                tools,
            },
        );
        Ok(())
    }

    pub async fn refresh(&self, connector_id: &str) -> anyhow::Result<Vec<ConnectorToolSpec>> {
        let registered = self.registered(connector_id)?;
        let tools = load_tools(
            &registered.descriptor,
            registered.transport.list_tools().await?,
        )?;
        self.connectors
            .write()
            .expect("connector registry lock poisoned")
            .get_mut(connector_id)
            .ok_or_else(|| anyhow::anyhow!("connector disappeared during refresh"))?
            .tools = tools.clone();
        Ok(tools.into_values().collect())
    }

    pub fn discover(&self) -> Vec<(ConnectorDescriptor, Vec<ConnectorToolSpec>)> {
        self.connectors
            .read()
            .expect("connector registry lock poisoned")
            .values()
            .map(|registered| {
                (
                    registered.descriptor.clone(),
                    registered.tools.values().cloned().collect(),
                )
            })
            .collect()
    }

    pub async fn execute(
        &self,
        connector_id: &str,
        tool_name: &str,
        arguments: Value,
        context: ConnectorCallContext,
    ) -> anyhow::Result<ConnectorExecutionResult> {
        let registered = self.registered(connector_id)?;
        let tool = registered
            .tools
            .get(tool_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown connector tool"))?;
        let action_hash = connector_action_hash(connector_id, tool_name, &arguments)?;
        enforce_approval(&registered.descriptor, &tool, &context, &action_hash)?;
        if tool.risk.is_write() {
            let key = context
                .idempotency_key
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("write connector call requires idempotency key"))?;
            if let Some(previous) = self
                .ledger
                .get(&context.credential_scope, connector_id, key)
                .await?
            {
                anyhow::ensure!(
                    previous.action_hash == action_hash,
                    "idempotency key argument conflict"
                );
                return Ok(ConnectorExecutionResult {
                    connector_id: connector_id.into(),
                    tool_name: tool_name.into(),
                    action_hash,
                    replayed: true,
                    output: previous.output,
                });
            }
        }
        let credential = self.credential(&registered, &tool, &context).await?;
        let request = ConnectorTransportCall {
            call_id: context.call_id,
            tool_name: tool_name.to_string(),
            arguments,
            account_id: context.account_id,
            credential,
            idempotency_key: context.idempotency_key.clone(),
            cancellation: context.cancellation.clone(),
        };
        let output = tokio::select! {
            biased;
            _ = context.cancellation.cancelled() => anyhow::bail!("connector call cancelled"),
            result = tokio::time::timeout(context.timeout, registered.transport.call(request)) => {
                result.map_err(|_| anyhow::anyhow!("connector call timed out"))??
            }
        };
        anyhow::ensure!(
            serde_json::to_vec(&output)?.len() <= self.max_output_bytes,
            "connector output exceeds limit"
        );
        if tool.risk.is_write() {
            self.ledger
                .record(
                    &context.credential_scope,
                    connector_id,
                    context.idempotency_key.as_deref().expect("checked above"),
                    ConnectorLedgerEntry {
                        action_hash: action_hash.clone(),
                        output: output.clone(),
                    },
                )
                .await?;
        }
        Ok(ConnectorExecutionResult {
            connector_id: connector_id.into(),
            tool_name: tool_name.into(),
            action_hash,
            replayed: false,
            output,
        })
    }

    pub async fn health(&self, connector_id: &str) -> anyhow::Result<ConnectorHealth> {
        self.registered(connector_id)?.transport.health().await
    }

    pub async fn stop(&self, connector_id: &str) -> anyhow::Result<()> {
        let registered = self.registered(connector_id)?;
        registered.transport.stop().await?;
        self.connectors
            .write()
            .expect("connector registry lock poisoned")
            .remove(connector_id);
        Ok(())
    }

    fn registered(&self, connector_id: &str) -> anyhow::Result<RegisteredConnector> {
        self.connectors
            .read()
            .expect("connector registry lock poisoned")
            .get(connector_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown connector"))
    }

    async fn credential(
        &self,
        registered: &RegisteredConnector,
        tool: &ConnectorToolSpec,
        context: &ConnectorCallContext,
    ) -> anyhow::Result<Option<SecretMaterial>> {
        if !registered.descriptor.account_required {
            return Ok(None);
        }
        let vault = self
            .vault
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("connector credential vault is unavailable"))?;
        let account_id = context
            .account_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("connector account is required"))?;
        vault
            .lease_for_connector(
                &context.credential_scope,
                &registered.descriptor.id,
                account_id,
                &tool.required_scopes,
            )
            .await
            .map(Some)
    }
}

fn load_tools(
    descriptor: &ConnectorDescriptor,
    tools: Vec<ConnectorToolSpec>,
) -> anyhow::Result<BTreeMap<String, ConnectorToolSpec>> {
    let mut loaded = BTreeMap::new();
    for tool in tools {
        tool.validate()?;
        if !descriptor.tool_is_enabled(&tool.name) {
            continue;
        }
        anyhow::ensure!(
            loaded.insert(tool.name.clone(), tool).is_none(),
            "duplicate connector tool"
        );
    }
    Ok(loaded)
}

fn enforce_approval(
    descriptor: &ConnectorDescriptor,
    tool: &ConnectorToolSpec,
    context: &ConnectorCallContext,
    action_hash: &str,
) -> anyhow::Result<()> {
    let required = match descriptor.approval_mode {
        ConnectorApprovalMode::Auto => false,
        ConnectorApprovalMode::Prompt | ConnectorApprovalMode::Explicit => true,
        ConnectorApprovalMode::Writes => tool.risk.requires_external_approval(),
    };
    if required {
        anyhow::ensure!(
            context.approved_action_hash.as_deref() == Some(action_hash),
            "connector action requires exact approval"
        );
    }
    Ok(())
}

pub fn connector_action_hash(
    connector_id: &str,
    tool_name: &str,
    arguments: &Value,
) -> anyhow::Result<String> {
    let canonical = canonical_json(arguments);
    let envelope = serde_json::json!({
        "connector_id": connector_id,
        "tool_name": tool_name,
        "arguments": canonical,
    });
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&envelope)?)))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        _ => value.clone(),
    }
}

fn validate_name(value: &str, label: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character)),
        "{label} must be model-safe"
    );
    Ok(())
}

#[cfg(test)]
#[path = "connector_tests.rs"]
mod tests;

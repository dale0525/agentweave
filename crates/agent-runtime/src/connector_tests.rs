use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FakeTransport {
    calls: AtomicUsize,
    tools: Mutex<Vec<ConnectorToolSpec>>,
    delay: Duration,
}

impl FakeTransport {
    fn new(tools: Vec<ConnectorToolSpec>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            tools: Mutex::new(tools),
            delay: Duration::ZERO,
        }
    }
}

#[async_trait]
impl ConnectorTransport for FakeTransport {
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn list_tools(&self) -> anyhow::Result<Vec<ConnectorToolSpec>> {
        Ok(self.tools.lock().unwrap().clone())
    }

    async fn call(&self, request: ConnectorTransportCall) -> anyhow::Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(serde_json::json!({
            "tool": request.tool_name,
            "arguments": request.arguments,
            "credential_present": request.credential.is_some()
        }))
    }

    async fn health(&self) -> anyhow::Result<ConnectorHealth> {
        Ok(ConnectorHealth::Ready)
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn tool(name: &str, risk: ConnectorToolRisk) -> ConnectorToolSpec {
    ConnectorToolSpec {
        name: name.into(),
        description: format!("Run {name}."),
        input_schema: serde_json::json!({"type": "object"}),
        output_schema: None,
        risk,
        required_scopes: BTreeSet::new(),
        parallel_safe: risk == ConnectorToolRisk::Read,
        supports_idempotency: risk.is_write(),
    }
}

fn descriptor(mode: ConnectorApprovalMode) -> ConnectorDescriptor {
    ConnectorDescriptor {
        id: "mail_fake".into(),
        name: "Fake Mail".into(),
        version: "0.1.0".into(),
        instructions: None,
        transport: ConnectorTransportKind::McpStreamableHttp,
        required_startup: true,
        account_required: false,
        approval_mode: mode,
        allowed_tools: BTreeSet::new(),
        denied_tools: BTreeSet::new(),
    }
}

fn context() -> ConnectorCallContext {
    ConnectorCallContext {
        call_id: "call-1".into(),
        credential_scope: CredentialScope {
            app_id: "com.example.app".into(),
            tenant_id: "tenant".into(),
            user_id: "user".into(),
        },
        account_id: None,
        approved_action_hash: None,
        idempotency_key: None,
        timeout: Duration::from_secs(1),
        cancellation: CancellationToken::new(),
    }
}

#[tokio::test]
async fn discovers_refreshes_filters_and_checks_health() {
    let transport = Arc::new(FakeTransport::new(vec![
        tool("read", ConnectorToolRisk::Read),
        tool("send", ConnectorToolRisk::Write),
    ]));
    let runtime = ConnectorRuntime::new(None, 4096).unwrap();
    let mut descriptor = descriptor(ConnectorApprovalMode::Writes);
    descriptor.denied_tools.insert("send".into());
    runtime
        .register(descriptor, transport.clone())
        .await
        .unwrap();

    assert_eq!(runtime.discover()[0].1.len(), 1);
    assert_eq!(
        runtime.health("mail_fake").await.unwrap(),
        ConnectorHealth::Ready
    );
    transport
        .tools
        .lock()
        .unwrap()
        .push(tool("search", ConnectorToolRisk::Read));
    assert_eq!(runtime.refresh("mail_fake").await.unwrap().len(), 2);
}

#[tokio::test]
async fn write_requires_exact_approval_and_replays_idempotently_once() {
    let transport = Arc::new(FakeTransport::new(vec![tool(
        "send",
        ConnectorToolRisk::Write,
    )]));
    let runtime = ConnectorRuntime::new(None, 4096).unwrap();
    runtime
        .register(descriptor(ConnectorApprovalMode::Writes), transport.clone())
        .await
        .unwrap();
    let arguments = serde_json::json!({"draft": "d1"});
    let mut call_context = context();
    call_context.idempotency_key = Some("outbox-1".into());
    assert!(
        runtime
            .execute("mail_fake", "send", arguments.clone(), call_context.clone())
            .await
            .is_err()
    );
    call_context.approved_action_hash =
        Some(connector_action_hash("mail_fake", "send", &arguments).unwrap());
    let first = runtime
        .execute("mail_fake", "send", arguments.clone(), call_context.clone())
        .await
        .unwrap();
    let replay = runtime
        .execute("mail_fake", "send", arguments, call_context)
        .await
        .unwrap();

    assert!(!first.replayed);
    assert!(replay.replayed);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn timeout_cancel_and_output_limits_fail_closed() {
    let transport = Arc::new(FakeTransport {
        calls: AtomicUsize::new(0),
        tools: Mutex::new(vec![tool("read", ConnectorToolRisk::Read)]),
        delay: Duration::from_millis(100),
    });
    let runtime = ConnectorRuntime::new(None, 4).unwrap();
    runtime
        .register(descriptor(ConnectorApprovalMode::Auto), transport)
        .await
        .unwrap();
    let mut call_context = context();
    call_context.timeout = Duration::from_millis(1);
    assert!(
        runtime
            .execute("mail_fake", "read", serde_json::json!({}), call_context)
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    let call_context = context();
    call_context.cancellation.cancel();
    assert!(
        runtime
            .execute("mail_fake", "read", serde_json::json!({}), call_context)
            .await
            .is_err()
    );
}

#[test]
fn action_hash_is_stable_across_object_key_order() {
    let left = serde_json::json!({"a": 1, "b": 2});
    let right: Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
    assert_eq!(
        connector_action_hash("c", "t", &left).unwrap(),
        connector_action_hash("c", "t", &right).unwrap()
    );
}

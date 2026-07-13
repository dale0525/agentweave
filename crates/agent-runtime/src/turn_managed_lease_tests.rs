use crate::events::RuntimeEvent;
use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_management::{OwnerSkillManagementService, SkillRollbackOutcome};
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_package::SkillPackageId;
use crate::skill_package::SkillPackageKind;
use crate::skill_policy::SkillManagementPolicy;
use crate::skill_policy::{ActorContext, SkillGrant};
use crate::skill_recovery::snapshot_members;
use crate::skill_source::{ManagedSkillSource, SkillLayer};
use crate::skill_state::{SkillLayerRecord, SkillStateStore};
use crate::skill_store::SkillRevisionStore;
use crate::tools::RuntimeConfig;
use crate::turn::{ModelClient, ModelEventStream, TurnRunner};
use async_trait::async_trait;
use futures::stream;
use model_gateway::responses::{GatewayEvent, GatewayRequest};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::{TempDir, tempdir};

const PACKAGE_ID: &str = "com.example.turn-lease";

struct ManagedTurnFixture {
    authoring: AuthoringFixture,
    second_manager: SkillManager,
    package_id: SkillPackageId,
    revision_a: String,
}

impl ManagedTurnFixture {
    async fn new() -> Self {
        let mut policy = SkillManagementPolicy::owner_only();
        policy.allowed_kinds.insert(SkillPackageKind::NativeRuntime);
        let mut authoring = AuthoringFixture::with_faults(Default::default()).await;
        authoring.service = OwnerSkillManagementService::new(
            authoring.manager.clone(),
            authoring.store.clone(),
            authoring.state.clone(),
            policy,
        );
        let package_id = SkillPackageId::parse(PACKAGE_ID).unwrap();
        let revision_a = create_managed_revision(&authoring, "A", "1.0.0").await;
        authoring
            .state
            .activate_revision(
                &package_id,
                &revision_a,
                SkillLayerRecord::Managed,
                "fixture",
            )
            .await
            .unwrap();
        authoring.manager.startup_reconcile().await.unwrap();
        let second_state = authoring.second_state_connection().await;
        let second_store =
            SkillRevisionStore::new(authoring.store.paths().clone(), second_state.clone());
        let second_manager = SkillManager::new(SkillManagerConfig {
            sources: vec![Arc::new(ManagedSkillSource::from_store(
                second_store.clone(),
            ))],
            platform: crate::platform::PlatformId::Server,
            capabilities: crate::platform::CapabilitySet::from_names(Vec::<String>::new()),
            protected_packages: Vec::new(),
            allowed_overrides: Vec::new(),
            runtime_version: "0.1.0".parse().unwrap(),
        })
        .await
        .unwrap();
        let _second_service = OwnerSkillManagementService::new(
            second_manager.clone(),
            second_store,
            second_state,
            SkillManagementPolicy::owner_only(),
        );
        Self {
            authoring,
            second_manager,
            package_id,
            revision_a,
        }
    }

    async fn create_revision(&self, label: &str, version: &str) -> String {
        create_managed_revision(&self.authoring, label, version).await
    }

    async fn publish_revision(&self, revision_id: &str) {
        publish_revision(
            &self.authoring.state,
            &self.authoring.manager,
            &self.package_id,
            revision_id,
        )
        .await;
    }
}

enum TurnMutation {
    Activate {
        state: SkillStateStore,
        manager: SkillManager,
        package_id: SkillPackageId,
        revision_id: String,
    },
    Rollback {
        service: OwnerSkillManagementService,
        package_id: SkillPackageId,
        revision_id: String,
    },
    Disable {
        service: OwnerSkillManagementService,
        package_id: SkillPackageId,
    },
    RemoveAndCleanup {
        service: OwnerSkillManagementService,
        manager: SkillManager,
        package_id: SkillPackageId,
        leased_revision_id: String,
    },
    DeleteLease {
        state: SkillStateStore,
    },
    ExpireLease {
        state: SkillStateStore,
    },
}

impl TurnMutation {
    async fn apply(self) -> anyhow::Result<()> {
        match self {
            Self::Activate {
                state,
                manager,
                package_id,
                revision_id,
            } => publish_revision(&state, &manager, &package_id, &revision_id).await,
            Self::Rollback {
                service,
                package_id,
                revision_id,
            } => {
                let outcome = service
                    .rollback_managed_skill(
                        &ActorContext::owner("rollback-owner", [SkillGrant::Rollback]),
                        &package_id,
                        &revision_id,
                    )
                    .await?;
                anyhow::ensure!(matches!(outcome, SkillRollbackOutcome::Published(_)));
            }
            Self::Disable {
                service,
                package_id,
            } => {
                service
                    .disable_managed_skill(
                        &ActorContext::owner("disable-owner", [SkillGrant::Disable]),
                        &package_id,
                    )
                    .await?;
            }
            Self::RemoveAndCleanup {
                service,
                manager,
                package_id,
                leased_revision_id,
            } => {
                let request = service
                    .request_removal(
                        &ActorContext::owner("remove-requester", [SkillGrant::DeleteManaged]),
                        &package_id,
                    )
                    .await?;
                service
                    .approve_removal(
                        &request.approval_id,
                        &ActorContext::owner("remove-approver", [SkillGrant::DeleteManaged]),
                    )
                    .await?;
                let cleanup = manager.cleanup_unreferenced_revisions().await?;
                anyhow::ensure!(cleanup.retained_revisions.contains(&leased_revision_id));
            }
            Self::DeleteLease { state } => {
                sqlx::query("DELETE FROM skill_snapshot_leases")
                    .execute(state.pool())
                    .await?;
            }
            Self::ExpireLease { state } => {
                sqlx::query("UPDATE skill_snapshot_leases SET expires_at = '2000-01-01T00:00:00Z'")
                    .execute(state.pool())
                    .await?;
            }
        }
        Ok(())
    }
}

struct MutatingModel {
    calls: AtomicUsize,
    mutation: tokio::sync::Mutex<Option<TurnMutation>>,
    return_tool: bool,
}

impl MutatingModel {
    fn new(mutation: TurnMutation) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            mutation: tokio::sync::Mutex::new(Some(mutation)),
            return_tool: true,
        }
    }

    fn text_after_mutation(mutation: TurnMutation) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            mutation: tokio::sync::Mutex::new(Some(mutation)),
            return_tool: false,
        }
    }
}

#[async_trait]
impl ModelClient for MutatingModel {
    async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = if call == 0 {
            let tool_name = request
                .tools
                .iter()
                .map(|tool| tool.advertised_name())
                .find(|name| name.ends_with("managed_tool"))
                .expect("managed tool must be advertised")
                .to_string();
            self.mutation
                .lock()
                .await
                .take()
                .expect("turn mutation must run once")
                .apply()
                .await?;
            if self.return_tool {
                vec![
                    GatewayEvent::ToolCall {
                        call_id: "managed-call".into(),
                        name: tool_name,
                        legacy_alias_selected: false,
                        arguments: json!({}),
                    },
                    GatewayEvent::Completed,
                ]
            } else {
                vec![
                    GatewayEvent::TextDelta {
                        text: "must not complete".into(),
                    },
                    GatewayEvent::Completed,
                ]
            }
        } else {
            vec![
                GatewayEvent::TextDelta {
                    text: "done".into(),
                },
                GatewayEvent::Completed,
            ]
        };
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

#[tokio::test]
async fn old_turn_executes_exact_a_after_activate_b_same_manager() {
    let fixture = ManagedTurnFixture::new().await;
    let revision_b = fixture.create_revision("B", "2.0.0").await;
    let events = run_mutating_turn(
        fixture.authoring.manager.clone(),
        TurnMutation::Activate {
            state: fixture.authoring.state.clone(),
            manager: fixture.authoring.manager.clone(),
            package_id: fixture.package_id.clone(),
            revision_id: revision_b.clone(),
        },
    )
    .await;

    assert_successful_revision(&events, "A");
    assert_active_revision(
        &fixture
            .authoring
            .manager
            .lease_snapshot_for_turn()
            .await
            .unwrap(),
        Some(&revision_b),
    );
}

#[tokio::test]
async fn old_turn_executes_exact_a_after_rollback_from_independent_manager() {
    let fixture = ManagedTurnFixture::new().await;
    let rollback_revision = fixture.revision_a.clone();
    let revision_a = fixture.create_revision("A-current", "2.0.0").await;
    fixture.publish_revision(&revision_a).await;
    let turn_manager = fixture.second_manager.clone();
    let events = run_mutating_turn(
        turn_manager.clone(),
        TurnMutation::Rollback {
            service: fixture.authoring.service.clone(),
            package_id: fixture.package_id.clone(),
            revision_id: rollback_revision.clone(),
        },
    )
    .await;

    assert_successful_revision(&events, "A-current");
    assert_active_revision(
        &turn_manager.lease_snapshot_for_turn().await.unwrap(),
        Some(&rollback_revision),
    );
}

#[tokio::test]
async fn old_turn_executes_exact_a_after_disable_same_manager() {
    let fixture = ManagedTurnFixture::new().await;
    let events = run_mutating_turn(
        fixture.authoring.manager.clone(),
        TurnMutation::Disable {
            service: fixture.authoring.service.clone(),
            package_id: fixture.package_id.clone(),
        },
    )
    .await;

    assert_successful_revision(&events, "A");
    assert_active_revision(
        &fixture
            .authoring
            .manager
            .lease_snapshot_for_turn()
            .await
            .unwrap(),
        None,
    );
}

#[tokio::test]
async fn old_turn_executes_exact_a_after_remove_and_cross_manager_cleanup() {
    let fixture = ManagedTurnFixture::new().await;
    let turn_manager = fixture.second_manager.clone();
    let events = run_mutating_turn(
        turn_manager.clone(),
        TurnMutation::RemoveAndCleanup {
            service: fixture.authoring.service.clone(),
            manager: fixture.authoring.manager.clone(),
            package_id: fixture.package_id.clone(),
            leased_revision_id: fixture.revision_a.clone(),
        },
    )
    .await;

    assert_successful_revision(&events, "A");
    assert_active_revision(&turn_manager.lease_snapshot_for_turn().await.unwrap(), None);
}

#[tokio::test]
async fn missing_durable_lease_fences_managed_turn_before_execution() {
    let fixture = ManagedTurnFixture::new().await;
    let workspace = tempdir().unwrap();
    let runner = TurnRunner::new_with_manager_and_config(
        MutatingModel::text_after_mutation(TurnMutation::DeleteLease {
            state: fixture.authoring.state.clone(),
        }),
        fixture.authoring.manager.clone(),
        RuntimeConfig::workspace_write(workspace.path(), workspace.path()),
    );
    let events = runner.run("finish with text").await.unwrap();

    assert_lease_fenced(&events);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::TurnFinished { .. }))
    );
}

#[tokio::test]
async fn expired_durable_lease_fences_managed_turn_before_execution() {
    let fixture = ManagedTurnFixture::new().await;
    let events = run_mutating_turn(
        fixture.authoring.manager.clone(),
        TurnMutation::ExpireLease {
            state: fixture.authoring.state.clone(),
        },
    )
    .await;

    assert_lease_fenced(&events);
}

#[tokio::test]
async fn expired_durable_lease_cannot_be_refreshed_or_resurrected() {
    let fixture = ManagedTurnFixture::new().await;
    let _lease = fixture
        .authoring
        .manager
        .lease_snapshot_for_turn()
        .await
        .unwrap();
    let lease_id: String = sqlx::query_scalar("SELECT lease_id FROM skill_snapshot_leases")
        .fetch_one(fixture.authoring.state.pool())
        .await
        .unwrap();
    let expired_at = "2000-01-01T00:00:00Z";
    sqlx::query("UPDATE skill_snapshot_leases SET expires_at = ? WHERE lease_id = ?")
        .bind(expired_at)
        .bind(&lease_id)
        .execute(fixture.authoring.state.pool())
        .await
        .unwrap();

    assert!(
        !fixture
            .authoring
            .state
            .refresh_snapshot_lease(&lease_id)
            .await
            .unwrap()
    );
    let persisted: String =
        sqlx::query_scalar("SELECT expires_at FROM skill_snapshot_leases WHERE lease_id = ?")
            .bind(&lease_id)
            .fetch_one(fixture.authoring.state.pool())
            .await
            .unwrap();
    assert_eq!(persisted, expired_at);
}

async fn run_mutating_turn(manager: SkillManager, mutation: TurnMutation) -> Vec<RuntimeEvent> {
    let workspace = tempdir().unwrap();
    let runner = TurnRunner::new_with_manager_and_config(
        MutatingModel::new(mutation),
        manager,
        RuntimeConfig::workspace_write(workspace.path(), workspace.path()),
    );
    runner.run("run the managed tool").await.unwrap()
}

async fn create_managed_revision(fixture: &AuthoringFixture, label: &str, version: &str) -> String {
    let package = write_runtime_package(label, version).await;
    let staged = fixture
        .store
        .create_staging_revision(package.path(), "fixture")
        .await
        .unwrap();
    let managed = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();
    let validation = crate::skill_management::SkillDraftValidation {
        ok: true,
        errors: Vec::new(),
        warnings: Vec::new(),
        required_tools: Vec::new(),
        required_connectors: Vec::new(),
        dependencies: Vec::new(),
        required_capabilities: Vec::new(),
        resolver_status: "active".into(),
        resolver_errors: Vec::new(),
        permission_diff: json!({}),
        revision_id: managed.revision_id.clone(),
        content_hash: managed.content_hash.clone(),
        snapshot_generation: fixture.manager.current_snapshot().generation(),
    };
    sqlx::query("UPDATE skill_revisions SET validation_json = ? WHERE revision_id = ?")
        .bind(serde_json::to_string(&validation).unwrap())
        .bind(&managed.revision_id)
        .execute(fixture.state.pool())
        .await
        .unwrap();
    managed.revision_id
}

async fn publish_revision(
    state: &SkillStateStore,
    manager: &SkillManager,
    package_id: &SkillPackageId,
    revision_id: &str,
) {
    let active = state
        .snapshot_with_status(crate::skill_state::SkillSnapshotStatus::Active)
        .await
        .unwrap()
        .unwrap();
    state
        .activate_revision(
            package_id,
            revision_id,
            SkillLayerRecord::Managed,
            "fixture",
        )
        .await
        .unwrap();
    manager.reload().await.unwrap();
    let candidate = manager.current_snapshot();
    state
        .persist_recovery_candidate(
            &active,
            candidate.generation(),
            &snapshot_members(&candidate),
        )
        .await
        .unwrap();
}

async fn write_runtime_package(label: &str, version: &str) -> TempDir {
    let root = tempdir().unwrap();
    tokio::fs::write(
        root.path().join("general-agent.json"),
        json!({
            "schemaVersion": 1,
            "id": PACKAGE_ID,
            "version": version,
            "displayName": "Turn lease runtime",
            "kind": "native_runtime",
            "package": {"includeInstructions": false, "includeRuntime": true}
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("skill.json"),
        json!({
            "name": "turn-lease-runtime",
            "description": "Managed turn lease regression runtime.",
            "version": version,
            "entry": {"type": "command", "command": "sh", "args": ["run.sh"]},
            "tools": [{
                "name": "managed_tool",
                "description": "Return the executing revision bytes.",
                "input_schema": {"type": "object"}
            }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("run.sh"),
        format!("printf '{{\"revision\":\"{label}\"}}'\n"),
    )
    .await
    .unwrap();
    root
}

fn assert_successful_revision(events: &[RuntimeEvent], expected: &str) {
    let result = events
        .iter()
        .find_map(|event| match event {
            RuntimeEvent::ToolCallFinished { result, .. } => Some(result),
            _ => None,
        })
        .expect("managed tool must finish");
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["data"]["revision"], expected);
}

fn assert_lease_fenced(events: &[RuntimeEvent]) {
    assert!(!events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallFinished { result, .. } if result["ok"] == true
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::TurnFailed { message, .. }
            if message.contains("turn snapshot lease is no longer authoritative")
    )));
}

fn assert_active_revision(
    lease: &crate::skill_snapshot::SkillSnapshotLease,
    expected: Option<&str>,
) {
    let revision = lease
        .snapshot()
        .packages()
        .iter()
        .find(|resolved| resolved.package.layer == SkillLayer::Managed)
        .and_then(|resolved| resolved.package.verified_content.as_ref())
        .and_then(|content| content.execution_binding.as_ref())
        .map(|binding| binding.revision_id.as_str());
    assert_eq!(revision, expected);
}

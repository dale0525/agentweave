use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_management::{CreateSkillDraftRequest, OwnerSkillManagementService};
use agent_runtime::skill_manager::{SkillManager, SkillManagerConfig};
use agent_runtime::skill_package::{SkillPackageId, SkillPackageKind};
use agent_runtime::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use agent_runtime::skill_source::ManagedSkillSource;
use agent_runtime::skill_state::SkillStateStore;
use agent_runtime::skill_store::{SkillRevisionStore, SkillStorePaths};
use agent_runtime::storage::Storage;
use agent_runtime::tools::RuntimeConfig;
use agent_runtime::turn::{ModelClient, ModelEventStream, TurnRunner};
use async_trait::async_trait;
use futures::stream;
use model_gateway::responses::{GatewayEvent, GatewayRequest};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::sync::Notify;

struct BlockingCaptureModel {
    calls: AtomicUsize,
    requests: Mutex<Vec<serde_json::Value>>,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl ModelClient for BlockingCaptureModel {
    async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_value(&request.input)?);
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(Box::pin(stream::iter(vec![Ok(GatewayEvent::Completed)])))
    }
}

#[tokio::test]
async fn approved_skill_is_visible_on_the_next_turn_only() {
    let app = tempdir().unwrap();
    let cache = tempdir().unwrap();
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let paths = SkillStorePaths::prepare(app.path(), cache.path())
        .await
        .unwrap();
    let store = SkillRevisionStore::new(paths, state.clone());
    let manager = SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(ManagedSkillSource::from_store(store.clone()))],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap();
    let service = OwnerSkillManagementService::new(
        manager.clone(),
        store,
        state,
        SkillManagementPolicy::owner_only(),
    );
    let model = Arc::new(BlockingCaptureModel {
        calls: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let runner = Arc::new(
        TurnRunner::new_with_manager_and_config(
            model.clone(),
            manager.clone(),
            RuntimeConfig::read_only(".", ".").without_builtin_tools(),
        )
        .with_skill_management(service.clone()),
    );

    let running_turn = {
        let runner = runner.clone();
        tokio::spawn(async move { runner.run("before activation").await })
    };
    model.entered.notified().await;

    let requester = ActorContext::owner(
        "owner-1",
        [
            SkillGrant::CreateDraft,
            SkillGrant::Validate,
            SkillGrant::Activate,
        ],
    );
    let draft = service
        .create_draft(
            &requester,
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse("com.example.calendar").unwrap(),
                display_name: "Calendar".into(),
                description: "Next turn calendar marker.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    service
        .validate_draft(&requester, &draft.revision_id)
        .await
        .unwrap();
    let approval = service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    service
        .approve_activation(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::Activate]),
        )
        .await
        .unwrap();

    model.release.notify_one();
    running_turn.await.unwrap().unwrap();
    runner.run("after activation").await.unwrap();

    let requests = model.requests.lock().unwrap();
    assert!(
        !requests[0]
            .to_string()
            .contains("Next turn calendar marker.")
    );
    assert!(
        requests[1]
            .to_string()
            .contains("Next turn calendar marker.")
    );
}

use agent_runtime::skill::SkillRegistry;
use agent_runtime::skill_catalog::SkillCatalog;
use agent_runtime::skill_management::OwnerSkillManagementService;
use agent_runtime::skill_manager::SkillManager;
use agent_runtime::skill_policy::{
    ActorContext, SkillGrant, SkillManagementMode, SkillManagementPolicy,
};
use agent_runtime::skill_state::SkillStateStore;
use agent_runtime::skill_store::{SkillRevisionStore, SkillStoreLimits, SkillStorePaths};
use agent_runtime::storage::Storage;
use agent_runtime::tools::RuntimeConfig;
use agent_runtime::turn::{ModelClient, ModelEventStream};
use agent_server::api::{self, AppState};
use agent_server::owner_api::{OwnerApiConfig, OwnerAuth};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures::stream;
use model_gateway::responses::{GatewayEvent, GatewayRequest};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

struct OwnerTestApp {
    app: Router,
    service: OwnerSkillManagementService,
    state: SkillStateStore,
    store: SkillRevisionStore,
    roots: TestRoots,
    chat_tools: Arc<std::sync::Mutex<Vec<String>>>,
}

struct CapturingChatModel {
    tools: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl ModelClient for CapturingChatModel {
    async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
        *self.tools.lock().unwrap() = request.tools.into_iter().map(|tool| tool.name).collect();
        Ok(Box::pin(stream::iter(vec![
            Ok(GatewayEvent::TextDelta {
                text: "done".into(),
            }),
            Ok(GatewayEvent::Completed),
        ])))
    }
}

struct TestRoots {
    app_root: PathBuf,
    cache_root: PathBuf,
}

impl TestRoots {
    fn new() -> Self {
        let base =
            std::env::temp_dir().join(format!("general-agent-owner-api-{}", uuid::Uuid::new_v4()));
        let app_root = base.join("app");
        let cache_root = base.join("cache");
        std::fs::create_dir_all(&app_root).unwrap();
        std::fs::create_dir_all(&cache_root).unwrap();
        Self {
            app_root,
            cache_root,
        }
    }
}

impl Drop for TestRoots {
    fn drop(&mut self) {
        if let Some(base) = self.app_root.parent() {
            let _ = std::fs::remove_dir_all(base);
        }
    }
}

async fn owner_test_app(
    policy: SkillManagementPolicy,
    token: &str,
    actor: ActorContext,
) -> OwnerTestApp {
    owner_test_app_with_limits(policy, token, actor, SkillStoreLimits::default()).await
}

async fn owner_test_app_with_limits(
    policy: SkillManagementPolicy,
    token: &str,
    actor: ActorContext,
    limits: SkillStoreLimits,
) -> OwnerTestApp {
    let roots = TestRoots::new();
    let paths = SkillStorePaths::prepare(&roots.app_root, &roots.cache_root)
        .await
        .unwrap();
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let store = SkillRevisionStore::with_limits(paths, state.clone(), limits);
    let manager =
        SkillManager::from_registry_and_catalog(SkillRegistry::empty(), SkillCatalog::empty());
    let service =
        OwnerSkillManagementService::new(manager.clone(), store.clone(), state.clone(), policy);
    let owner = OwnerApiConfig::new(service.clone(), OwnerAuth::new(token, actor).unwrap());
    let chat_tools = Arc::new(std::sync::Mutex::new(Vec::new()));
    let app_state = Arc::new(AppState::new_with_model_skill_manager_and_owner(
        storage,
        CapturingChatModel {
            tools: chat_tools.clone(),
        },
        manager,
        RuntimeConfig::read_only(".", ".").without_builtin_tools(),
        owner,
    ));
    OwnerTestApp {
        app: api::router(app_state),
        service,
        state,
        store,
        roots,
        chat_tools,
    }
}

fn owner_actor() -> ActorContext {
    ActorContext::owner("owner-1", [SkillGrant::Inspect, SkillGrant::CreateDraft])
}

fn request(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", token);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap()
}

fn draft_body(package_id: &str) -> Value {
    json!({
        "package_id": package_id,
        "display_name": "Calendar",
        "description": "Calendar workflow.",
        "kind": "instruction_only",
        "required_tools": []
    })
}

#[tokio::test]
async fn owner_routes_are_absent_when_management_is_disabled() {
    let test = owner_test_app(
        SkillManagementPolicy::default(),
        "secret-token",
        owner_actor(),
    )
    .await;

    let response = test
        .app
        .oneshot(request("GET", "/owner/skills", None, None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn owner_routes_require_an_exact_host_token_without_echoing_it() {
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        owner_actor(),
    )
    .await;
    for supplied in [
        None,
        Some("Bearer wrong-token"),
        Some("Bearer secret-token "),
    ] {
        let response = test
            .app
            .clone()
            .oneshot(request("GET", "/owner/skills", supplied, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!body.contains("secret-token"));
        assert!(!body.contains("wrong-token"));
    }

    let response = test
        .app
        .oneshot(request(
            "GET",
            "/owner/skills",
            Some("Bearer secret-token"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn diagnostics_and_organization_modes_mount_authenticated_get_only_routes() {
    for mode in [
        SkillManagementMode::DiagnosticsOnly,
        SkillManagementMode::OrganizationManaged,
    ] {
        let policy = SkillManagementPolicy {
            mode,
            ..SkillManagementPolicy::default()
        };
        let actor = ActorContext::anonymous().with_grants([SkillGrant::Inspect]);
        let test = owner_test_app(policy, "secret-token", actor).await;

        for uri in [
            "/owner/policy",
            "/owner/skills",
            "/owner/skills/com.example.calendar/audit",
        ] {
            let response = test
                .app
                .clone()
                .oneshot(request("GET", uri, Some("Bearer secret-token"), None))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{mode:?} {uri}");
        }
        let response = test
            .app
            .oneshot(request(
                "POST",
                "/owner/skills/drafts",
                Some("Bearer secret-token"),
                Some(draft_body("com.example.calendar")),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{mode:?}");
    }
}

#[tokio::test]
async fn request_headers_and_json_cannot_spoof_the_host_actor() {
    let actor = ActorContext::anonymous();
    let test = owner_test_app(SkillManagementPolicy::owner_only(), "secret-token", actor).await;
    let response = test
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/owner/skills")
                .header("authorization", "Bearer secret-token")
                .header("x-actor-role", "owner")
                .header("x-actor-grants", "inspect,create_draft")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let owner_test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        owner_actor(),
    )
    .await;
    let mut body = draft_body("com.example.spoofed");
    body["actor"] = json!({
        "actor_id": "attacker",
        "role": "owner",
        "grants": ["create_draft"]
    });
    let response = owner_test
        .app
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            Some("Bearer secret-token"),
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(directory_is_empty(&owner_test.store.paths().staging).await);
}

#[tokio::test]
async fn owner_routes_keep_stable_bad_request_forbidden_and_internal_boundaries() {
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        owner_actor(),
    )
    .await;
    let malformed = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            Some("Bearer secret-token"),
            Some(json!({"package_id": "not-valid"})),
        ))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let denied = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            Some("Bearer secret-token"),
            Some(json!({
                "package_id": "com.example.native",
                "display_name": "Native",
                "description": "Denied by default.",
                "kind": "native_runtime",
                "required_tools": []
            })),
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);

    tokio::fs::remove_dir_all(test.roots.cache_root.join("skill-staging"))
        .await
        .unwrap();
    let internal = test
        .app
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            Some("Bearer secret-token"),
            Some(draft_body("com.example.internal")),
        ))
        .await
        .unwrap();
    assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn authorized_owner_can_create_list_and_read_audit() {
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        owner_actor(),
    )
    .await;
    let response = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            Some("Bearer secret-token"),
            Some(draft_body("com.example.calendar")),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    let revision_id = body["revision_id"].as_str().unwrap();
    let promoted = test.store.promote_revision(revision_id).await.unwrap();
    let package_id =
        agent_runtime::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    test.state
        .activate_revision(
            &package_id,
            &promoted.revision_id,
            agent_runtime::skill_state::SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();

    let skills = test
        .app
        .clone()
        .oneshot(request(
            "GET",
            "/owner/skills",
            Some("Bearer secret-token"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(skills.status(), StatusCode::OK);
    let skills: Value =
        serde_json::from_slice(&to_bytes(skills.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(skills["managed"].as_array().unwrap().len(), 1);

    let audit = test
        .app
        .oneshot(request(
            "GET",
            "/owner/skills/com.example.calendar/audit",
            Some("Bearer secret-token"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(audit.status(), StatusCode::OK);
    let audit: Value =
        serde_json::from_slice(&to_bytes(audit.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(audit.as_array().unwrap().iter().any(|record| {
        record["operation"] == "activate_revision" && record["actor_id"] == "owner-1"
    }));

    let direct = test
        .service
        .list_managed_skills(&owner_actor())
        .await
        .unwrap();
    assert_eq!(direct.len(), 1);
}

#[tokio::test]
async fn ordinary_chat_does_not_inherit_owner_api_actor_or_management_tools() {
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        owner_actor(),
    )
    .await;
    let session = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/sessions",
            None,
            Some(json!({"title": "Ordinary chat"})),
        ))
        .await
        .unwrap();
    let session: Value =
        serde_json::from_slice(&to_bytes(session.into_body(), usize::MAX).await.unwrap()).unwrap();
    let response = test
        .app
        .oneshot(request(
            "POST",
            &format!("/sessions/{}/messages", session["id"].as_str().unwrap()),
            None,
            Some(json!({"content": "create a skill"})),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !test
            .chat_tools
            .lock()
            .unwrap()
            .iter()
            .any(|name| name == "create_skill_draft")
    );
    assert!(directory_is_empty(&test.store.paths().staging).await);
}

#[tokio::test]
async fn oversized_owner_draft_request_is_bad_request_not_internal_error() {
    let test = owner_test_app_with_limits(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        owner_actor(),
        SkillStoreLimits {
            max_file_bytes: 128,
            max_package_bytes: 256,
            ..SkillStoreLimits::default()
        },
    )
    .await;
    let mut body = draft_body("com.example.api-oversized");
    body["description"] = Value::String("x".repeat(512));

    let response = test
        .app
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            Some("Bearer secret-token"),
            Some(body),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(directory_is_empty(&test.store.paths().staging).await);
}

async fn directory_is_empty(path: &std::path::Path) -> bool {
    tokio::fs::read_dir(path)
        .await
        .unwrap()
        .next_entry()
        .await
        .unwrap()
        .is_none()
}

use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_management::OwnerSkillManagementService;
use agent_runtime::skill_manager::{SkillManager, SkillManagerConfig};
use agent_runtime::skill_package::SkillPackageId;
use agent_runtime::skill_policy::{
    ActorContext, SkillGrant, SkillManagementMode, SkillManagementPolicy,
};
use agent_runtime::skill_source::ManagedSkillSource;
use agent_runtime::skill_state::SkillStateStore;
use agent_runtime::skill_store::{SkillRevisionStore, SkillStoreLimits, SkillStorePaths};
use agent_runtime::storage::Storage;
use agent_runtime::tools::RuntimeConfig;
use agent_runtime::turn::{ModelClient, ModelEventStream};
use agent_server::api::{self, AppState};
use agent_server::owner_api::{OwnerApiConfig, OwnerAuth};
use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::http::{Request, StatusCode};
use futures::stream;
use model_gateway::responses::{GatewayEvent, GatewayRequest};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;

struct OwnerTestApp {
    app: Router,
    service: OwnerSkillManagementService,
    manager: SkillManager,
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
    import_root: PathBuf,
    export_root: PathBuf,
}

impl TestRoots {
    fn new() -> Self {
        let base =
            std::env::temp_dir().join(format!("general-agent-owner-api-{}", uuid::Uuid::new_v4()));
        let app_root = base.join("app");
        let cache_root = base.join("cache");
        let import_root = base.join("imports");
        let export_root = base.join("exports");
        std::fs::create_dir_all(&app_root).unwrap();
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&import_root).unwrap();
        std::fs::create_dir_all(&export_root).unwrap();
        Self {
            app_root,
            cache_root,
            import_root,
            export_root,
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
    owner_test_app_with_auth(policy, OwnerAuth::new(token, actor).unwrap(), limits).await
}

async fn owner_test_app_with_auth(
    policy: SkillManagementPolicy,
    auth: OwnerAuth,
    limits: SkillStoreLimits,
) -> OwnerTestApp {
    let roots = TestRoots::new();
    let paths = SkillStorePaths::prepare(&roots.app_root, &roots.cache_root)
        .await
        .unwrap();
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let store = SkillRevisionStore::with_limits(paths, state.clone(), limits);
    let manager = SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(ManagedSkillSource::from_store(store.clone()))],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: policy.protected_packages.iter().cloned().collect(),
        allowed_overrides: policy.allowed_overrides.iter().cloned().collect(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap();
    let service =
        OwnerSkillManagementService::new(manager.clone(), store.clone(), state.clone(), policy)
            .with_transfer_roots(&roots.import_root, &roots.export_root)
            .unwrap();
    let owner = OwnerApiConfig::new(service.clone(), auth);
    let chat_tools = Arc::new(std::sync::Mutex::new(Vec::new()));
    let app_state = Arc::new(AppState::new_with_model_skill_manager_and_owner(
        storage.clone(),
        CapturingChatModel {
            tools: chat_tools.clone(),
        },
        manager.clone(),
        RuntimeConfig::read_only(".", ".").without_builtin_tools(),
        owner,
    ));
    OwnerTestApp {
        app: api::router(app_state),
        service,
        manager,
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

fn raw_json_request(token: Option<&str>, body: impl Into<Body>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/owner/skills/drafts")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", token);
    }
    builder.body(body.into()).unwrap()
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
async fn owner_auth_rejects_before_polling_the_request_body() {
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        owner_actor(),
    )
    .await;
    let polled = Arc::new(AtomicBool::new(false));
    let body_polled = polled.clone();
    let body = Body::from_stream(stream::once(async move {
        body_polled.store(true, Ordering::SeqCst);
        Ok::<_, std::io::Error>(Bytes::from_static(b"{"))
    }));

    let response = test
        .app
        .oneshot(raw_json_request(None, body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!polled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn owner_auth_precedes_malformed_and_oversized_json_errors() {
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        owner_actor(),
    )
    .await;
    let malformed = r#"{"package_id":"#.to_string();
    let oversized = format!(
        r#"{{"package_id":"com.example.large","display_name":"Large","description":"{}","kind":"instruction_only","required_tools":[]}}"#,
        "x".repeat(2 * 1024 * 1024)
    );

    for token in [None, Some("Bearer wrong-token")] {
        for body in [&malformed, &oversized] {
            let response = test
                .app
                .clone()
                .oneshot(raw_json_request(token, body.clone()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    let response = test
        .app
        .oneshot(raw_json_request(Some("Bearer secret-token"), malformed))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
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

fn task10_actor(id: &str) -> ActorContext {
    ActorContext::owner(
        id,
        [
            SkillGrant::Inspect,
            SkillGrant::CreateDraft,
            SkillGrant::EditDraft,
            SkillGrant::Validate,
            SkillGrant::Test,
            SkillGrant::Activate,
            SkillGrant::Import,
            SkillGrant::Export,
        ],
    )
}

#[test]
fn owner_auth_accepts_distinct_principals_and_rejects_invalid_credentials() {
    let auth = OwnerAuth::from_principals([
        (b"owner-token".as_slice(), task10_actor("owner-1")),
        (
            b"approver-token".as_slice(),
            ActorContext::owner("approver-2", [SkillGrant::Inspect, SkillGrant::Activate]),
        ),
    ]);
    assert!(auth.is_ok());
    assert!(OwnerAuth::from_principals([(b"".as_slice(), task10_actor("owner-1"))]).is_err());
    assert!(
        OwnerAuth::from_principals([
            (b"same".as_slice(), task10_actor("owner-1")),
            (b"same".as_slice(), task10_actor("approver-2")),
        ])
        .is_err()
    );
}

async fn response_json(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn owner_task10_routes_complete_authoring_and_request_flow() {
    let auth = OwnerAuth::from_principals([
        (b"secret-token".as_slice(), task10_actor("owner-1")),
        (
            b"approver-token".as_slice(),
            ActorContext::owner("approver-2", [SkillGrant::Inspect, SkillGrant::Activate]),
        ),
    ])
    .unwrap();
    let test = owner_test_app_with_auth(
        SkillManagementPolicy::owner_only(),
        auth,
        SkillStoreLimits::default(),
    )
    .await;
    let token = Some("Bearer secret-token");
    let created = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            token,
            Some(draft_body("com.example.task10")),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    let revision_id = created["revision_id"].as_str().unwrap();

    for (method, suffix, body, expected) in [
        (
            "PUT",
            "",
            json!({"files": [{"path": "references/guide.md", "content": "guide"}]}),
            StatusCode::OK,
        ),
        ("POST", "/validate", json!({}), StatusCode::OK),
        ("POST", "/test", json!({}), StatusCode::OK),
        ("POST", "/activation", json!({}), StatusCode::ACCEPTED),
    ] {
        let response = test
            .app
            .clone()
            .oneshot(request(
                method,
                &format!("/owner/skills/drafts/{revision_id}{suffix}"),
                token,
                Some(body),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{method} {suffix}");
    }

    let package_id = SkillPackageId::parse("com.example.task10").unwrap();
    let approval_audit = test
        .state
        .list_audit(&package_id)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.operation == "skill_approval_required")
        .unwrap();
    let approval_id = approval_audit.metadata_json["approvalId"].as_str().unwrap();
    let approved = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/owner/skills/approvals/{approval_id}"),
            Some("Bearer approver-token"),
            Some(json!({"decision": "approve"})),
        ))
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    let duplicate = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/owner/skills/approvals/{approval_id}"),
            Some("Bearer approver-token"),
            Some(json!({"decision": "approve"})),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(test.manager.current_snapshot().generation(), 2);
}

#[tokio::test]
async fn all_task10_mutations_authenticate_before_json_and_reject_actor_spoofing() {
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        task10_actor("owner-1"),
    )
    .await;
    for (method, uri) in [
        ("POST", "/owner/skills/drafts/import"),
        (
            "PUT",
            "/owner/skills/drafts/00000000-0000-4000-8000-000000000001",
        ),
        (
            "POST",
            "/owner/skills/drafts/00000000-0000-4000-8000-000000000001/validate",
        ),
        (
            "POST",
            "/owner/skills/drafts/00000000-0000-4000-8000-000000000001/test",
        ),
        (
            "POST",
            "/owner/skills/drafts/00000000-0000-4000-8000-000000000001/activation",
        ),
        ("POST", "/owner/skills/com.example.calendar/export"),
        (
            "POST",
            "/owner/skills/approvals/00000000-0000-4000-8000-000000000001",
        ),
    ] {
        let response = test
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from("{"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }

    let spoofed = test
        .app
        .oneshot(request(
            "POST",
            "/owner/skills/drafts/import",
            Some("Bearer secret-token"),
            Some(json!({
                "name": "calendar",
                "actor": {"actor_id": "attacker", "role": "owner"}
            })),
        ))
        .await
        .unwrap();
    assert_eq!(spoofed.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn task10_service_errors_map_to_stable_http_boundaries_without_leaks() {
    let token = Some("Bearer secret-token");
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        task10_actor("owner-1"),
    )
    .await;

    let malformed = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/approvals/not-a-uuid",
            token,
            Some(json!({"decision": "approve"})),
        ))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let missing = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts/00000000-0000-4000-8000-000000000001/validate",
            token,
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let missing_export = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/com.example.missing/export",
            token,
            Some(json!({"name": "missing"})),
        ))
        .await
        .unwrap();
    assert_eq!(missing_export.status(), StatusCode::NOT_FOUND);

    let created = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            token,
            Some(draft_body("com.example.error-boundary")),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let revision_id = response_json(created).await["revision_id"]
        .as_str()
        .unwrap()
        .to_string();
    let staging = test.store.paths().staging.clone();
    std::fs::rename(&staging, staging.with_extension("moved")).unwrap();
    std::fs::create_dir(&staging).unwrap();
    let internal = test
        .app
        .oneshot(request(
            "POST",
            &format!("/owner/skills/drafts/{revision_id}/validate"),
            token,
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response_json(internal).await.to_string();
    assert!(!body.contains("skill_revisions"));
    assert!(!body.contains("secret-token"));
    assert!(!body.contains(test.roots.app_root.to_string_lossy().as_ref()));
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

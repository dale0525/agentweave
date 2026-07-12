use crate::api::{AppState, ErrorResponse};
use agent_runtime::skill_management::{
    CreateSkillDraftRequest, DraftFileUpdate, OwnerSkillManagementService, SkillManagementError,
    SkillPackageStatus,
};
use agent_runtime::skill_policy::{
    ActorContext, SkillManagementMode, SkillManagementPolicy, SkillOperation,
};
use agent_runtime::skill_state::SkillAuditRecord;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone)]
pub struct OwnerAuth {
    principals: Arc<[OwnerPrincipal]>,
}

#[derive(Clone)]
struct OwnerPrincipal {
    authorization: Arc<[u8]>,
    actor: ActorContext,
}

impl OwnerAuth {
    pub fn new(token: impl AsRef<[u8]>, actor: ActorContext) -> anyhow::Result<Self> {
        Self::from_principals([(token, actor)])
    }

    pub fn from_principals<T, I>(principals: I) -> anyhow::Result<Self>
    where
        T: AsRef<[u8]>,
        I: IntoIterator<Item = (T, ActorContext)>,
    {
        let mut seen = std::collections::BTreeSet::new();
        let mut resolved = Vec::new();
        for (token, actor) in principals {
            let token = token.as_ref();
            if token.is_empty() {
                anyhow::bail!("owner bearer token cannot be empty");
            }
            if !seen.insert(token.to_vec()) {
                anyhow::bail!("owner bearer tokens must be distinct");
            }
            let mut authorization = Vec::with_capacity(7 + token.len());
            authorization.extend_from_slice(b"Bearer ");
            authorization.extend_from_slice(token);
            resolved.push(OwnerPrincipal {
                authorization: Arc::from(authorization),
                actor,
            });
        }
        if resolved.is_empty() {
            anyhow::bail!("at least one owner bearer principal is required");
        }
        Ok(Self {
            principals: Arc::from(resolved),
        })
    }

    fn authenticate(&self, headers: &HeaderMap) -> Result<ActorContext, OwnerApiError> {
        let supplied = headers
            .get(header::AUTHORIZATION)
            .map(|value| value.as_bytes())
            .ok_or(OwnerApiError::Unauthorized)?;
        let mut matched = None;
        for principal in self.principals.iter() {
            if constant_time_eq(&principal.authorization, supplied) {
                matched = Some(principal.actor.clone());
            }
        }
        matched.ok_or(OwnerApiError::Unauthorized)
    }

    fn actors(&self) -> impl Iterator<Item = &ActorContext> {
        self.principals.iter().map(|principal| &principal.actor)
    }
}

#[derive(Clone)]
pub struct OwnerApiConfig {
    service: OwnerSkillManagementService,
    auth: OwnerAuth,
}

impl OwnerApiConfig {
    pub fn new(service: OwnerSkillManagementService, auth: OwnerAuth) -> Self {
        Self { service, auth }
    }

    pub fn management_service(&self) -> OwnerSkillManagementService {
        self.service.clone()
    }

    fn policy(&self) -> &SkillManagementPolicy {
        self.service.policy()
    }
}

#[derive(Debug, Serialize)]
struct OwnerSkillsResponse {
    effective: Vec<SkillPackageStatus>,
    managed: Vec<SkillPackageStatus>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateDraftBody {
    files: Vec<DraftFileUpdate>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferBody {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyBody {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum ApprovalDecision {
    Approve,
    Reject,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveApprovalBody {
    decision: ApprovalDecision,
}

pub(crate) fn router(state: &Arc<AppState>) -> Option<Router<Arc<AppState>>> {
    let owner = state.owner_management()?;
    if owner.policy().mode == SkillManagementMode::Disabled {
        return None;
    }
    let mut router = Router::new()
        .route("/owner/policy", get(owner_policy))
        .route("/owner/skills", get(list_skills))
        .route("/owner/skills/{package_id}/audit", get(list_audit));
    if allows_route(owner, SkillOperation::CreateDraft) {
        router = router.route("/owner/skills/drafts", post(create_draft));
    }
    if allows_route(owner, SkillOperation::Import) {
        router = router.route("/owner/skills/drafts/import", post(import_draft));
    }
    if allows_route(owner, SkillOperation::EditDraft) {
        router = router.route("/owner/skills/drafts/{revision_id}", put(update_draft));
    }
    if allows_route(owner, SkillOperation::Validate) {
        router = router.route(
            "/owner/skills/drafts/{revision_id}/validate",
            post(validate_draft),
        );
    }
    if allows_route(owner, SkillOperation::Test) {
        router = router.route("/owner/skills/drafts/{revision_id}/test", post(test_draft));
    }
    if allows_route(owner, SkillOperation::Activate) {
        router = router
            .route(
                "/owner/skills/drafts/{revision_id}/activation",
                post(request_activation),
            )
            .route(
                "/owner/skills/approvals/{approval_id}",
                post(resolve_approval),
            );
    }
    if allows_route(owner, SkillOperation::Export) {
        router = router.route("/owner/skills/{package_id}/export", post(export_skill));
    }
    Some(router.route_layer(middleware::from_fn_with_state(state.clone(), require_owner)))
}

fn allows_route(owner: &OwnerApiConfig, operation: SkillOperation) -> bool {
    owner.auth.actors().any(|actor| {
        owner
            .policy()
            .allowed_kinds
            .iter()
            .copied()
            .any(|kind| owner.policy().allows(actor, operation, kind))
    })
}

async fn owner_policy(
    State(state): State<Arc<AppState>>,
    Extension(_actor): Extension<ActorContext>,
) -> Result<Json<SkillManagementPolicy>, OwnerApiError> {
    let owner = owner_config(&state)?;
    Ok(Json(owner.policy().clone()))
}

async fn list_skills(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
) -> Result<Json<OwnerSkillsResponse>, OwnerApiError> {
    let owner = owner_config(&state)?;
    let effective = owner
        .service
        .list_effective_skills(&actor)
        .await
        .map_err(OwnerApiError::from_service)?;
    let managed = owner
        .service
        .list_managed_skills(&actor)
        .await
        .map_err(OwnerApiError::from_service)?;
    Ok(Json(OwnerSkillsResponse { effective, managed }))
}

async fn list_audit(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    Path(package_id): Path<String>,
) -> Result<Json<Vec<SkillAuditRecord>>, OwnerApiError> {
    let owner = owner_config(&state)?;
    let package_id = agent_runtime::skill_package::SkillPackageId::parse(&package_id)
        .map_err(|_| OwnerApiError::BadRequest)?;
    let audit = owner
        .service
        .list_audit(&actor, &package_id)
        .await
        .map_err(OwnerApiError::from_service)?;
    Ok(Json(audit))
}

async fn create_draft(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    payload: Result<Json<CreateSkillDraftRequest>, JsonRejection>,
) -> Result<impl IntoResponse, OwnerApiError> {
    let owner = owner_config(&state)?;
    let Json(request) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    let summary = owner
        .service
        .create_draft(&actor, request)
        .await
        .map_err(OwnerApiError::from_service)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn update_draft(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    Path(revision_id): Path<String>,
    payload: Result<Json<UpdateDraftBody>, JsonRejection>,
) -> Result<Json<agent_runtime::skill_management::SkillDraftSummary>, OwnerApiError> {
    validate_uuid(&revision_id)?;
    let Json(body) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    Ok(Json(
        owner_config(&state)?
            .service
            .update_draft(&actor, &revision_id, body.files)
            .await
            .map_err(OwnerApiError::from_service)?,
    ))
}

async fn validate_draft(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    Path(revision_id): Path<String>,
    payload: Result<Json<EmptyBody>, JsonRejection>,
) -> Result<Json<agent_runtime::skill_management::SkillDraftValidation>, OwnerApiError> {
    validate_uuid(&revision_id)?;
    let Json(_) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    Ok(Json(
        owner_config(&state)?
            .service
            .validate_draft(&actor, &revision_id)
            .await
            .map_err(OwnerApiError::from_service)?,
    ))
}

async fn test_draft(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    Path(revision_id): Path<String>,
    payload: Result<Json<EmptyBody>, JsonRejection>,
) -> Result<Json<agent_runtime::skill_management::SkillDraftTestResult>, OwnerApiError> {
    validate_uuid(&revision_id)?;
    let Json(_) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    Ok(Json(
        owner_config(&state)?
            .service
            .test_draft(&actor, &revision_id)
            .await
            .map_err(OwnerApiError::from_service)?,
    ))
}

async fn request_activation(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    Path(revision_id): Path<String>,
    payload: Result<Json<EmptyBody>, JsonRejection>,
) -> Result<impl IntoResponse, OwnerApiError> {
    validate_uuid(&revision_id)?;
    let Json(_) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    let approval = owner_config(&state)?
        .service
        .request_activation(&actor, &revision_id)
        .await
        .map_err(OwnerApiError::from_service)?;
    Ok((StatusCode::ACCEPTED, Json(approval_json(&approval))))
}

async fn import_draft(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    payload: Result<Json<TransferBody>, JsonRejection>,
) -> Result<impl IntoResponse, OwnerApiError> {
    let Json(body) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    let summary = owner_config(&state)?
        .service
        .import_draft(&actor, std::path::Path::new(&body.name))
        .await
        .map_err(OwnerApiError::from_service)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn export_skill(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    Path(package_id): Path<String>,
    payload: Result<Json<TransferBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, OwnerApiError> {
    let package_id = agent_runtime::skill_package::SkillPackageId::parse(&package_id)
        .map_err(|_| OwnerApiError::BadRequest)?;
    let Json(body) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    owner_config(&state)?
        .service
        .export_managed_skill(&actor, &package_id, std::path::Path::new(&body.name))
        .await
        .map_err(OwnerApiError::from_service)?;
    Ok(Json(serde_json::json!({"name": body.name})))
}

async fn resolve_approval(
    State(state): State<Arc<AppState>>,
    Extension(actor): Extension<ActorContext>,
    Path(approval_id): Path<String>,
    payload: Result<Json<ResolveApprovalBody>, JsonRejection>,
) -> Result<Json<serde_json::Value>, OwnerApiError> {
    validate_uuid(&approval_id)?;
    let Json(body) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    let value = match body.decision {
        ApprovalDecision::Approve => {
            let report = owner_config(&state)?
                .service
                .approve_activation(&approval_id, &actor)
                .await
                .map_err(OwnerApiError::from_service)?;
            serde_json::json!({
                "active_generation": report.active_generation,
                "active_packages": report.active_packages,
                "inactive_packages": report.inactive_packages,
                "previous_generation": report.previous_generation,
                "status": "approved"
            })
        }
        ApprovalDecision::Reject => {
            let approval = owner_config(&state)?
                .service
                .reject_activation(&approval_id, &actor)
                .await
                .map_err(OwnerApiError::from_service)?;
            approval_json(&approval)
        }
    };
    Ok(Json(value))
}

fn approval_json(approval: &agent_runtime::skill_state::SkillApprovalRecord) -> serde_json::Value {
    serde_json::json!({
        "approval_id": approval.approval_id,
        "package_id": approval.package_id.as_str(),
        "permission_diff": approval.permission_diff,
        "requested_by": approval.requested_by,
        "revision_id": approval.revision_id,
        "status": approval.status.as_str(),
    })
}

fn validate_uuid(value: &str) -> Result<(), OwnerApiError> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| OwnerApiError::BadRequest)?;
    if parsed.get_version() != Some(uuid::Version::Random) {
        return Err(OwnerApiError::BadRequest);
    }
    Ok(())
}

async fn require_owner(
    State(state): State<Arc<AppState>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let actor =
        match owner_config(&state).and_then(|owner| owner.auth.authenticate(request.headers())) {
            Ok(actor) => actor,
            Err(error) => return error.into_response(),
        };
    request.extensions_mut().insert(actor);
    next.run(request).await
}

fn owner_config(state: &AppState) -> Result<&OwnerApiConfig, OwnerApiError> {
    state
        .owner_management()
        .ok_or(OwnerApiError::Internal(anyhow::anyhow!(
            "owner management route has no configuration"
        )))
}

fn constant_time_eq(expected: &[u8], supplied: &[u8]) -> bool {
    let mut difference = expected.len() ^ supplied.len();
    for (index, expected_byte) in expected.iter().enumerate() {
        let supplied_byte = supplied.get(index).copied().unwrap_or_default();
        difference |= usize::from(expected_byte ^ supplied_byte);
    }
    difference == 0
}

#[derive(Debug)]
enum OwnerApiError {
    Unauthorized,
    Forbidden,
    BadRequest,
    NotFound,
    Conflict,
    Internal(anyhow::Error),
}

impl OwnerApiError {
    fn from_service(error: anyhow::Error) -> Self {
        match error.downcast_ref::<SkillManagementError>() {
            Some(SkillManagementError::Denied { .. }) => Self::Forbidden,
            Some(SkillManagementError::InvalidRequest(_)) => Self::BadRequest,
            Some(SkillManagementError::NotFound { .. }) => Self::NotFound,
            Some(SkillManagementError::Conflict { .. }) => Self::Conflict,
            Some(SkillManagementError::Internal { .. }) => Self::Internal(error),
            None => Self::Internal(error),
        }
    }
}

impl IntoResponse for OwnerApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "authentication required"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "operation forbidden"),
            Self::BadRequest => (StatusCode::BAD_REQUEST, "invalid request"),
            Self::NotFound => (StatusCode::NOT_FOUND, "resource not found"),
            Self::Conflict => (StatusCode::CONFLICT, "resource conflict"),
            Self::Internal(error) => {
                tracing::error!(?error, "owner skill management request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
            }
        };
        (
            status,
            Json(ErrorResponse {
                error: message.into(),
            }),
        )
            .into_response()
    }
}

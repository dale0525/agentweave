use crate::api::{AppState, ErrorResponse};
use agent_runtime::skill_management::{
    CreateSkillDraftRequest, OwnerSkillManagementService, SkillManagementError, SkillPackageStatus,
};
use agent_runtime::skill_management_tools::SkillManagementTools;
use agent_runtime::skill_policy::{ActorContext, SkillManagementMode, SkillManagementPolicy};
use agent_runtime::skill_state::SkillAuditRecord;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct OwnerAuth {
    bearer_token: Arc<[u8]>,
    actor: ActorContext,
}

impl OwnerAuth {
    pub fn new(token: impl AsRef<[u8]>, actor: ActorContext) -> anyhow::Result<Self> {
        let token = token.as_ref();
        if token.is_empty() {
            anyhow::bail!("owner bearer token cannot be empty");
        }
        Ok(Self {
            bearer_token: Arc::from(token),
            actor,
        })
    }

    fn authenticate(&self, headers: &HeaderMap) -> Result<ActorContext, OwnerApiError> {
        let supplied = headers
            .get(header::AUTHORIZATION)
            .map(|value| value.as_bytes())
            .ok_or(OwnerApiError::Unauthorized)?;
        let mut expected = Vec::with_capacity(7 + self.bearer_token.len());
        expected.extend_from_slice(b"Bearer ");
        expected.extend_from_slice(&self.bearer_token);
        if !constant_time_eq(&expected, supplied) {
            return Err(OwnerApiError::Unauthorized);
        }
        Ok(self.actor.clone())
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

pub(crate) fn router(state: &Arc<AppState>) -> Option<Router<Arc<AppState>>> {
    let owner = state.owner_management()?;
    if owner.policy().mode == SkillManagementMode::Disabled {
        return None;
    }
    let mut router = Router::new()
        .route("/owner/policy", get(owner_policy))
        .route("/owner/skills", get(list_skills))
        .route("/owner/skills/{package_id}/audit", get(list_audit));
    if !SkillManagementTools::definitions(&owner.service, &owner.auth.actor).is_empty() {
        router = router.route("/owner/skills/drafts", post(create_draft));
    }
    Some(router)
}

async fn owner_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<SkillManagementPolicy>, OwnerApiError> {
    let owner = owner_config(&state)?;
    owner.auth.authenticate(&headers)?;
    Ok(Json(owner.policy().clone()))
}

async fn list_skills(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<OwnerSkillsResponse>, OwnerApiError> {
    let owner = owner_config(&state)?;
    let actor = owner.auth.authenticate(&headers)?;
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
    headers: HeaderMap,
    Path(package_id): Path<String>,
) -> Result<Json<Vec<SkillAuditRecord>>, OwnerApiError> {
    let owner = owner_config(&state)?;
    let actor = owner.auth.authenticate(&headers)?;
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
    headers: HeaderMap,
    payload: Result<Json<CreateSkillDraftRequest>, JsonRejection>,
) -> Result<impl IntoResponse, OwnerApiError> {
    let owner = owner_config(&state)?;
    let actor = owner.auth.authenticate(&headers)?;
    let Json(request) = payload.map_err(|_| OwnerApiError::BadRequest)?;
    let summary = owner
        .service
        .create_draft(&actor, request)
        .await
        .map_err(OwnerApiError::from_service)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

fn owner_config(state: &AppState) -> Result<&OwnerApiConfig, OwnerApiError> {
    state
        .owner_management()
        .ok_or(OwnerApiError::Internal(anyhow::anyhow!(
            "owner management route has no configuration"
        )))
}

fn constant_time_eq(expected: &[u8], supplied: &[u8]) -> bool {
    if expected.len() != supplied.len() {
        return false;
    }
    expected
        .iter()
        .zip(supplied)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[derive(Debug)]
enum OwnerApiError {
    Unauthorized,
    Forbidden,
    BadRequest,
    Internal(anyhow::Error),
}

impl OwnerApiError {
    fn from_service(error: anyhow::Error) -> Self {
        match error.downcast_ref::<SkillManagementError>() {
            Some(SkillManagementError::Denied { .. }) => Self::Forbidden,
            Some(SkillManagementError::InvalidRequest(_)) => Self::BadRequest,
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

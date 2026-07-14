use crate::api::{ApiError, AppState};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;
use std::sync::Arc;

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/foundation/memory", get(list_memories))
        .route("/foundation/memory/export", get(export_memories))
        .route(
            "/foundation/memory/{memory_id}",
            get(get_memory).delete(forget_memory),
        )
        .route("/foundation/mail/accounts", get(list_mail_accounts))
        .route(
            "/foundation/mail/accounts/{account_id}",
            get(get_mail_account_status)
                .post(connect_mail_account)
                .delete(disconnect_mail_account),
        )
        .route(
            "/foundation/mail/send-approvals",
            post(request_mail_send_approval),
        )
        .route("/foundation/actions", get(list_foundation_actions))
        .route(
            "/foundation/actions/{approval_id}",
            post(resolve_foundation_action),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MemoryListQuery {
    #[serde(default)]
    query: String,
    #[serde(default = "default_memory_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForgetMemoryRequest {
    expected_version: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MailSendApprovalRequest {
    account_id: String,
    draft_id: String,
    expected_revision: u64,
    idempotency_key: String,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolveFoundationActionRequest {
    decision: agent_runtime::approval::ApprovalDecision,
}

async fn list_memories(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MemoryListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if query.limit == 0 || query.limit > 100 {
        return Err(ApiError::BadRequest(
            "memory limit must be between 1 and 100",
        ));
    }
    let runtime = state
        .memory_tools()
        .ok_or(ApiError::NotFound("Memory Foundation is disabled"))?;
    runtime
        .execute(
            "memory_search",
            serde_json::json!({"query": query.query, "limit": query.limit}),
        )
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn get_memory(
    State(state): State<Arc<AppState>>,
    Path(memory_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state
        .memory_tools()
        .ok_or(ApiError::NotFound("Memory Foundation is disabled"))?;
    let value = runtime
        .execute(
            "memory_get",
            serde_json::json!({"id": memory_id, "includeTombstone": false}),
        )
        .await
        .map_err(ApiError::Internal)?;
    if value.is_null() {
        return Err(ApiError::NotFound("memory not found"));
    }
    Ok(Json(value))
}

async fn forget_memory(
    State(state): State<Arc<AppState>>,
    Path(memory_id): Path<String>,
    Json(request): Json<ForgetMemoryRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state
        .memory_tools()
        .ok_or(ApiError::NotFound("Memory Foundation is disabled"))?;
    runtime
        .execute(
            "memory_forget",
            serde_json::json!({
                "id": memory_id,
                "expectedVersion": request.expected_version,
                "reason": "user_request"
            }),
        )
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn export_memories(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state
        .memory_tools()
        .ok_or(ApiError::NotFound("Memory Foundation is disabled"))?;
    runtime
        .execute(
            "memory_export",
            serde_json::json!({
                "includeProposals": false,
                "includeTombstones": false
            }),
        )
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn list_mail_accounts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_mail_read(&state, "mail_accounts_list", serde_json::json!({})).await
}

async fn get_mail_account_status(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_mail_read(
        &state,
        "mail_account_status",
        serde_json::json!({"accountId": account_id}),
    )
    .await
}

async fn connect_mail_account(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_mail_host_action(
        &state,
        "mail_account_connect",
        serde_json::json!({"accountId": account_id}),
    )
    .await
}

async fn disconnect_mail_account(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_mail_host_action(
        &state,
        "mail_account_disconnect",
        serde_json::json!({"accountId": account_id}),
    )
    .await
}

async fn request_mail_send_approval(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MailSendApprovalRequest>,
) -> Result<Json<agent_runtime::foundation_actions::PendingFoundationAction>, ApiError> {
    let preview = execute_mail_read(
        &state,
        "mail_send_preview",
        serde_json::json!({
            "accountId": request.account_id,
            "draftId": request.draft_id,
            "expectedRevision": request.expected_revision,
            "idempotencyKey": request.idempotency_key,
        }),
    )
    .await?
    .0;
    let preview =
        serde_json::from_value(preview).map_err(|error| ApiError::Internal(error.into()))?;
    state
        .mail_actions()
        .ok_or(ApiError::NotFound("Foundation action service is disabled"))?
        .request_send(preview, request.session_id, chrono::Utc::now())
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn list_foundation_actions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<agent_runtime::foundation_actions::PendingFoundationAction>>, ApiError> {
    state
        .mail_actions()
        .ok_or(ApiError::NotFound("Foundation action service is disabled"))?
        .list_actions()
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn resolve_foundation_action(
    State(state): State<Arc<AppState>>,
    Path(approval_id): Path<String>,
    Json(request): Json<ResolveFoundationActionRequest>,
) -> Result<Json<agent_runtime::foundation_actions::FoundationActionResolution>, ApiError> {
    state
        .mail_actions()
        .ok_or(ApiError::NotFound("Foundation action service is disabled"))?
        .resolve(
            &approval_id,
            request.decision,
            "local-user",
            chrono::Utc::now(),
        )
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn execute_mail_read(
    state: &AppState,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state
        .connector_tools()
        .ok_or(ApiError::NotFound("Mail Foundation is disabled"))?;
    let envelope = runtime
        .execute(tool, &uuid::Uuid::new_v4().to_string(), arguments)
        .await
        .map_err(ApiError::Internal)?;
    envelope
        .get("output")
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("connector output is missing")))
}

async fn execute_mail_host_action(
    state: &AppState,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state
        .connector_tools()
        .ok_or(ApiError::NotFound("Mail Foundation is disabled"))?;
    let envelope = runtime
        .execute_trusted_host_action(tool, &uuid::Uuid::new_v4().to_string(), arguments)
        .await
        .map_err(ApiError::Internal)?;
    envelope
        .get("output")
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("connector output is missing")))
}

fn default_memory_limit() -> usize {
    50
}

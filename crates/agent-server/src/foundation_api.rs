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
        .route("/foundation/calendar/events", get(list_calendar_events))
        .route(
            "/foundation/calendar/events/{event_id}",
            get(get_calendar_event),
        )
        .route(
            "/foundation/calendar/free-busy",
            get(get_calendar_free_busy),
        )
        .route(
            "/foundation/calendar/create-approvals",
            post(request_calendar_create_approval),
        )
        .route(
            "/foundation/calendar/update-approvals",
            post(request_calendar_update_approval),
        )
        .route(
            "/foundation/calendar/cancel-approvals",
            post(request_calendar_cancel_approval),
        )
        .route("/foundation/contacts", get(resolve_contacts))
        .route("/foundation/contacts/{contact_id}", get(get_contact))
        .route(
            "/foundation/contacts/update-approvals",
            post(request_contact_update_approval),
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
struct CalendarRangeQuery {
    account_id: String,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CalendarEventQuery {
    account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CalendarCreateApprovalRequest {
    account_id: String,
    content: agent_runtime::calendar::CalendarEventContent,
    idempotency_key: String,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CalendarUpdateApprovalRequest {
    account_id: String,
    event_id: String,
    expected_version: u64,
    content: agent_runtime::calendar::CalendarEventContent,
    idempotency_key: String,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CalendarCancelApprovalRequest {
    account_id: String,
    event_id: String,
    expected_version: u64,
    idempotency_key: String,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContactsResolveQuery {
    account_id: String,
    query: String,
    #[serde(default = "default_contacts_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContactQuery {
    account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContactUpdateApprovalRequest {
    account_id: String,
    contact_id: String,
    expected_version: u64,
    replacement: agent_runtime::contacts::ContactRecord,
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

async fn list_calendar_events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CalendarRangeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_connector_read(
        &state,
        "calendar_events_list",
        serde_json::json!({
            "accountId": query.account_id,
            "start": query.start,
            "end": query.end,
        }),
        "Calendar Foundation is disabled",
    )
    .await
}

async fn get_calendar_event(
    State(state): State<Arc<AppState>>,
    Path(event_id): Path<String>,
    Query(query): Query<CalendarEventQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_connector_read(
        &state,
        "calendar_event_get",
        serde_json::json!({
            "accountId": query.account_id,
            "eventId": event_id,
        }),
        "Calendar Foundation is disabled",
    )
    .await
}

async fn get_calendar_free_busy(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CalendarRangeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_connector_read(
        &state,
        "calendar_free_busy",
        serde_json::json!({
            "accountId": query.account_id,
            "start": query.start,
            "end": query.end,
        }),
        "Calendar Foundation is disabled",
    )
    .await
}

async fn request_calendar_create_approval(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CalendarCreateApprovalRequest>,
) -> Result<Json<agent_runtime::calendar_actions::PendingCalendarAction>, ApiError> {
    request_calendar_approval(
        &state,
        "calendar_event_create_preview",
        serde_json::json!({
            "accountId": request.account_id,
            "content": request.content,
            "idempotencyKey": request.idempotency_key,
        }),
        request.session_id,
    )
    .await
}

async fn request_calendar_update_approval(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CalendarUpdateApprovalRequest>,
) -> Result<Json<agent_runtime::calendar_actions::PendingCalendarAction>, ApiError> {
    request_calendar_approval(
        &state,
        "calendar_event_update_preview",
        serde_json::json!({
            "accountId": request.account_id,
            "eventId": request.event_id,
            "expectedVersion": request.expected_version,
            "content": request.content,
            "idempotencyKey": request.idempotency_key,
        }),
        request.session_id,
    )
    .await
}

async fn request_calendar_cancel_approval(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CalendarCancelApprovalRequest>,
) -> Result<Json<agent_runtime::calendar_actions::PendingCalendarAction>, ApiError> {
    request_calendar_approval(
        &state,
        "calendar_event_cancel_preview",
        serde_json::json!({
            "accountId": request.account_id,
            "eventId": request.event_id,
            "expectedVersion": request.expected_version,
            "idempotencyKey": request.idempotency_key,
        }),
        request.session_id,
    )
    .await
}

async fn request_calendar_approval(
    state: &AppState,
    preview_tool: &str,
    arguments: serde_json::Value,
    session_id: Option<String>,
) -> Result<Json<agent_runtime::calendar_actions::PendingCalendarAction>, ApiError> {
    let preview = execute_connector_read(
        state,
        preview_tool,
        arguments,
        "Calendar Foundation is disabled",
    )
    .await?
    .0;
    let preview =
        serde_json::from_value(preview).map_err(|error| ApiError::Internal(error.into()))?;
    state
        .calendar_actions()
        .ok_or(ApiError::NotFound(
            "Calendar Foundation action service is disabled",
        ))?
        .request(preview, session_id, chrono::Utc::now())
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn resolve_contacts(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ContactsResolveQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !(1..=50).contains(&query.limit) {
        return Err(ApiError::BadRequest(
            "contact limit must be between 1 and 50",
        ));
    }
    execute_connector_read(
        &state,
        "contacts_resolve",
        serde_json::json!({
            "accountId": query.account_id,
            "query": query.query,
            "limit": query.limit,
        }),
        "Contacts Foundation is disabled",
    )
    .await
}

async fn get_contact(
    State(state): State<Arc<AppState>>,
    Path(contact_id): Path<String>,
    Query(query): Query<ContactQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_connector_read(
        &state,
        "contact_get",
        serde_json::json!({
            "accountId": query.account_id,
            "contactId": contact_id,
        }),
        "Contacts Foundation is disabled",
    )
    .await
}

async fn request_contact_update_approval(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ContactUpdateApprovalRequest>,
) -> Result<Json<agent_runtime::contacts_actions::PendingContactAction>, ApiError> {
    let preview = execute_connector_read(
        &state,
        "contact_update_preview",
        serde_json::json!({
            "accountId": request.account_id,
            "contactId": request.contact_id,
            "expectedVersion": request.expected_version,
            "replacement": request.replacement,
            "idempotencyKey": request.idempotency_key,
        }),
        "Contacts Foundation is disabled",
    )
    .await?
    .0;
    let preview =
        serde_json::from_value(preview).map_err(|error| ApiError::Internal(error.into()))?;
    state
        .contacts_actions()
        .ok_or(ApiError::NotFound(
            "Contacts Foundation action service is disabled",
        ))?
        .request(preview, request.session_id, chrono::Utc::now())
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn list_foundation_actions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let mut actions = Vec::new();
    let mut enabled = false;
    if let Some(service) = state.mail_actions() {
        enabled = true;
        for action in service.list_actions().await.map_err(ApiError::Internal)? {
            actions.push(
                serde_json::to_value(action).map_err(|error| ApiError::Internal(error.into()))?,
            );
        }
    }
    if let Some(service) = state.calendar_actions() {
        enabled = true;
        for action in service.list_actions().await.map_err(ApiError::Internal)? {
            actions.push(
                serde_json::to_value(action).map_err(|error| ApiError::Internal(error.into()))?,
            );
        }
    }
    if let Some(service) = state.contacts_actions() {
        enabled = true;
        for action in service.list_actions().await.map_err(ApiError::Internal)? {
            actions.push(
                serde_json::to_value(action).map_err(|error| ApiError::Internal(error.into()))?,
            );
        }
    }
    if !enabled {
        return Err(ApiError::NotFound("Foundation action service is disabled"));
    }
    actions.sort_by(|left, right| {
        right["approval"]["created_at"]
            .as_str()
            .cmp(&left["approval"]["created_at"].as_str())
    });
    Ok(Json(actions))
}

async fn resolve_foundation_action(
    State(state): State<Arc<AppState>>,
    Path(approval_id): Path<String>,
    Json(request): Json<ResolveFoundationActionRequest>,
) -> Result<Json<agent_runtime::foundation_actions::FoundationActionResolution>, ApiError> {
    if let Some(service) = state.contacts_actions()
        && service
            .handles_approval(&approval_id)
            .await
            .map_err(ApiError::Internal)?
    {
        return service
            .resolve(
                &approval_id,
                request.decision,
                "local-user",
                chrono::Utc::now(),
            )
            .await
            .map(Json)
            .map_err(ApiError::Internal);
    }
    if let Some(service) = state.calendar_actions()
        && service
            .handles_approval(&approval_id)
            .await
            .map_err(ApiError::Internal)?
    {
        return service
            .resolve(
                &approval_id,
                request.decision,
                "local-user",
                chrono::Utc::now(),
            )
            .await
            .map(Json)
            .map_err(ApiError::Internal);
    }
    if let Some(service) = state.mail_actions() {
        return service
            .resolve(
                &approval_id,
                request.decision,
                "local-user",
                chrono::Utc::now(),
            )
            .await
            .map(Json)
            .map_err(ApiError::Internal);
    }
    Err(ApiError::NotFound("Foundation action service is disabled"))
}

async fn execute_mail_read(
    state: &AppState,
    tool: &str,
    arguments: serde_json::Value,
) -> Result<Json<serde_json::Value>, ApiError> {
    execute_connector_read(state, tool, arguments, "Mail Foundation is disabled").await
}

async fn execute_connector_read(
    state: &AppState,
    tool: &str,
    arguments: serde_json::Value,
    disabled_message: &'static str,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = state
        .connector_tools()
        .ok_or(ApiError::NotFound(disabled_message))?;
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

fn default_contacts_limit() -> usize {
    20
}

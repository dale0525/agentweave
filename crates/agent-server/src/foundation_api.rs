use crate::api::{ApiError, AppState};
use agent_runtime::credential::{CredentialScope, SecretMaterial};
use agent_runtime::mail::{MailAccount, MailAddress, ProviderReference};
use agent_runtime::mail_imap_smtp::{ImapSmtpMailConfig, MailTlsMode};
use agent_runtime::mail_imap_smtp_accounts::IMAP_SMTP_PROVIDER_ID;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::{Deserialize, Deserializer, Serialize};
use std::{fmt, sync::Arc};
use zeroize::Zeroize;

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
            "/foundation/mail/account-configurations",
            get(list_mail_account_configurations),
        )
        .route(
            "/foundation/mail/account-configurations/{account_id}",
            get(get_mail_account_configuration)
                .put(put_mail_account_configuration)
                .delete(delete_mail_account_configuration),
        )
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

struct RedactedPassword(Vec<u8>);

impl<'de> Deserialize<'de> for RedactedPassword {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)
            .map(String::into_bytes)
            .map(Self)
    }
}

impl fmt::Debug for RedactedPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RedactedPassword([REDACTED])")
    }
}

impl Drop for RedactedPassword {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl RedactedPassword {
    fn validate(&self) -> Result<(), ApiError> {
        if self.0.is_empty()
            || self.0.len() > 64 * 1024
            || std::str::from_utf8(&self.0)
                .map(str::trim)
                .map_or(true, str::is_empty)
        {
            return Err(ApiError::BadRequest(
                "Mail account configuration is invalid",
            ));
        }
        Ok(())
    }

    fn into_secret(mut self) -> anyhow::Result<SecretMaterial> {
        SecretMaterial::new(std::mem::take(&mut self.0))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MailAccountConfigurationRequest {
    display_name: String,
    primary_name: Option<String>,
    primary_address: String,
    username: String,
    password: RedactedPassword,
    imap_host: String,
    imap_port: u16,
    imap_tls: MailTlsMode,
    smtp_host: String,
    smtp_port: u16,
    smtp_tls: MailTlsMode,
    archive_mailbox: Option<String>,
    sent_mailbox: Option<String>,
    drafts_mailbox: Option<String>,
    trash_mailbox: Option<String>,
    #[serde(default)]
    allow_insecure_localhost: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MailAccountConfigurationResponse {
    id: String,
    display_name: String,
    primary_name: Option<String>,
    primary_address: String,
    username: String,
    imap_host: String,
    imap_port: u16,
    imap_tls: MailTlsMode,
    smtp_host: String,
    smtp_port: u16,
    smtp_tls: MailTlsMode,
    archive_mailbox: Option<String>,
    sent_mailbox: Option<String>,
    drafts_mailbox: Option<String>,
    trash_mailbox: Option<String>,
    allow_insecure_localhost: bool,
    credential_configured: bool,
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

async fn list_mail_account_configurations(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<MailAccountConfigurationResponse>>, ApiError> {
    let manager = state.mail_account_manager().ok_or(ApiError::NotFound(
        "Mail account configuration is unavailable",
    ))?;
    let scope = mail_credential_scope(&state);
    let configs = manager.list(&scope).await.map_err(ApiError::Internal)?;
    let mut response = Vec::with_capacity(configs.len());
    for config in configs {
        response.push(
            mail_configuration_response(manager.as_ref(), &scope, config)
                .await
                .map_err(ApiError::Internal)?,
        );
    }
    Ok(Json(response))
}

async fn get_mail_account_configuration(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<String>,
) -> Result<Json<MailAccountConfigurationResponse>, ApiError> {
    validate_mail_account_id(&account_id)?;
    let manager = state.mail_account_manager().ok_or(ApiError::NotFound(
        "Mail account configuration is unavailable",
    ))?;
    let scope = mail_credential_scope(&state);
    let config = manager
        .get(&scope, &account_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound(
            "Mail account configuration was not found",
        ))?;
    mail_configuration_response(manager.as_ref(), &scope, config)
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn put_mail_account_configuration(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<String>,
    Json(request): Json<MailAccountConfigurationRequest>,
) -> Result<Json<MailAccountConfigurationResponse>, ApiError> {
    validate_mail_account_id(&account_id)?;
    let manager = state.mail_account_manager().ok_or(ApiError::NotFound(
        "Mail account configuration is unavailable",
    ))?;
    let scope = mail_credential_scope(&state);
    let (config, password) = mail_configuration(&account_id, scope.clone(), request)?;
    let config = manager
        .configure(config, password)
        .await
        .map_err(ApiError::Internal)?;
    mail_configuration_response(manager.as_ref(), &scope, config)
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn delete_mail_account_configuration(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_mail_account_id(&account_id)?;
    let manager = state.mail_account_manager().ok_or(ApiError::NotFound(
        "Mail account configuration is unavailable",
    ))?;
    manager
        .delete(&mail_credential_scope(&state), &account_id)
        .await
        .map(|deleted| Json(serde_json::json!({"deleted": deleted})))
        .map_err(ApiError::Internal)
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

fn mail_credential_scope(state: &AppState) -> CredentialScope {
    CredentialScope {
        app_id: state.app_prompt().identity.app_id.clone(),
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    }
}

fn mail_configuration(
    account_id: &str,
    scope: CredentialScope,
    request: MailAccountConfigurationRequest,
) -> Result<(ImapSmtpMailConfig, SecretMaterial), ApiError> {
    for value in [
        &request.display_name,
        &request.primary_address,
        &request.username,
        &request.imap_host,
        &request.smtp_host,
    ] {
        if value.trim().is_empty() || value.len() > 512 {
            return Err(ApiError::BadRequest(
                "Mail account configuration is invalid",
            ));
        }
    }
    for value in [
        request.primary_name.as_ref(),
        request.archive_mailbox.as_ref(),
        request.sent_mailbox.as_ref(),
        request.drafts_mailbox.as_ref(),
        request.trash_mailbox.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if value.len() > 512 {
            return Err(ApiError::BadRequest(
                "Mail account configuration is invalid",
            ));
        }
    }
    request.password.validate()?;
    let password = request
        .password
        .into_secret()
        .map_err(|_| ApiError::BadRequest("Mail account configuration is invalid"))?;
    let credential_secret_id =
        agent_runtime::mail_imap_smtp_accounts::mail_secret_id(&scope, account_id)
            .map_err(|_| ApiError::BadRequest("Mail account configuration is invalid"))?;
    let config = ImapSmtpMailConfig {
        account: MailAccount {
            id: account_id.to_string(),
            display_name: request.display_name,
            primary_address: MailAddress {
                name: request.primary_name.filter(|value| !value.is_empty()),
                address: request.primary_address,
            },
            addresses: Vec::new(),
            provider_reference: Some(ProviderReference {
                provider: IMAP_SMTP_PROVIDER_ID.into(),
                id: account_id.to_string(),
            }),
        },
        credential_scope: scope,
        credential_secret_id,
        imap_host: request.imap_host,
        imap_port: request.imap_port,
        imap_tls: request.imap_tls,
        smtp_host: request.smtp_host,
        smtp_port: request.smtp_port,
        smtp_tls: request.smtp_tls,
        username: request.username,
        archive_mailbox: request.archive_mailbox.filter(|value| !value.is_empty()),
        sent_mailbox: request.sent_mailbox.filter(|value| !value.is_empty()),
        drafts_mailbox: request.drafts_mailbox.filter(|value| !value.is_empty()),
        trash_mailbox: request.trash_mailbox.filter(|value| !value.is_empty()),
        allow_insecure_localhost: request.allow_insecure_localhost,
        connect_timeout_seconds: 15,
        operation_timeout_seconds: 30,
    };
    config
        .validate()
        .map_err(|_| ApiError::BadRequest("Mail account configuration is invalid"))?;
    Ok((config, password))
}

async fn mail_configuration_response(
    manager: &agent_runtime::mail_imap_smtp_accounts::ImapSmtpMailAccountManager,
    scope: &CredentialScope,
    config: ImapSmtpMailConfig,
) -> anyhow::Result<MailAccountConfigurationResponse> {
    let credential_configured = manager
        .credential_configured(scope, &config.account.id)
        .await?;
    Ok(MailAccountConfigurationResponse {
        id: config.account.id,
        display_name: config.account.display_name,
        primary_name: config.account.primary_address.name,
        primary_address: config.account.primary_address.address,
        username: config.username,
        imap_host: config.imap_host,
        imap_port: config.imap_port,
        imap_tls: config.imap_tls,
        smtp_host: config.smtp_host,
        smtp_port: config.smtp_port,
        smtp_tls: config.smtp_tls,
        archive_mailbox: config.archive_mailbox,
        sent_mailbox: config.sent_mailbox,
        drafts_mailbox: config.drafts_mailbox,
        trash_mailbox: config.trash_mailbox,
        allow_insecure_localhost: config.allow_insecure_localhost,
        credential_configured,
    })
}

fn validate_mail_account_id(account_id: &str) -> Result<(), ApiError> {
    if account_id.is_empty()
        || account_id.len() > 255
        || !account_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err(ApiError::BadRequest("Mail account identifier is invalid"));
    }
    Ok(())
}

fn default_memory_limit() -> usize {
    50
}

#[cfg(test)]
mod mail_configuration_tests {
    use super::*;

    #[test]
    fn request_debug_redacts_password() {
        let password = "debug-credential-marker";
        let request: MailAccountConfigurationRequest = serde_json::from_value(serde_json::json!({
            "displayName": "Primary Mail",
            "primaryName": "Local User",
            "primaryAddress": "user@example.test",
            "username": "user@example.test",
            "password": password,
            "imapHost": "imap.example.test",
            "imapPort": 993,
            "imapTls": "implicit",
            "smtpHost": "smtp.example.test",
            "smtpPort": 587,
            "smtpTls": "start_tls"
        }))
        .unwrap();

        let debug = format!("{request:?}");

        assert!(!debug.contains(password));
        assert!(debug.contains("[REDACTED]"));
    }
}

use crate::api::{ApiError, AppState};
use agent_runtime::automation::{
    NotificationDeliveryOutcome, NotificationRecord, NotificationScope, NotificationStore,
};
use agent_runtime::scheduler::{
    ScheduledJob, ScheduledJobRequest, ScheduledJobStatus, SchedulerStore,
};
use agent_runtime::storage::Storage;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct AutomationApiState {
    scheduler: SchedulerStore,
    notifications: NotificationStore,
}

impl AutomationApiState {
    pub(crate) async fn from_storage(storage: &Storage) -> anyhow::Result<Self> {
        Ok(Self {
            scheduler: SchedulerStore::from_storage(storage).await?,
            notifications: NotificationStore::from_storage(storage).await?,
        })
    }
}

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/foundation/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/foundation/schedules/{job_id}",
            get(get_schedule).post(set_schedule_status),
        )
        .route("/foundation/notifications/claim", get(claim_notifications))
        .route(
            "/foundation/notifications/{notification_id}",
            post(finish_notification),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduleListQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetScheduleStatusRequest {
    expected_version: i64,
    status: ScheduledJobStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NotificationClaimQuery {
    channel: String,
    worker: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FinishNotificationRequest {
    worker: String,
    outcome: NotificationDeliveryOutcome,
}

async fn list_schedules(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ScheduleListQuery>,
) -> Result<Json<Vec<ScheduledJob>>, ApiError> {
    validate_limit(query.limit)?;
    state
        .automation()
        .ok_or(ApiError::NotFound("Automation Foundation is disabled"))?
        .scheduler
        .list_jobs(
            &state.app_prompt().identity.app_id,
            "local",
            "local-user",
            query.limit,
        )
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn create_schedule(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ScheduledJobRequest>,
) -> Result<Json<ScheduledJob>, ApiError> {
    if request.app_id != state.app_prompt().identity.app_id
        || request.tenant_id != "local"
        || request.user_id != "local-user"
    {
        return Err(ApiError::BadRequest(
            "schedule scope does not match the active App",
        ));
    }
    state
        .automation()
        .ok_or(ApiError::NotFound("Automation Foundation is disabled"))?
        .scheduler
        .create_job(request, Utc::now())
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn get_schedule(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<ScheduledJob>, ApiError> {
    let job = state
        .automation()
        .ok_or(ApiError::NotFound("Automation Foundation is disabled"))?
        .scheduler
        .get_job(&job_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound("scheduled job not found"))?;
    ensure_job_scope(&state, &job)?;
    Ok(Json(job))
}

async fn set_schedule_status(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(request): Json<SetScheduleStatusRequest>,
) -> Result<Json<ScheduledJob>, ApiError> {
    let automation = state
        .automation()
        .ok_or(ApiError::NotFound("Automation Foundation is disabled"))?;
    let current = automation
        .scheduler
        .get_job(&job_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound("scheduled job not found"))?;
    ensure_job_scope(&state, &current)?;
    let updated = automation
        .scheduler
        .set_status(
            &job_id,
            request.expected_version,
            request.status,
            Utc::now(),
        )
        .await
        .map_err(ApiError::Internal)?;
    if !updated {
        return Err(ApiError::BadRequest("scheduled job version conflict"));
    }
    automation
        .scheduler
        .get_job(&job_id)
        .await
        .map_err(ApiError::Internal)?
        .map(Json)
        .ok_or(ApiError::NotFound("scheduled job not found"))
}

async fn claim_notifications(
    State(state): State<Arc<AppState>>,
    Query(query): Query<NotificationClaimQuery>,
) -> Result<Json<Vec<NotificationRecord>>, ApiError> {
    validate_limit(query.limit)?;
    state
        .automation()
        .ok_or(ApiError::NotFound("Automation Foundation is disabled"))?
        .notifications
        .claim_due_for_scope_and_channel(
            &query.worker,
            NotificationScope {
                app_id: &state.app_prompt().identity.app_id,
                tenant_id: "local",
                user_id: "local-user",
            },
            &query.channel,
            Utc::now(),
            Duration::seconds(60),
            query.limit,
        )
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

async fn finish_notification(
    State(state): State<Arc<AppState>>,
    Path(notification_id): Path<String>,
    Json(request): Json<FinishNotificationRequest>,
) -> Result<Json<bool>, ApiError> {
    state
        .automation()
        .ok_or(ApiError::NotFound("Automation Foundation is disabled"))?
        .notifications
        .finish_for_scope(
            &notification_id,
            &request.worker,
            NotificationScope {
                app_id: &state.app_prompt().identity.app_id,
                tenant_id: "local",
                user_id: "local-user",
            },
            request.outcome,
            Utc::now(),
        )
        .await
        .map(Json)
        .map_err(ApiError::Internal)
}

fn ensure_job_scope(state: &AppState, job: &ScheduledJob) -> Result<(), ApiError> {
    if job.request.app_id == state.app_prompt().identity.app_id
        && job.request.tenant_id == "local"
        && job.request.user_id == "local-user"
    {
        Ok(())
    } else {
        Err(ApiError::NotFound("scheduled job not found"))
    }
}

fn validate_limit(limit: usize) -> Result<(), ApiError> {
    if (1..=100).contains(&limit) {
        Ok(())
    } else {
        Err(ApiError::BadRequest("limit must be between 1 and 100"))
    }
}

fn default_limit() -> usize {
    25
}

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarScope {
    pub app_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub account_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarAttendee {
    pub address: String,
    pub display_name: Option<String>,
    pub response: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventContent {
    pub calendar_id: String,
    pub title: String,
    pub description: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub timezone: String,
    pub location: Option<String>,
    pub attendees: Vec<CalendarAttendee>,
    pub recurrence: Option<String>,
}

impl CalendarEventContent {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.calendar_id.trim().is_empty(),
            "calendar id is required"
        );
        anyhow::ensure!(!self.title.trim().is_empty(), "event title is required");
        anyhow::ensure!(self.end > self.start, "event end must be after start");
        anyhow::ensure!(self.title.len() <= 1024, "event title is too long");
        anyhow::ensure!(self.attendees.len() <= 500, "event has too many attendees");
        self.timezone.parse::<chrono_tz::Tz>()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEvent {
    pub id: String,
    pub content: CalendarEventContent,
    pub status: CalendarEventStatus,
    pub version: u64,
    pub provider_id: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CalendarEventStatus {
    Confirmed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BusyInterval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub event_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CalendarMutationKind {
    Create,
    Update,
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarMutationPreview {
    pub preview_id: String,
    pub kind: CalendarMutationKind,
    pub event_id: Option<String>,
    pub expected_version: Option<u64>,
    pub content: Option<CalendarEventContent>,
    pub conflicts: Vec<BusyInterval>,
    pub attendee_visible: bool,
    pub preview_hash: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovedCalendarMutation {
    pub preview_id: String,
    pub preview_hash: String,
    pub approval_id: String,
}

#[async_trait]
pub trait CalendarConnector: Send + Sync {
    async fn list_events(
        &self,
        scope: &CalendarScope,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<Vec<CalendarEvent>>;
    async fn free_busy(
        &self,
        scope: &CalendarScope,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<Vec<BusyInterval>>;
    async fn preview_create(
        &self,
        scope: &CalendarScope,
        content: CalendarEventContent,
        idempotency_key: String,
    ) -> anyhow::Result<CalendarMutationPreview>;
    async fn preview_update(
        &self,
        scope: &CalendarScope,
        event_id: &str,
        expected_version: u64,
        content: CalendarEventContent,
        idempotency_key: String,
    ) -> anyhow::Result<CalendarMutationPreview>;
    async fn preview_cancel(
        &self,
        scope: &CalendarScope,
        event_id: &str,
        expected_version: u64,
        idempotency_key: String,
    ) -> anyhow::Result<CalendarMutationPreview>;
    async fn apply(
        &self,
        scope: &CalendarScope,
        approval: ApprovedCalendarMutation,
    ) -> anyhow::Result<CalendarEvent>;
}

#[derive(Clone, Default)]
pub struct FakeCalendarConnector {
    state: Arc<Mutex<FakeCalendarState>>,
}

#[derive(Default)]
struct FakeCalendarState {
    events: BTreeMap<String, (CalendarScope, CalendarEvent)>,
    previews: HashMap<String, (CalendarScope, CalendarMutationPreview)>,
    results: HashMap<String, CalendarEvent>,
}

impl FakeCalendarConnector {
    pub fn seed(&self, scope: CalendarScope, event: CalendarEvent) {
        self.state
            .lock()
            .expect("calendar lock poisoned")
            .events
            .insert(event.id.clone(), (scope, event));
    }

    fn preview(
        &self,
        scope: &CalendarScope,
        kind: CalendarMutationKind,
        event_id: Option<String>,
        expected_version: Option<u64>,
        content: Option<CalendarEventContent>,
        idempotency_key: String,
    ) -> anyhow::Result<CalendarMutationPreview> {
        anyhow::ensure!(
            !idempotency_key.trim().is_empty(),
            "calendar idempotency key is required"
        );
        if let Some(content) = &content {
            content.validate()?;
        }
        let state = self.state.lock().expect("calendar lock poisoned");
        if let (Some(id), Some(version)) = (&event_id, expected_version) {
            let (_, event) = state
                .events
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("calendar event not found"))?;
            anyhow::ensure!(event.version == version, "calendar event version conflict");
        }
        let conflicts = content
            .as_ref()
            .map(|candidate| {
                state
                    .events
                    .values()
                    .filter(|(event_scope, event)| {
                        event_scope == scope
                            && event.status == CalendarEventStatus::Confirmed
                            && event.content.start < candidate.end
                            && event.content.end > candidate.start
                            && Some(event.id.as_str()) != event_id.as_deref()
                    })
                    .map(|(_, event)| BusyInterval {
                        start: event.content.start,
                        end: event.content.end,
                        event_id: Some(event.id.clone()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        drop(state);
        let preview_id = Uuid::new_v4().to_string();
        let hash_input = serde_json::to_vec(&(
            kind,
            &event_id,
            expected_version,
            &content,
            &conflicts,
            &idempotency_key,
        ))?;
        let preview = CalendarMutationPreview {
            preview_id: preview_id.clone(),
            kind,
            event_id,
            expected_version,
            attendee_visible: content
                .as_ref()
                .is_some_and(|value| !value.attendees.is_empty()),
            content,
            conflicts,
            preview_hash: hex::encode(Sha256::digest(hash_input)),
            idempotency_key,
        };
        self.state
            .lock()
            .expect("calendar lock poisoned")
            .previews
            .insert(preview_id, (scope.clone(), preview.clone()));
        Ok(preview)
    }
}

#[async_trait]
impl CalendarConnector for FakeCalendarConnector {
    async fn list_events(
        &self,
        scope: &CalendarScope,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<Vec<CalendarEvent>> {
        anyhow::ensure!(end > start, "calendar range is invalid");
        Ok(self
            .state
            .lock()
            .expect("calendar lock poisoned")
            .events
            .values()
            .filter(|(event_scope, event)| {
                event_scope == scope && event.content.start < end && event.content.end > start
            })
            .map(|(_, event)| event.clone())
            .collect())
    }

    async fn free_busy(
        &self,
        scope: &CalendarScope,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> anyhow::Result<Vec<BusyInterval>> {
        Ok(self
            .list_events(scope, start, end)
            .await?
            .into_iter()
            .filter(|event| event.status == CalendarEventStatus::Confirmed)
            .map(|event| BusyInterval {
                start: event.content.start,
                end: event.content.end,
                event_id: Some(event.id),
            })
            .collect())
    }

    async fn preview_create(
        &self,
        scope: &CalendarScope,
        content: CalendarEventContent,
        idempotency_key: String,
    ) -> anyhow::Result<CalendarMutationPreview> {
        self.preview(
            scope,
            CalendarMutationKind::Create,
            None,
            None,
            Some(content),
            idempotency_key,
        )
    }

    async fn preview_update(
        &self,
        scope: &CalendarScope,
        event_id: &str,
        expected_version: u64,
        content: CalendarEventContent,
        idempotency_key: String,
    ) -> anyhow::Result<CalendarMutationPreview> {
        self.preview(
            scope,
            CalendarMutationKind::Update,
            Some(event_id.into()),
            Some(expected_version),
            Some(content),
            idempotency_key,
        )
    }

    async fn preview_cancel(
        &self,
        scope: &CalendarScope,
        event_id: &str,
        expected_version: u64,
        idempotency_key: String,
    ) -> anyhow::Result<CalendarMutationPreview> {
        self.preview(
            scope,
            CalendarMutationKind::Cancel,
            Some(event_id.into()),
            Some(expected_version),
            None,
            idempotency_key,
        )
    }

    async fn apply(
        &self,
        scope: &CalendarScope,
        approval: ApprovedCalendarMutation,
    ) -> anyhow::Result<CalendarEvent> {
        anyhow::ensure!(
            !approval.approval_id.trim().is_empty(),
            "calendar approval is required"
        );
        let mut state = self.state.lock().expect("calendar lock poisoned");
        let (preview_scope, preview) = state
            .previews
            .get(&approval.preview_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("calendar preview not found"))?;
        anyhow::ensure!(
            &preview_scope == scope && approval.preview_hash == preview.preview_hash,
            "calendar approval does not match preview"
        );
        if let Some(existing) = state.results.get(&preview.idempotency_key) {
            return Ok(existing.clone());
        }
        let event = match preview.kind {
            CalendarMutationKind::Create => CalendarEvent {
                id: Uuid::new_v4().to_string(),
                content: preview.content.unwrap(),
                status: CalendarEventStatus::Confirmed,
                version: 1,
                provider_id: None,
                updated_at: Utc::now(),
            },
            CalendarMutationKind::Update => {
                let id = preview.event_id.unwrap();
                let stored = state
                    .events
                    .get_mut(&id)
                    .ok_or_else(|| anyhow::anyhow!("calendar event not found"))?;
                anyhow::ensure!(
                    stored.1.version == preview.expected_version.unwrap(),
                    "calendar event version conflict"
                );
                stored.1.content = preview.content.unwrap();
                stored.1.version += 1;
                stored.1.updated_at = Utc::now();
                stored.1.clone()
            }
            CalendarMutationKind::Cancel => {
                let id = preview.event_id.unwrap();
                let stored = state
                    .events
                    .get_mut(&id)
                    .ok_or_else(|| anyhow::anyhow!("calendar event not found"))?;
                anyhow::ensure!(
                    stored.1.version == preview.expected_version.unwrap(),
                    "calendar event version conflict"
                );
                stored.1.status = CalendarEventStatus::Cancelled;
                stored.1.version += 1;
                stored.1.updated_at = Utc::now();
                stored.1.clone()
            }
        };
        state
            .events
            .insert(event.id.clone(), (scope.clone(), event.clone()));
        state.results.insert(preview.idempotency_key, event.clone());
        Ok(event)
    }
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;

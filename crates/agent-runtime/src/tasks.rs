use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskScope {
    pub app_id: String,
    pub tenant_id: String,
    pub user_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Urgent,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskContent {
    pub title: String,
    pub notes: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub timezone: Option<String>,
    pub recurrence: Option<String>,
    pub priority: TaskPriority,
    pub tags: Vec<String>,
}

impl TaskContent {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.title.trim().is_empty(), "task title is required");
        anyhow::ensure!(self.title.len() <= 1024, "task title is too long");
        anyhow::ensure!(
            self.notes
                .as_ref()
                .is_none_or(|value| value.len() <= 64 * 1024),
            "task notes are too long"
        );
        anyhow::ensure!(self.tags.len() <= 100, "task has too many tags");
        if let Some(timezone) = &self.timezone {
            timezone.parse::<chrono_tz::Tz>()?;
        }
        if self.recurrence.is_some() {
            anyhow::ensure!(self.due_at.is_some(), "recurring task requires a due time");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskRecord {
    pub id: String,
    pub content: TaskContent,
    pub status: TaskStatus,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskQuery {
    pub status: Option<TaskStatus>,
    pub due_before: Option<DateTime<Utc>>,
    pub tag: Option<String>,
    pub text: Option<String>,
    pub limit: usize,
}

#[async_trait]
pub trait TaskProvider: Send + Sync {
    async fn list(&self, scope: &TaskScope, query: TaskQuery) -> anyhow::Result<Vec<TaskRecord>>;
    async fn get(&self, scope: &TaskScope, task_id: &str) -> anyhow::Result<Option<TaskRecord>>;
    async fn create(
        &self,
        scope: &TaskScope,
        content: TaskContent,
        idempotency_key: &str,
    ) -> anyhow::Result<TaskRecord>;
    async fn update(
        &self,
        scope: &TaskScope,
        task_id: &str,
        expected_version: u64,
        content: TaskContent,
    ) -> anyhow::Result<TaskRecord>;
    async fn set_status(
        &self,
        scope: &TaskScope,
        task_id: &str,
        expected_version: u64,
        status: TaskStatus,
    ) -> anyhow::Result<TaskRecord>;
    async fn delete(
        &self,
        scope: &TaskScope,
        task_id: &str,
        expected_version: u64,
    ) -> anyhow::Result<bool>;
}

#[derive(Clone, Default)]
pub struct FakeTaskProvider {
    state: Arc<Mutex<FakeTaskState>>,
}

#[derive(Default)]
struct FakeTaskState {
    tasks: BTreeMap<(TaskScope, String), TaskRecord>,
    idempotency: HashMap<(TaskScope, String), String>,
}

#[async_trait]
impl TaskProvider for FakeTaskProvider {
    async fn list(&self, scope: &TaskScope, query: TaskQuery) -> anyhow::Result<Vec<TaskRecord>> {
        anyhow::ensure!((1..=100).contains(&query.limit), "task limit is invalid");
        let text = query.text.as_deref().map(str::to_lowercase);
        let mut values = self
            .state
            .lock()
            .expect("task lock poisoned")
            .tasks
            .iter()
            .filter(|((task_scope, _), task)| {
                task_scope == scope
                    && query.status.is_none_or(|status| task.status == status)
                    && query
                        .due_before
                        .is_none_or(|due| task.content.due_at.is_some_and(|value| value <= due))
                    && query
                        .tag
                        .as_ref()
                        .is_none_or(|tag| task.content.tags.contains(tag))
                    && text.as_ref().is_none_or(|text| {
                        task.content.title.to_lowercase().contains(text)
                            || task
                                .content
                                .notes
                                .as_ref()
                                .is_some_and(|notes| notes.to_lowercase().contains(text))
                    })
            })
            .map(|(_, task)| task.clone())
            .collect::<Vec<_>>();
        values.sort_by(|left, right| {
            left.content
                .due_at
                .cmp(&right.content.due_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        values.truncate(query.limit);
        Ok(values)
    }

    async fn get(&self, scope: &TaskScope, task_id: &str) -> anyhow::Result<Option<TaskRecord>> {
        Ok(self
            .state
            .lock()
            .expect("task lock poisoned")
            .tasks
            .get(&(scope.clone(), task_id.into()))
            .cloned())
    }

    async fn create(
        &self,
        scope: &TaskScope,
        content: TaskContent,
        idempotency_key: &str,
    ) -> anyhow::Result<TaskRecord> {
        content.validate()?;
        anyhow::ensure!(
            !idempotency_key.trim().is_empty(),
            "task idempotency key is required"
        );
        let mut state = self.state.lock().expect("task lock poisoned");
        if let Some(id) = state
            .idempotency
            .get(&(scope.clone(), idempotency_key.into()))
        {
            return Ok(state.tasks[&(scope.clone(), id.clone())].clone());
        }
        let now = Utc::now();
        let task = TaskRecord {
            id: Uuid::new_v4().to_string(),
            content,
            status: TaskStatus::Open,
            version: 1,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        state
            .idempotency
            .insert((scope.clone(), idempotency_key.into()), task.id.clone());
        state
            .tasks
            .insert((scope.clone(), task.id.clone()), task.clone());
        Ok(task)
    }

    async fn update(
        &self,
        scope: &TaskScope,
        task_id: &str,
        expected_version: u64,
        content: TaskContent,
    ) -> anyhow::Result<TaskRecord> {
        content.validate()?;
        mutate_task(&self.state, scope, task_id, expected_version, |task| {
            task.content = content
        })
    }

    async fn set_status(
        &self,
        scope: &TaskScope,
        task_id: &str,
        expected_version: u64,
        status: TaskStatus,
    ) -> anyhow::Result<TaskRecord> {
        mutate_task(&self.state, scope, task_id, expected_version, |task| {
            task.status = status;
            task.completed_at = (status == TaskStatus::Completed).then(Utc::now);
        })
    }

    async fn delete(
        &self,
        scope: &TaskScope,
        task_id: &str,
        expected_version: u64,
    ) -> anyhow::Result<bool> {
        let mut state = self.state.lock().expect("task lock poisoned");
        let key = (scope.clone(), task_id.into());
        let task = state
            .tasks
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("task not found"))?;
        anyhow::ensure!(task.version == expected_version, "task version conflict");
        state.tasks.remove(&key);
        Ok(true)
    }
}

fn mutate_task(
    state: &Arc<Mutex<FakeTaskState>>,
    scope: &TaskScope,
    task_id: &str,
    expected_version: u64,
    mutation: impl FnOnce(&mut TaskRecord),
) -> anyhow::Result<TaskRecord> {
    let mut state = state.lock().expect("task lock poisoned");
    let task = state
        .tasks
        .get_mut(&(scope.clone(), task_id.into()))
        .ok_or_else(|| anyhow::anyhow!("task not found"))?;
    anyhow::ensure!(task.version == expected_version, "task version conflict");
    mutation(task);
    task.version += 1;
    task.updated_at = Utc::now();
    Ok(task.clone())
}

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tests;

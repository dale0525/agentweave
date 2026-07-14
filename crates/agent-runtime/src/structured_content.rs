use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_FALLBACK_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StructuredContentAudience {
    User,
    Owner,
    Developer,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StructuredContent {
    pub content_id: String,
    pub mime_type: String,
    pub schema_version: String,
    pub payload: Value,
    pub fallback_text: String,
    pub audience: StructuredContentAudience,
    pub owner: String,
    pub revision: u64,
}

impl StructuredContent {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_id(&self.content_id, "content id")?;
        validate_id(&self.owner, "content owner")?;
        anyhow::ensure!(self.revision > 0, "content revision must be positive");
        anyhow::ensure!(
            valid_mime_type(&self.mime_type),
            "structured content MIME type is invalid"
        );
        anyhow::ensure!(
            !self.schema_version.trim().is_empty() && self.schema_version.len() <= 64,
            "structured content schema version is invalid"
        );
        anyhow::ensure!(
            serde_json::to_vec(&self.payload)?.len() <= MAX_PAYLOAD_BYTES,
            "structured content payload exceeds limit"
        );
        anyhow::ensure!(
            !self.fallback_text.trim().is_empty() && self.fallback_text.len() <= MAX_FALLBACK_BYTES,
            "structured content fallback text is invalid"
        );
        reject_active_content(&self.payload)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StructuredContentAction {
    pub action_id: String,
    pub content_id: String,
    pub content_revision: u64,
    pub owner: String,
    pub idempotency_key: String,
    pub expires_at: DateTime<Utc>,
    pub payload: Value,
}

impl StructuredContentAction {
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_id(&self.action_id, "action id")?;
        validate_id(&self.content_id, "content id")?;
        validate_id(&self.owner, "action owner")?;
        validate_id(&self.idempotency_key, "action idempotency key")?;
        anyhow::ensure!(
            serde_json::to_vec(&self.payload)?.len() <= MAX_PAYLOAD_BYTES,
            "structured action payload exceeds limit"
        );
        reject_active_content(&self.payload)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct StructuredActionReceipt {
    pub action_id: String,
    pub content_id: String,
    pub content_revision: u64,
    pub replayed: bool,
    pub payload: Value,
}

#[derive(Clone, Default)]
pub struct StructuredContentRegistry {
    inner: Arc<RwLock<StructuredContentState>>,
}

#[derive(Default)]
struct StructuredContentState {
    content: BTreeMap<String, StructuredContent>,
    consumed_actions: BTreeMap<String, StructuredActionReceipt>,
    deleted: BTreeSet<String>,
}

impl StructuredContentRegistry {
    pub fn publish(&self, content: StructuredContent) -> anyhow::Result<()> {
        content.validate()?;
        let mut state = self
            .inner
            .write()
            .expect("structured content lock poisoned");
        if let Some(previous) = state.content.get(&content.content_id) {
            anyhow::ensure!(
                previous.owner == content.owner,
                "structured content owner cannot change"
            );
            anyhow::ensure!(
                content.revision == previous.revision + 1,
                "structured content revision conflict"
            );
        } else {
            anyhow::ensure!(
                content.revision == 1,
                "initial content revision must be one"
            );
        }
        state.deleted.remove(&content.content_id);
        state.content.insert(content.content_id.clone(), content);
        Ok(())
    }

    pub fn get(&self, content_id: &str) -> Option<StructuredContent> {
        self.inner
            .read()
            .expect("structured content lock poisoned")
            .content
            .get(content_id)
            .cloned()
    }

    pub fn replay(&self, audience: StructuredContentAudience) -> Vec<StructuredContent> {
        self.inner
            .read()
            .expect("structured content lock poisoned")
            .content
            .values()
            .filter(|content| content.audience == audience)
            .cloned()
            .collect()
    }

    pub fn delete(&self, content_id: &str, owner: &str, revision: u64) -> anyhow::Result<bool> {
        let mut state = self
            .inner
            .write()
            .expect("structured content lock poisoned");
        let Some(content) = state.content.get(content_id) else {
            return Ok(false);
        };
        anyhow::ensure!(content.owner == owner, "structured content owner mismatch");
        anyhow::ensure!(
            content.revision == revision,
            "structured content revision conflict"
        );
        state.content.remove(content_id);
        state.deleted.insert(content_id.to_string());
        Ok(true)
    }

    pub fn accept_action(
        &self,
        action: StructuredContentAction,
    ) -> anyhow::Result<StructuredActionReceipt> {
        action.validate()?;
        let mut state = self
            .inner
            .write()
            .expect("structured content lock poisoned");
        if let Some(receipt) = state.consumed_actions.get(&action.idempotency_key) {
            anyhow::ensure!(
                receipt.action_id == action.action_id
                    && receipt.content_id == action.content_id
                    && receipt.content_revision == action.content_revision,
                "structured action idempotency conflict"
            );
            let mut receipt = receipt.clone();
            receipt.replayed = true;
            return Ok(receipt);
        }
        anyhow::ensure!(action.expires_at > Utc::now(), "structured action expired");
        let content = state
            .content
            .get(&action.content_id)
            .ok_or_else(|| anyhow::anyhow!("structured content is unavailable"))?;
        anyhow::ensure!(
            content.owner == action.owner,
            "structured action owner mismatch"
        );
        anyhow::ensure!(
            content.revision == action.content_revision,
            "structured action revision is stale"
        );
        let receipt = StructuredActionReceipt {
            action_id: action.action_id,
            content_id: action.content_id,
            content_revision: action.content_revision,
            replayed: false,
            payload: action.payload,
        };
        state
            .consumed_actions
            .insert(action.idempotency_key, receipt.clone());
        Ok(receipt)
    }
}

fn validate_id(value: &str, label: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value.len() <= 255
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character)),
        "invalid {label}"
    );
    Ok(())
}

fn valid_mime_type(value: &str) -> bool {
    let Some((top, subtype)) = value.split_once('/') else {
        return false;
    };
    !top.is_empty()
        && !subtype.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/.+-".contains(character))
}

fn reject_active_content(value: &Value) -> anyhow::Result<()> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = key.to_ascii_lowercase();
                anyhow::ensure!(
                    !matches!(
                        normalized.as_str(),
                        "html" | "script" | "javascript" | "iframe"
                    ),
                    "structured content cannot embed active content"
                );
                reject_active_content(child)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_active_content(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn content(revision: u64) -> StructuredContent {
        StructuredContent {
            content_id: "briefing-1".into(),
            mime_type: "application/vnd.agentweave.card+json".into(),
            schema_version: "1".into(),
            payload: serde_json::json!({"title": "Daily briefing"}),
            fallback_text: "Daily briefing".into(),
            audience: StructuredContentAudience::User,
            owner: "secretary-agent".into(),
            revision,
        }
    }

    #[test]
    fn publication_requires_monotonic_revision_and_stable_owner() {
        let registry = StructuredContentRegistry::default();
        registry.publish(content(1)).unwrap();
        assert!(registry.publish(content(3)).is_err());
        registry.publish(content(2)).unwrap();
        assert_eq!(registry.get("briefing-1").unwrap().revision, 2);
    }

    #[test]
    fn actions_validate_owner_revision_expiry_and_idempotency() {
        let registry = StructuredContentRegistry::default();
        registry.publish(content(1)).unwrap();
        let action = StructuredContentAction {
            action_id: "open-task".into(),
            content_id: "briefing-1".into(),
            content_revision: 1,
            owner: "secretary-agent".into(),
            idempotency_key: "action-1".into(),
            expires_at: Utc::now() + Duration::minutes(5),
            payload: serde_json::json!({"task_id": "task-1"}),
        };
        assert!(!registry.accept_action(action.clone()).unwrap().replayed);
        assert!(registry.accept_action(action).unwrap().replayed);
    }

    #[test]
    fn active_content_is_rejected_and_text_fallback_is_required() {
        let registry = StructuredContentRegistry::default();
        let mut invalid = content(1);
        invalid.payload = serde_json::json!({"html": "<script>bad()</script>"});
        assert!(registry.publish(invalid).is_err());
        let mut invalid = content(1);
        invalid.fallback_text.clear();
        assert!(registry.publish(invalid).is_err());
    }
}

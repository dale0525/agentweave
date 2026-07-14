use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessagingScope {
    pub app_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub account_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelIdentity {
    pub channel: String,
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelMessage {
    pub id: String,
    pub thread_id: String,
    pub sender: ChannelIdentity,
    pub recipients: Vec<ChannelIdentity>,
    pub text: String,
    pub sent_at: DateTime<Utc>,
    pub provider_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDraft {
    pub id: String,
    pub thread_id: Option<String>,
    pub recipients: Vec<ChannelIdentity>,
    pub text: String,
    pub version: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSendPreview {
    pub preview_id: String,
    pub draft_id: String,
    pub draft_version: u64,
    pub recipients: Vec<ChannelIdentity>,
    pub text_sha256: String,
    pub preview_hash: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeliveryReceipt {
    pub delivery_id: String,
    pub message_id: String,
    pub delivered_at: DateTime<Utc>,
    pub state: String,
}

#[derive(Clone, Default)]
pub struct FakeMessagingConnector {
    state: Arc<Mutex<FakeMessagingState>>,
}

#[derive(Default)]
struct FakeMessagingState {
    messages: BTreeMap<(MessagingScope, String), ChannelMessage>,
    drafts: BTreeMap<(MessagingScope, String), MessageDraft>,
    previews: HashMap<String, (MessagingScope, MessageSendPreview)>,
    receipts: HashMap<(MessagingScope, String), MessageDeliveryReceipt>,
}

impl FakeMessagingConnector {
    pub fn seed_message(&self, scope: MessagingScope, message: ChannelMessage) {
        self.state
            .lock()
            .expect("messaging lock poisoned")
            .messages
            .insert((scope, message.id.clone()), message);
    }

    pub fn list_thread(
        &self,
        scope: &MessagingScope,
        thread_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<ChannelMessage>> {
        anyhow::ensure!(
            (1..=100).contains(&limit),
            "message result limit is invalid"
        );
        let mut values = self
            .state
            .lock()
            .expect("messaging lock poisoned")
            .messages
            .iter()
            .filter(|((message_scope, _), message)| {
                message_scope == scope && message.thread_id == thread_id
            })
            .map(|(_, message)| message.clone())
            .collect::<Vec<_>>();
        values.sort_by(|left, right| {
            left.sent_at
                .cmp(&right.sent_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        values.truncate(limit);
        Ok(values)
    }

    pub fn create_draft(
        &self,
        scope: &MessagingScope,
        thread_id: Option<String>,
        recipients: Vec<ChannelIdentity>,
        text: String,
    ) -> anyhow::Result<MessageDraft> {
        validate_message_content(&recipients, &text)?;
        let draft = MessageDraft {
            id: Uuid::new_v4().to_string(),
            thread_id,
            recipients,
            text,
            version: 1,
        };
        self.state
            .lock()
            .expect("messaging lock poisoned")
            .drafts
            .insert((scope.clone(), draft.id.clone()), draft.clone());
        Ok(draft)
    }

    pub fn update_draft(
        &self,
        scope: &MessagingScope,
        draft_id: &str,
        expected_version: u64,
        recipients: Vec<ChannelIdentity>,
        text: String,
    ) -> anyhow::Result<MessageDraft> {
        validate_message_content(&recipients, &text)?;
        let mut state = self.state.lock().expect("messaging lock poisoned");
        let draft = state
            .drafts
            .get_mut(&(scope.clone(), draft_id.into()))
            .ok_or_else(|| anyhow::anyhow!("message draft not found"))?;
        anyhow::ensure!(
            draft.version == expected_version,
            "message draft version conflict"
        );
        draft.recipients = recipients;
        draft.text = text;
        draft.version += 1;
        Ok(draft.clone())
    }

    pub fn preview_send(
        &self,
        scope: &MessagingScope,
        draft_id: &str,
        expected_version: u64,
        idempotency_key: String,
    ) -> anyhow::Result<MessageSendPreview> {
        anyhow::ensure!(
            !idempotency_key.trim().is_empty(),
            "message idempotency key is required"
        );
        let mut state = self.state.lock().expect("messaging lock poisoned");
        let draft = state
            .drafts
            .get(&(scope.clone(), draft_id.into()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("message draft not found"))?;
        anyhow::ensure!(
            draft.version == expected_version,
            "message draft version conflict"
        );
        let preview_id = Uuid::new_v4().to_string();
        let text_sha256 = hex::encode(Sha256::digest(draft.text.as_bytes()));
        let preview_hash = hex::encode(Sha256::digest(serde_json::to_vec(&(
            scope,
            &draft,
            &idempotency_key,
        ))?));
        let preview = MessageSendPreview {
            preview_id: preview_id.clone(),
            draft_id: draft.id,
            draft_version: draft.version,
            recipients: draft.recipients,
            text_sha256,
            preview_hash,
            idempotency_key,
        };
        state
            .previews
            .insert(preview_id, (scope.clone(), preview.clone()));
        Ok(preview)
    }

    pub fn send_approved(
        &self,
        scope: &MessagingScope,
        preview_id: &str,
        approved_hash: &str,
        approval_id: &str,
    ) -> anyhow::Result<MessageDeliveryReceipt> {
        anyhow::ensure!(
            !approval_id.trim().is_empty(),
            "message approval is required"
        );
        let mut state = self.state.lock().expect("messaging lock poisoned");
        let (preview_scope, preview) = state
            .previews
            .get(preview_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("message preview not found"))?;
        anyhow::ensure!(
            &preview_scope == scope && preview.preview_hash == approved_hash,
            "message approval does not match preview"
        );
        if let Some(receipt) = state
            .receipts
            .get(&(scope.clone(), preview.idempotency_key.clone()))
        {
            return Ok(receipt.clone());
        }
        let draft = state
            .drafts
            .get(&(scope.clone(), preview.draft_id.clone()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("message draft not found"))?;
        anyhow::ensure!(
            draft.version == preview.draft_version,
            "message draft changed after preview"
        );
        let message = ChannelMessage {
            id: Uuid::new_v4().to_string(),
            thread_id: draft
                .thread_id
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            sender: ChannelIdentity {
                channel: "fake".into(),
                id: scope.account_id.clone(),
                display_name: None,
            },
            recipients: draft.recipients,
            text: draft.text,
            sent_at: Utc::now(),
            provider_id: None,
        };
        let receipt = MessageDeliveryReceipt {
            delivery_id: Uuid::new_v4().to_string(),
            message_id: message.id.clone(),
            delivered_at: Utc::now(),
            state: "delivered".into(),
        };
        state
            .messages
            .insert((scope.clone(), message.id.clone()), message);
        state
            .receipts
            .insert((scope.clone(), preview.idempotency_key), receipt.clone());
        Ok(receipt)
    }
}

fn validate_message_content(recipients: &[ChannelIdentity], text: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !recipients.is_empty() && recipients.len() <= 200,
        "message recipients are invalid"
    );
    anyhow::ensure!(
        !text.trim().is_empty() && text.len() <= 1024 * 1024,
        "message text is invalid"
    );
    for recipient in recipients {
        anyhow::ensure!(
            !recipient.channel.trim().is_empty() && !recipient.id.trim().is_empty(),
            "message recipient is unresolved"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> MessagingScope {
        MessagingScope {
            app_id: "com.example.app".into(),
            tenant_id: "local".into(),
            user_id: "user".into(),
            account_id: "primary".into(),
        }
    }

    #[test]
    fn approved_send_is_bound_to_resolved_recipients_and_idempotent() {
        let connector = FakeMessagingConnector::default();
        let recipient = ChannelIdentity {
            channel: "fake".into(),
            id: "contact-1".into(),
            display_name: Some("Alex".into()),
        };
        let draft = connector
            .create_draft(&scope(), None, vec![recipient.clone()], "Hello".into())
            .unwrap();
        let preview = connector
            .preview_send(&scope(), &draft.id, 1, "send-1".into())
            .unwrap();
        assert_eq!(preview.recipients, vec![recipient]);
        let first = connector
            .send_approved(
                &scope(),
                &preview.preview_id,
                &preview.preview_hash,
                "approval-1",
            )
            .unwrap();
        let second = connector
            .send_approved(
                &scope(),
                &preview.preview_id,
                &preview.preview_hash,
                "approval-1",
            )
            .unwrap();
        assert_eq!(first.delivery_id, second.delivery_id);
    }
}

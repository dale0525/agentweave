use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContactScope {
    pub app_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub account_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContactIdentity {
    pub kind: String,
    pub value: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContactRecord {
    pub id: String,
    pub display_name: String,
    pub identities: Vec<ContactIdentity>,
    pub organization: Option<String>,
    pub relationship: Option<String>,
    pub version: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContactMutationPreview {
    pub preview_id: String,
    pub contact_id: String,
    pub expected_version: u64,
    pub replacement: ContactRecord,
    pub preview_hash: String,
}

#[derive(Clone, Default)]
pub struct FakeContactsConnector {
    state: Arc<Mutex<BTreeMap<(ContactScope, String), ContactRecord>>>,
}

impl FakeContactsConnector {
    pub fn seed(&self, scope: ContactScope, contact: ContactRecord) {
        self.state
            .lock()
            .expect("contacts lock poisoned")
            .insert((scope, contact.id.clone()), contact);
    }

    pub fn resolve(
        &self,
        scope: &ContactScope,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<ContactRecord>> {
        anyhow::ensure!(
            !query.trim().is_empty() && query.len() <= 1024,
            "contact query is invalid"
        );
        anyhow::ensure!((1..=50).contains(&limit), "contact result limit is invalid");
        let query = query.to_lowercase();
        let mut contacts = self
            .state
            .lock()
            .expect("contacts lock poisoned")
            .iter()
            .filter(|((contact_scope, _), contact)| {
                contact_scope == scope
                    && (contact.display_name.to_lowercase().contains(&query)
                        || contact
                            .identities
                            .iter()
                            .any(|identity| identity.value.to_lowercase().contains(&query)))
            })
            .map(|(_, contact)| contact.clone())
            .collect::<Vec<_>>();
        contacts.sort_by(|left, right| {
            left.display_name
                .cmp(&right.display_name)
                .then_with(|| left.id.cmp(&right.id))
        });
        contacts.truncate(limit);
        Ok(contacts)
    }

    pub fn preview_update(
        &self,
        scope: &ContactScope,
        contact_id: &str,
        expected_version: u64,
        mut replacement: ContactRecord,
    ) -> anyhow::Result<ContactMutationPreview> {
        validate_contact(&replacement)?;
        let state = self.state.lock().expect("contacts lock poisoned");
        let current = state
            .get(&(scope.clone(), contact_id.into()))
            .ok_or_else(|| anyhow::anyhow!("contact not found"))?;
        anyhow::ensure!(
            current.version == expected_version,
            "contact version conflict"
        );
        replacement.id = contact_id.into();
        replacement.version = expected_version + 1;
        let preview_id = Uuid::new_v4().to_string();
        let preview_hash = hex::encode(Sha256::digest(serde_json::to_vec(&(scope, &replacement))?));
        Ok(ContactMutationPreview {
            preview_id,
            contact_id: contact_id.into(),
            expected_version,
            replacement,
            preview_hash,
        })
    }

    pub fn apply_update(
        &self,
        scope: &ContactScope,
        preview: ContactMutationPreview,
        approved_hash: &str,
    ) -> anyhow::Result<ContactRecord> {
        anyhow::ensure!(
            preview.preview_hash == approved_hash,
            "contact approval does not match preview"
        );
        let mut state = self.state.lock().expect("contacts lock poisoned");
        let current = state
            .get(&(scope.clone(), preview.contact_id.clone()))
            .ok_or_else(|| anyhow::anyhow!("contact not found"))?;
        anyhow::ensure!(
            current.version == preview.expected_version,
            "contact version conflict"
        );
        let mut replacement = preview.replacement;
        replacement.updated_at = Utc::now();
        state.insert((scope.clone(), replacement.id.clone()), replacement.clone());
        Ok(replacement)
    }
}

fn validate_contact(contact: &ContactRecord) -> anyhow::Result<()> {
    anyhow::ensure!(
        !contact.display_name.trim().is_empty(),
        "contact display name is required"
    );
    anyhow::ensure!(
        !contact.identities.is_empty(),
        "contact identity is required"
    );
    anyhow::ensure!(
        contact.identities.len() <= 100,
        "contact has too many identities"
    );
    for identity in &contact.identities {
        anyhow::ensure!(
            !identity.kind.trim().is_empty() && !identity.value.trim().is_empty(),
            "contact identity is invalid"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> ContactScope {
        ContactScope {
            app_id: "com.example.app".into(),
            tenant_id: "local".into(),
            user_id: "user".into(),
            account_id: "primary".into(),
        }
    }
    fn contact(id: &str, name: &str, email: &str) -> ContactRecord {
        ContactRecord {
            id: id.into(),
            display_name: name.into(),
            identities: vec![ContactIdentity {
                kind: "email".into(),
                value: email.into(),
                label: None,
            }],
            organization: None,
            relationship: None,
            version: 1,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn ambiguous_resolution_stays_explicit_and_updates_require_exact_preview() {
        let connector = FakeContactsConnector::default();
        connector.seed(
            scope(),
            contact("one", "Alex Chen", "alex.one@example.test"),
        );
        connector.seed(
            scope(),
            contact("two", "Alex Chen", "alex.two@example.test"),
        );
        assert_eq!(connector.resolve(&scope(), "Alex", 10).unwrap().len(), 2);
        let preview = connector
            .preview_update(
                &scope(),
                "one",
                1,
                contact("ignored", "Alex Chen", "new@example.test"),
            )
            .unwrap();
        assert!(
            connector
                .apply_update(&scope(), preview.clone(), "wrong")
                .is_err()
        );
        assert_eq!(
            connector
                .apply_update(&scope(), preview.clone(), &preview.preview_hash)
                .unwrap()
                .version,
            2
        );
    }
}

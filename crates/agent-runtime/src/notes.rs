use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoteScope {
    pub app_id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub provider_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoteRecord {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub owner_user_id: String,
    pub sharing: String,
    pub source_ids: Vec<String>,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Default)]
pub struct FakeNotesProvider {
    notes: Arc<Mutex<BTreeMap<(NoteScope, String), NoteRecord>>>,
}

impl FakeNotesProvider {
    pub fn create(
        &self,
        scope: &NoteScope,
        title: &str,
        body: &str,
        tags: Vec<String>,
        source_ids: Vec<String>,
    ) -> anyhow::Result<NoteRecord> {
        validate_note(title, body, &tags)?;
        let now = Utc::now();
        let note = NoteRecord {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            body: body.into(),
            tags,
            owner_user_id: scope.user_id.clone(),
            sharing: "private".into(),
            source_ids,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        self.notes
            .lock()
            .expect("notes lock poisoned")
            .insert((scope.clone(), note.id.clone()), note.clone());
        Ok(note)
    }

    pub fn search(
        &self,
        scope: &NoteScope,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<NoteRecord>> {
        anyhow::ensure!((1..=100).contains(&limit), "note result limit is invalid");
        let query = query.to_lowercase();
        let mut notes = self
            .notes
            .lock()
            .expect("notes lock poisoned")
            .iter()
            .filter(|((note_scope, _), note)| {
                note_scope == scope
                    && (query.is_empty()
                        || note.title.to_lowercase().contains(&query)
                        || note.body.to_lowercase().contains(&query))
            })
            .map(|(_, note)| note.clone())
            .collect::<Vec<_>>();
        notes.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        notes.truncate(limit);
        Ok(notes)
    }

    pub fn update(
        &self,
        scope: &NoteScope,
        id: &str,
        expected_version: u64,
        title: &str,
        body: &str,
        tags: Vec<String>,
    ) -> anyhow::Result<NoteRecord> {
        validate_note(title, body, &tags)?;
        let mut state = self.notes.lock().expect("notes lock poisoned");
        let note = state
            .get_mut(&(scope.clone(), id.into()))
            .ok_or_else(|| anyhow::anyhow!("note not found"))?;
        anyhow::ensure!(note.version == expected_version, "note version conflict");
        note.title = title.into();
        note.body = body.into();
        note.tags = tags;
        note.version += 1;
        note.updated_at = Utc::now();
        Ok(note.clone())
    }

    pub fn delete(
        &self,
        scope: &NoteScope,
        id: &str,
        expected_version: u64,
    ) -> anyhow::Result<bool> {
        let mut state = self.notes.lock().expect("notes lock poisoned");
        let key = (scope.clone(), id.into());
        anyhow::ensure!(
            state
                .get(&key)
                .is_some_and(|note| note.version == expected_version),
            "note not found or version conflict"
        );
        state.remove(&key);
        Ok(true)
    }
}

fn validate_note(title: &str, body: &str, tags: &[String]) -> anyhow::Result<()> {
    anyhow::ensure!(
        !title.trim().is_empty() && title.len() <= 1024,
        "note title is invalid"
    );
    anyhow::ensure!(body.len() <= 2 * 1024 * 1024, "note body is too large");
    anyhow::ensure!(tags.len() <= 100, "note has too many tags");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(user: &str) -> NoteScope {
        NoteScope {
            app_id: "com.example.app".into(),
            tenant_id: "local".into(),
            user_id: user.into(),
            provider_id: "local".into(),
        }
    }

    #[test]
    fn notes_remain_user_owned_scoped_and_separate_from_memory() {
        let provider = FakeNotesProvider::default();
        let note = provider
            .create(
                &scope("user"),
                "Project",
                "Explicit user content",
                vec!["work".into()],
                vec!["document-1".into()],
            )
            .unwrap();
        assert_eq!(note.owner_user_id, "user");
        assert!(
            provider
                .search(&scope("other"), "Project", 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            provider
                .update(&scope("user"), &note.id, 1, "Project", "Updated", vec![])
                .unwrap()
                .version,
            2
        );
    }
}

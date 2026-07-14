use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    PlainText,
    Markdown,
    Pdf,
    Docx,
    Xlsx,
    Pptx,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentArtifact {
    pub artifact_id: String,
    pub source_artifact_ids: Vec<String>,
    pub file_name: String,
    pub format: DocumentFormat,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
    pub revision: u64,
    pub verified: bool,
    pub verification_notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredDocumentArtifact {
    pub metadata: DocumentArtifact,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Default)]
pub struct FakeDocumentArtifactHost {
    artifacts: Arc<Mutex<BTreeMap<String, StoredDocumentArtifact>>>,
}

impl FakeDocumentArtifactHost {
    pub fn create(
        &self,
        file_name: &str,
        format: DocumentFormat,
        bytes: Vec<u8>,
        source_artifact_ids: Vec<String>,
    ) -> anyhow::Result<DocumentArtifact> {
        anyhow::ensure!(
            !file_name.trim().is_empty() && file_name.len() <= 512,
            "document file name is invalid"
        );
        anyhow::ensure!(
            bytes.len() <= 100 * 1024 * 1024,
            "document artifact is too large"
        );
        let artifact = DocumentArtifact {
            artifact_id: Uuid::new_v4().to_string(),
            source_artifact_ids,
            file_name: file_name.into(),
            format,
            content_sha256: hex::encode(Sha256::digest(&bytes)),
            size_bytes: bytes.len() as u64,
            created_at: Utc::now(),
            revision: 1,
            verified: false,
            verification_notes: Vec::new(),
        };
        self.artifacts
            .lock()
            .expect("document lock poisoned")
            .insert(
                artifact.artifact_id.clone(),
                StoredDocumentArtifact {
                    metadata: artifact.clone(),
                    bytes,
                },
            );
        Ok(artifact)
    }

    pub fn inspect(&self, artifact_id: &str, max_bytes: usize) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            (1..=16 * 1024 * 1024).contains(&max_bytes),
            "document read limit is invalid"
        );
        let state = self.artifacts.lock().expect("document lock poisoned");
        let artifact = state
            .get(artifact_id)
            .ok_or_else(|| anyhow::anyhow!("document artifact not found"))?;
        Ok(artifact.bytes[..artifact.bytes.len().min(max_bytes)].to_vec())
    }

    pub fn derive(
        &self,
        source_id: &str,
        file_name: &str,
        format: DocumentFormat,
        bytes: Vec<u8>,
    ) -> anyhow::Result<DocumentArtifact> {
        anyhow::ensure!(
            self.artifacts
                .lock()
                .expect("document lock poisoned")
                .contains_key(source_id),
            "source document artifact not found"
        );
        self.create(file_name, format, bytes, vec![source_id.into()])
    }

    pub fn verify_render(
        &self,
        artifact_id: &str,
        notes: Vec<String>,
    ) -> anyhow::Result<DocumentArtifact> {
        let mut state = self.artifacts.lock().expect("document lock poisoned");
        let artifact = state
            .get_mut(artifact_id)
            .ok_or_else(|| anyhow::anyhow!("document artifact not found"))?;
        anyhow::ensure!(
            matches!(
                artifact.metadata.format,
                DocumentFormat::Pdf
                    | DocumentFormat::Docx
                    | DocumentFormat::Xlsx
                    | DocumentFormat::Pptx
            ),
            "format does not require visual verification"
        );
        artifact.metadata.verified = notes.is_empty();
        artifact.metadata.verification_notes = notes;
        artifact.metadata.revision += 1;
        Ok(artifact.metadata.clone())
    }

    pub fn get(&self, artifact_id: &str) -> Option<DocumentArtifact> {
        self.artifacts
            .lock()
            .expect("document lock poisoned")
            .get(artifact_id)
            .map(|value| value.metadata.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_documents_preserve_provenance_and_require_verification() {
        let host = FakeDocumentArtifactHost::default();
        let source = host
            .create(
                "notes.md",
                DocumentFormat::Markdown,
                b"source".to_vec(),
                vec![],
            )
            .unwrap();
        let pdf = host
            .derive(
                &source.artifact_id,
                "report.pdf",
                DocumentFormat::Pdf,
                b"pdf".to_vec(),
            )
            .unwrap();
        assert_eq!(pdf.source_artifact_ids, vec![source.artifact_id]);
        assert!(!pdf.verified);
        assert!(
            !host
                .verify_render(&pdf.artifact_id, vec!["footer clipped".into()])
                .unwrap()
                .verified
        );
        assert!(
            host.verify_render(&pdf.artifact_id, vec![])
                .unwrap()
                .verified
        );
    }
}

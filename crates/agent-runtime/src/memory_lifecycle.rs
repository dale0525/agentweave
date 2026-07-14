use crate::memory::MemoryDraft;
use async_trait::async_trait;
use serde_json::Value;

pub struct ExplicitMemoryCandidateExtractor;

#[async_trait]
impl MemoryCandidateExtractor for ExplicitMemoryCandidateExtractor {
    async fn extract_candidates(
        &self,
        transcript: &MemoryTurnTranscript,
    ) -> anyhow::Result<Vec<MemoryDraft>> {
        let Some((kind, text)) = explicit_candidate(&transcript.user_text) else {
            return Ok(Vec::new());
        };
        if looks_sensitive(text) {
            return Ok(Vec::new());
        }
        Ok(vec![MemoryDraft {
            kind: crate::memory::MemoryKind::parse(kind)?,
            value: crate::memory::MemoryValue::new(text)?,
            evidence: vec![crate::memory::MemoryEvidence {
                source: crate::memory::MemoryEvidenceSource::UserStatement,
                source_id: Some(transcript.session_id.clone()),
                excerpt: Some(transcript.user_text.chars().take(1024).collect()),
                observed_at: chrono::Utc::now(),
            }],
            confidence: crate::memory::MemoryConfidence::from_basis_points(10_000)?,
            sensitivity: crate::memory::MemorySensitivity::Personal,
            retention: crate::memory::MemoryRetention::Persistent,
            conflict_key: None,
            supersedes: None,
        }])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemoryTurnTranscript {
    pub session_id: String,
    pub user_text: String,
    pub assistant_text: String,
    pub tool_results: Vec<Value>,
}

#[async_trait]
pub trait MemoryCandidateExtractor: Send + Sync {
    async fn extract_candidates(
        &self,
        transcript: &MemoryTurnTranscript,
    ) -> anyhow::Result<Vec<MemoryDraft>>;
}

fn explicit_candidate(value: &str) -> Option<(&'static str, &str)> {
    let value = value.trim();
    for prefix in [
        "请记住：",
        "请记住:",
        "记住：",
        "记住:",
        "Remember that ",
        "Remember: ",
    ] {
        if let Some(text) = value.strip_prefix(prefix).map(str::trim)
            && !text.is_empty()
        {
            return Some((crate::memory::MemoryKind::USER_FACT, text));
        }
    }
    if (value.starts_with("以后默认") || value.starts_with("今后默认"))
        && value.chars().count() <= 1024
    {
        return Some((crate::memory::MemoryKind::PREFERENCE, value));
    }
    None
}

fn looks_sensitive(value: &str) -> bool {
    let folded = value.to_lowercase();
    [
        "password",
        "api key",
        "access token",
        "refresh token",
        "密码",
        "密钥",
        "令牌",
    ]
    .iter()
    .any(|marker| folded.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_preferences_become_proposals_but_secrets_do_not() {
        let extractor = ExplicitMemoryCandidateExtractor;
        let candidates = extractor
            .extract_candidates(&MemoryTurnTranscript {
                session_id: "session".into(),
                user_text: "以后默认把会议安排在下午".into(),
                assistant_text: String::new(),
                tool_results: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].kind.as_str(),
            crate::memory::MemoryKind::PREFERENCE
        );

        let secret = extractor
            .extract_candidates(&MemoryTurnTranscript {
                session_id: "session".into(),
                user_text: "记住：我的 API key 是 secret".into(),
                assistant_text: String::new(),
                tool_results: Vec::new(),
            })
            .await
            .unwrap();
        assert!(secret.is_empty());
    }
}

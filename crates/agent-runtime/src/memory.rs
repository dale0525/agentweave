use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use uuid::Uuid;

pub const MEMORY_CONTRACT_SCHEMA_VERSION: u32 = 1;
const MAX_SCOPE_COMPONENT_LENGTH: usize = 128;
const MAX_KIND_LENGTH: usize = 128;
const MAX_VALUE_TEXT_LENGTH: usize = 32 * 1024;
const MAX_ATTRIBUTE_COUNT: usize = 64;
const MAX_ATTRIBUTE_LENGTH: usize = 1024;
const MAX_EVIDENCE_COUNT: usize = 32;
const MAX_EVIDENCE_TEXT_LENGTH: usize = 4096;
const MAX_CONFLICT_KEY_LENGTH: usize = 256;
const MAX_RECALL_LIMIT: usize = 100;

pub type MemoryResult<T> = Result<T, MemoryError>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemoryError {
    #[error("invalid memory input: {0}")]
    InvalidInput(String),
    #[error("memory record not found: {0}")]
    NotFound(String),
    #[error("memory version conflict for {id}: expected {expected}, actual {actual}")]
    VersionConflict {
        id: String,
        expected: u64,
        actual: u64,
    },
    #[error("invalid memory state for {id}: expected {expected}, actual {actual}")]
    InvalidState {
        id: String,
        expected: &'static str,
        actual: &'static str,
    },
    #[error("memory provider unavailable during {0}")]
    ProviderUnavailable(&'static str),
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct MemoryId(String);

impl MemoryId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn parse(value: &str) -> MemoryResult<Self> {
        let parsed = Uuid::parse_str(value)
            .map_err(|_| MemoryError::InvalidInput("memory id must be a UUID".into()))?;
        Ok(Self(parsed.hyphenated().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MemoryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("MemoryId").field(&self.0).finish()
    }
}

impl<'de> Deserialize<'de> for MemoryId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryScope {
    pub app_id: String,
    pub tenant_id: String,
    pub user_id: String,
}

impl MemoryScope {
    pub fn new(
        app_id: impl Into<String>,
        tenant_id: impl Into<String>,
        user_id: impl Into<String>,
    ) -> MemoryResult<Self> {
        let scope = Self {
            app_id: app_id.into(),
            tenant_id: tenant_id.into(),
            user_id: user_id.into(),
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> MemoryResult<()> {
        validate_identifier(&self.app_id, "scope.appId", MAX_SCOPE_COMPONENT_LENGTH)?;
        validate_identifier(
            &self.tenant_id,
            "scope.tenantId",
            MAX_SCOPE_COMPONENT_LENGTH,
        )?;
        validate_identifier(&self.user_id, "scope.userId", MAX_SCOPE_COMPONENT_LENGTH)
    }
}

impl fmt::Debug for MemoryScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryScope")
            .field("app_id", &self.app_id)
            .field("tenant_id", &"[redacted]")
            .field("user_id", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct MemoryKind(String);

impl MemoryKind {
    pub const USER_FACT: &'static str = "user.fact";
    pub const PREFERENCE: &'static str = "user.preference";
    pub const RELATIONSHIP: &'static str = "user.relationship";
    pub const PROJECT: &'static str = "work.project";
    pub const COMMITMENT: &'static str = "work.commitment";
    pub const PROCEDURE: &'static str = "workflow.procedure";
    pub const INTERACTION_STYLE: &'static str = "user.interaction_style";

    pub fn parse(value: &str) -> MemoryResult<Self> {
        let valid = !value.is_empty()
            && value.len() <= MAX_KIND_LENGTH
            && value.split('.').count() >= 2
            && value.split('.').all(|segment| {
                !segment.is_empty()
                    && !segment.starts_with(['-', '_'])
                    && !segment.ends_with(['-', '_'])
                    && segment.chars().all(|character| {
                        character.is_ascii_lowercase()
                            || character.is_ascii_digit()
                            || matches!(character, '-' | '_')
                    })
            });
        if !valid {
            return Err(MemoryError::InvalidInput(format!(
                "invalid memory kind: {value}"
            )));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MemoryKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("MemoryKind").field(&self.0).finish()
    }
}

impl<'de> Deserialize<'de> for MemoryKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryValue {
    pub text: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

impl MemoryValue {
    pub fn new(text: impl Into<String>) -> MemoryResult<Self> {
        let value = Self {
            text: text.into(),
            attributes: BTreeMap::new(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn redacted() -> Self {
        Self {
            text: String::new(),
            attributes: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> MemoryResult<()> {
        validate_text(&self.text, "value.text", MAX_VALUE_TEXT_LENGTH, true)?;
        if self.attributes.len() > MAX_ATTRIBUTE_COUNT {
            return Err(MemoryError::InvalidInput(format!(
                "value.attributes exceeds {MAX_ATTRIBUTE_COUNT} entries"
            )));
        }
        for (key, value) in &self.attributes {
            validate_identifier(key, "value attribute key", MAX_KIND_LENGTH)?;
            validate_text(value, "value attribute value", MAX_ATTRIBUTE_LENGTH, false)?;
        }
        Ok(())
    }
}

impl fmt::Debug for MemoryValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryValue")
            .field("text", &"[redacted]")
            .field("text_chars", &self.text.chars().count())
            .field("attribute_count", &self.attributes.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryConfidence(u16);

impl MemoryConfidence {
    pub const MAX_BASIS_POINTS: u16 = 10_000;

    pub fn from_basis_points(value: u16) -> MemoryResult<Self> {
        if value > Self::MAX_BASIS_POINTS {
            return Err(MemoryError::InvalidInput(
                "confidence must be between 0 and 10000 basis points".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn basis_points(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySensitivity {
    Public,
    Internal,
    Personal,
    Sensitive,
    Restricted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryRetention {
    Persistent,
    ExpiresAt { expires_at: DateTime<Utc> },
    Session { session_id: String },
}

impl MemoryRetention {
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::ExpiresAt { expires_at } => Some(*expires_at),
            Self::Persistent | Self::Session { .. } => None,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Session { session_id } => Some(session_id),
            Self::Persistent | Self::ExpiresAt { .. } => None,
        }
    }

    fn validate(&self) -> MemoryResult<()> {
        if let Self::Session { session_id } = self {
            validate_identifier(
                session_id,
                "retention.sessionId",
                MAX_SCOPE_COMPONENT_LENGTH,
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidenceSource {
    ExplicitUserAction,
    UserStatement,
    ToolObservation,
    SessionSummary,
    CompactionSummary,
    Import,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryEvidence {
    pub source: MemoryEvidenceSource,
    pub source_id: Option<String>,
    pub excerpt: Option<String>,
    pub observed_at: DateTime<Utc>,
}

impl MemoryEvidence {
    pub fn validate(&self) -> MemoryResult<()> {
        if let Some(source_id) = &self.source_id {
            validate_identifier(source_id, "evidence.sourceId", MAX_CONFLICT_KEY_LENGTH)?;
        }
        if let Some(excerpt) = &self.excerpt {
            validate_text(excerpt, "evidence.excerpt", MAX_EVIDENCE_TEXT_LENGTH, false)?;
        }
        Ok(())
    }
}

impl fmt::Debug for MemoryEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryEvidence")
            .field("source", &self.source)
            .field("source_id", &self.source_id.as_ref().map(|_| "[redacted]"))
            .field("excerpt", &self.excerpt.as_ref().map(|_| "[redacted]"))
            .field("observed_at", &self.observed_at)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    Proposed,
    Committed,
    Tombstoned,
}

impl MemoryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Committed => "committed",
            Self::Tombstoned => "tombstoned",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTombstoneReason {
    UserRequest,
    Expired,
    SessionEnded,
    ScopeDeleted,
    Replaced,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryTombstone {
    pub reason: MemoryTombstoneReason,
    pub at: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryDraft {
    pub kind: MemoryKind,
    pub value: MemoryValue,
    pub evidence: Vec<MemoryEvidence>,
    pub confidence: MemoryConfidence,
    pub sensitivity: MemorySensitivity,
    pub retention: MemoryRetention,
    pub conflict_key: Option<String>,
    pub supersedes: Option<MemoryId>,
}

impl MemoryDraft {
    pub fn validate(&self) -> MemoryResult<()> {
        self.value.validate()?;
        validate_evidence(&self.evidence)?;
        self.retention.validate()?;
        validate_conflict_key(self.conflict_key.as_deref())
    }
}

impl fmt::Debug for MemoryDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryDraft")
            .field("kind", &self.kind)
            .field("value", &self.value)
            .field("evidence_count", &self.evidence.len())
            .field("confidence", &self.confidence)
            .field("sensitivity", &self.sensitivity)
            .field("retention", &self.retention)
            .field(
                "conflict_key",
                &self.conflict_key.as_ref().map(|_| "[redacted]"),
            )
            .field("supersedes", &self.supersedes)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryRecord {
    pub schema_version: u32,
    pub id: MemoryId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub value: MemoryValue,
    pub evidence: Vec<MemoryEvidence>,
    pub confidence: MemoryConfidence,
    pub sensitivity: MemorySensitivity,
    pub retention: MemoryRetention,
    pub state: MemoryState,
    pub version: u64,
    pub conflict_key: Option<String>,
    pub supersedes: Option<MemoryId>,
    pub superseded_by: Option<MemoryId>,
    pub tombstone: Option<MemoryTombstone>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MemoryRecord {
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        self.retention
            .expires_at()
            .is_some_and(|expires_at| expires_at <= now)
    }

    pub fn validate(&self) -> MemoryResult<()> {
        if self.schema_version != MEMORY_CONTRACT_SCHEMA_VERSION {
            return Err(MemoryError::InvalidInput(format!(
                "unsupported memory schema version {}",
                self.schema_version
            )));
        }
        self.scope.validate()?;
        if self.version == 0 {
            return Err(MemoryError::InvalidInput(
                "memory version must be positive".into(),
            ));
        }
        validate_conflict_key(self.conflict_key.as_deref())?;
        match self.state {
            MemoryState::Tombstoned => {
                if self.tombstone.is_none()
                    || !self.value.text.is_empty()
                    || !self.evidence.is_empty()
                {
                    return Err(MemoryError::InvalidInput(
                        "tombstoned memory must be scrubbed and include tombstone metadata".into(),
                    ));
                }
            }
            MemoryState::Proposed | MemoryState::Committed => {
                if self.tombstone.is_some() {
                    return Err(MemoryError::InvalidInput(
                        "live memory cannot include tombstone metadata".into(),
                    ));
                }
                self.value.validate()?;
                validate_evidence(&self.evidence)?;
                self.retention.validate()?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for MemoryRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryRecord")
            .field("id", &self.id)
            .field("scope", &self.scope)
            .field("kind", &self.kind)
            .field("value", &self.value)
            .field("evidence_count", &self.evidence.len())
            .field("confidence", &self.confidence)
            .field("sensitivity", &self.sensitivity)
            .field("retention", &self.retention)
            .field("state", &self.state)
            .field("version", &self.version)
            .field("supersedes", &self.supersedes)
            .field("superseded_by", &self.superseded_by)
            .field("tombstone", &self.tombstone)
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryUpdate {
    pub value: Option<MemoryValue>,
    pub evidence: Option<Vec<MemoryEvidence>>,
    pub confidence: Option<MemoryConfidence>,
    pub sensitivity: Option<MemorySensitivity>,
    pub retention: Option<MemoryRetention>,
    pub conflict_key: Option<Option<String>>,
}

impl MemoryUpdate {
    pub fn validate(&self) -> MemoryResult<()> {
        if let Some(value) = &self.value {
            value.validate()?;
        }
        if let Some(evidence) = &self.evidence {
            validate_evidence(evidence)?;
        }
        if let Some(retention) = &self.retention {
            retention.validate()?;
        }
        if let Some(conflict_key) = &self.conflict_key {
            validate_conflict_key(conflict_key.as_deref())?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExplicitMemoryMutation {
    Propose {
        draft: MemoryDraft,
    },
    Confirm {
        id: MemoryId,
        expected_version: u64,
    },
    Update {
        id: MemoryId,
        expected_version: u64,
        changes: MemoryUpdate,
    },
    Forget {
        id: MemoryId,
        expected_version: u64,
        reason: MemoryTombstoneReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryMutationRequest {
    pub scope: MemoryScope,
    pub mutation: ExplicitMemoryMutation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMutationAction {
    Proposed,
    Confirmed,
    Updated,
    Forgotten,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryMutationResult {
    pub action: MemoryMutationAction,
    pub record: MemoryRecord,
    pub conflicts: Vec<MemoryId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryRecallRequest {
    pub scope: MemoryScope,
    pub query: String,
    #[serde(default)]
    pub kinds: BTreeSet<MemoryKind>,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryGetRequest {
    pub scope: MemoryScope,
    pub id: MemoryId,
    pub include_tombstone: bool,
}

impl MemoryRecallRequest {
    pub fn validate(&self) -> MemoryResult<()> {
        self.scope.validate()?;
        validate_text(&self.query, "recall.query", MAX_VALUE_TEXT_LENGTH, false)?;
        if self.limit == 0 || self.limit > MAX_RECALL_LIMIT {
            return Err(MemoryError::InvalidInput(format!(
                "recall.limit must be between 1 and {MAX_RECALL_LIMIT}"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryCandidateBatch {
    pub scope: MemoryScope,
    pub session_id: String,
    pub candidates: Vec<MemoryDraft>,
}

impl MemoryCandidateBatch {
    pub fn validate(&self) -> MemoryResult<()> {
        self.scope.validate()?;
        validate_identifier(
            &self.session_id,
            "candidateBatch.sessionId",
            MAX_SCOPE_COMPONENT_LENGTH,
        )?;
        for candidate in &self.candidates {
            candidate.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryExportRequest {
    pub scope: MemoryScope,
    pub include_proposals: bool,
    pub include_tombstones: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryExport {
    pub schema_version: u32,
    pub scope: MemoryScope,
    pub exported_at: DateTime<Utc>,
    pub records: Vec<MemoryRecord>,
}

impl fmt::Debug for MemoryExport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryExport")
            .field("schema_version", &self.schema_version)
            .field("scope", &self.scope)
            .field("exported_at", &self.exported_at)
            .field("record_count", &self.records.len())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryDeleteResult {
    pub deleted_records: u64,
    pub deleted_index_entries: u64,
}

#[async_trait]
pub trait MemoryProvider: Send + Sync {
    async fn initialize(&self) -> MemoryResult<()>;

    async fn get(&self, request: MemoryGetRequest) -> MemoryResult<Option<MemoryRecord>>;

    async fn pre_turn_recall(
        &self,
        request: MemoryRecallRequest,
    ) -> MemoryResult<Vec<MemoryRecord>>;

    async fn post_turn_candidates(
        &self,
        batch: MemoryCandidateBatch,
    ) -> MemoryResult<Vec<MemoryRecord>>;

    async fn on_session_end(&self, batch: MemoryCandidateBatch) -> MemoryResult<Vec<MemoryRecord>>;

    async fn on_compaction(&self, batch: MemoryCandidateBatch) -> MemoryResult<Vec<MemoryRecord>>;

    async fn mutate(&self, request: MemoryMutationRequest) -> MemoryResult<MemoryMutationResult>;

    async fn export(&self, request: MemoryExportRequest) -> MemoryResult<MemoryExport>;

    async fn delete_scope(&self, scope: &MemoryScope) -> MemoryResult<MemoryDeleteResult>;

    async fn shutdown(&self) -> MemoryResult<()>;
}

fn validate_evidence(evidence: &[MemoryEvidence]) -> MemoryResult<()> {
    if evidence.is_empty() || evidence.len() > MAX_EVIDENCE_COUNT {
        return Err(MemoryError::InvalidInput(format!(
            "evidence count must be between 1 and {MAX_EVIDENCE_COUNT}"
        )));
    }
    for item in evidence {
        item.validate()?;
    }
    Ok(())
}

fn validate_conflict_key(value: Option<&str>) -> MemoryResult<()> {
    if let Some(value) = value {
        validate_identifier(value, "conflictKey", MAX_CONFLICT_KEY_LENGTH)?;
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str, maximum: usize) -> MemoryResult<()> {
    if value.is_empty()
        || value != value.trim()
        || value.chars().count() > maximum
        || value.chars().any(char::is_control)
    {
        return Err(MemoryError::InvalidInput(format!("invalid {label}")));
    }
    Ok(())
}

fn validate_text(value: &str, label: &str, maximum: usize, required: bool) -> MemoryResult<()> {
    if (required && value.trim().is_empty())
        || value.chars().count() > maximum
        || value.chars().any(|character| character == '\0')
    {
        return Err(MemoryError::InvalidInput(format!("invalid {label}")));
    }
    Ok(())
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;

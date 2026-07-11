use crate::skill_state::{
    SkillRevisionExpectation, SkillRevisionMetadata, SkillRevisionPromotion, SkillRevisionRecord,
    SkillStateStore,
};
use crate::skill_state_rows::{
    REVISION_COLUMNS, revision_from_row, validate_storage_path, validate_uuid_v4,
};
use sqlx::{Executor, Sqlite};

impl SkillStateStore {
    pub async fn refresh_quarantined_revision_validation_cas(
        &self,
        revision_id: &str,
        expected: SkillRevisionExpectation,
        validation_json: serde_json::Value,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_uuid_v4("revision_id", revision_id)?;
        if expected.status != crate::skill_state::SkillRevisionStatus::Quarantined {
            anyhow::bail!("quarantine validation CAS requires quarantined lifecycle");
        }
        let expected_descriptor = serde_json::to_string(&expected.descriptor_json)?;
        let expected_validation = serde_json::to_string(&expected.validation_json)?;
        let validation = serde_json::to_string(&validation_json)?;
        let query = format!(
            r#"UPDATE skill_revisions SET validation_json = ?
               WHERE revision_id = ? AND lifecycle_status = ? AND version = ?
                 AND content_hash = ? AND storage_path = ? AND descriptor_json = ?
                 AND validation_json = ? RETURNING {REVISION_COLUMNS}"#
        );
        let row = sqlx::query(&query)
            .bind(validation)
            .bind(revision_id)
            .bind(expected.status.as_str())
            .bind(&expected.version)
            .bind(&expected.content_hash)
            .bind(&expected.storage_path)
            .bind(expected_descriptor)
            .bind(expected_validation)
            .fetch_optional(self.pool())
            .await?;
        match row {
            Some(row) => revision_from_row(&row),
            None => revision_cas_rejection(self.pool(), revision_id).await,
        }
    }

    pub async fn release_quarantined_revision_cas(
        &self,
        revision_id: &str,
        expected: SkillRevisionExpectation,
        staging_storage_path: &str,
        validation_json: serde_json::Value,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_uuid_v4("revision_id", revision_id)?;
        validate_storage_path(staging_storage_path)?;
        if expected.status != crate::skill_state::SkillRevisionStatus::Quarantined {
            anyhow::bail!("quarantine release CAS requires quarantined lifecycle");
        }
        let expected_descriptor = serde_json::to_string(&expected.descriptor_json)?;
        let expected_validation = serde_json::to_string(&expected.validation_json)?;
        let validation = serde_json::to_string(&validation_json)?;
        let query = format!(
            r#"UPDATE skill_revisions SET storage_path = ?, validation_json = ?, lifecycle_status = 'staging'
               WHERE revision_id = ? AND lifecycle_status = ? AND version = ?
                 AND content_hash = ? AND storage_path = ? AND descriptor_json = ?
                 AND validation_json = ? RETURNING {REVISION_COLUMNS}"#
        );
        let row = sqlx::query(&query)
            .bind(staging_storage_path)
            .bind(validation)
            .bind(revision_id)
            .bind(expected.status.as_str())
            .bind(&expected.version)
            .bind(&expected.content_hash)
            .bind(&expected.storage_path)
            .bind(expected_descriptor)
            .bind(expected_validation)
            .fetch_optional(self.pool())
            .await?;
        match row {
            Some(row) => revision_from_row(&row),
            None => revision_cas_rejection(self.pool(), revision_id).await,
        }
    }

    pub async fn refresh_staging_revision_metadata_cas(
        &self,
        revision_id: &str,
        expected: SkillRevisionExpectation,
        metadata: SkillRevisionMetadata,
    ) -> anyhow::Result<SkillRevisionRecord> {
        let storage_path = expected.storage_path.clone();
        self.replace_staging_revision_cas(revision_id, expected, &storage_path, metadata)
            .await
    }

    pub async fn replace_staging_revision_cas(
        &self,
        revision_id: &str,
        expected: SkillRevisionExpectation,
        replacement_storage_path: &str,
        metadata: SkillRevisionMetadata,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_uuid_v4("revision_id", revision_id)?;
        if expected.status != crate::skill_state::SkillRevisionStatus::Staging {
            anyhow::bail!(
                "staging metadata CAS expected staging lifecycle, got {}",
                expected.status.as_str()
            );
        }
        validate_storage_path(&expected.storage_path)?;
        validate_storage_path(replacement_storage_path)?;
        let expected_descriptor_json = serde_json::to_string(&expected.descriptor_json)?;
        let expected_validation_json = serde_json::to_string(&expected.validation_json)?;
        let descriptor_json = serde_json::to_string(&metadata.descriptor_json)?;
        let validation_json = serde_json::to_string(&metadata.validation_json)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let query = format!(
                r#"UPDATE skill_revisions
                   SET version = ?, content_hash = ?, storage_path = ?, descriptor_json = ?, validation_json = ?
                   WHERE revision_id = ? AND lifecycle_status = ?
                     AND version = ? AND content_hash = ? AND storage_path = ?
                     AND descriptor_json = ? AND validation_json = ?
                   RETURNING {REVISION_COLUMNS}"#
            );
            let updated = sqlx::query(&query)
                .bind(&metadata.version)
                .bind(&metadata.content_hash)
                .bind(replacement_storage_path)
                .bind(&descriptor_json)
                .bind(&validation_json)
                .bind(revision_id)
                .bind(expected.status.as_str())
                .bind(&expected.version)
                .bind(&expected.content_hash)
                .bind(&expected.storage_path)
                .bind(&expected_descriptor_json)
                .bind(&expected_validation_json)
                .fetch_optional(&mut *tx)
                .await?;
            if let Some(row) = updated {
                return revision_from_row(&row);
            }
            revision_cas_rejection(&mut *tx, revision_id).await
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub async fn promote_revision_record_with_metadata_cas(
        &self,
        revision_id: &str,
        expected: SkillRevisionExpectation,
        promotion: SkillRevisionPromotion,
    ) -> anyhow::Result<SkillRevisionRecord> {
        validate_uuid_v4("revision_id", revision_id)?;
        validate_storage_path(&expected.storage_path)?;
        validate_storage_path(&promotion.storage_path)?;
        let expected_descriptor_json = serde_json::to_string(&expected.descriptor_json)?;
        let expected_validation_json = serde_json::to_string(&expected.validation_json)?;
        let descriptor_json = serde_json::to_string(&promotion.descriptor_json)?;
        let validation_json = serde_json::to_string(&promotion.validation_json)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let query = format!(
                r#"UPDATE skill_revisions
                   SET version = ?, content_hash = ?, storage_path = ?, descriptor_json = ?,
                       validation_json = ?, lifecycle_status = 'managed'
                   WHERE revision_id = ? AND lifecycle_status = ? AND version = ?
                     AND content_hash = ? AND storage_path = ?
                     AND descriptor_json = ? AND validation_json = ?
                   RETURNING {REVISION_COLUMNS}"#
            );
            let updated = sqlx::query(&query)
                .bind(&promotion.version)
                .bind(&promotion.content_hash)
                .bind(&promotion.storage_path)
                .bind(&descriptor_json)
                .bind(&validation_json)
                .bind(revision_id)
                .bind(expected.status.as_str())
                .bind(&expected.version)
                .bind(&expected.content_hash)
                .bind(&expected.storage_path)
                .bind(&expected_descriptor_json)
                .bind(&expected_validation_json)
                .fetch_optional(&mut *tx)
                .await?;
            if let Some(row) = updated {
                return revision_from_row(&row);
            }
            revision_cas_rejection(&mut *tx, revision_id).await
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }
}

async fn revision_cas_rejection<'e, E>(
    executor: E,
    revision_id: &str,
) -> anyhow::Result<SkillRevisionRecord>
where
    E: Executor<'e, Database = Sqlite>,
{
    let exists: Option<String> =
        sqlx::query_scalar("SELECT revision_id FROM skill_revisions WHERE revision_id = ?")
            .bind(revision_id)
            .fetch_optional(executor)
            .await?;
    if exists.is_none() {
        anyhow::bail!("skill revision not found: {revision_id}");
    }
    anyhow::bail!("skill revision changed since operation observation: {revision_id}")
}

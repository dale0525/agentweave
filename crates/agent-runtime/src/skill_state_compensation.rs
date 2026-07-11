use crate::skill_state::{SkillRevisionRecord, SkillRevisionStatus, SkillStateStore};

impl SkillStateStore {
    pub(crate) async fn delete_staging_revision_record_if_matches(
        &self,
        expected: &SkillRevisionRecord,
    ) -> anyhow::Result<()> {
        if expected.status != SkillRevisionStatus::Staging {
            anyhow::bail!(
                "staging compensation expected staging lifecycle, got {}",
                expected.status.as_str()
            );
        }
        let descriptor_json = serde_json::to_string(&expected.descriptor_json)?;
        let validation_json = serde_json::to_string(&expected.validation_json)?;
        let deleted = sqlx::query(
            r#"DELETE FROM skill_revisions
               WHERE revision_id = ? AND package_id = ? AND version = ? AND content_hash = ?
                 AND storage_path = ? AND descriptor_json = ? AND validation_json = ?
                 AND created_by = ? AND created_at = ? AND lifecycle_status = 'staging'"#,
        )
        .bind(&expected.revision_id)
        .bind(expected.package_id.as_str())
        .bind(&expected.version)
        .bind(&expected.content_hash)
        .bind(&expected.storage_path)
        .bind(descriptor_json)
        .bind(validation_json)
        .bind(&expected.created_by)
        .bind(expected.created_at.to_rfc3339())
        .execute(self.pool())
        .await?;
        if deleted.rows_affected() != 1 {
            anyhow::bail!(
                "staging revision changed before compensation: {}",
                expected.revision_id
            );
        }
        Ok(())
    }
}

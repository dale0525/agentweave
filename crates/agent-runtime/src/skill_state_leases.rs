use crate::skill_state::{SkillStateBoundaryError, SkillStateStore};
use chrono::{Duration, Utc};
use std::collections::BTreeSet;

pub(crate) const SNAPSHOT_LEASE_TTL: std::time::Duration = std::time::Duration::from_secs(300);
pub(crate) const SNAPSHOT_LEASE_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(60);

impl SkillStateStore {
    pub(crate) async fn acquire_snapshot_lease(
        &self,
        generation: u64,
        members: &serde_json::Value,
        revision_ids: &BTreeSet<String>,
    ) -> anyhow::Result<String> {
        let lease_id = uuid::Uuid::new_v4().to_string();
        let generation = i64::try_from(generation)?;
        let members_json = serde_json::to_string(members)?;
        let now = Utc::now();
        let expires_at = lease_expiry(now);
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            delete_expired_leases(&mut tx, now).await?;
            let active: Option<(i64, String)> = sqlx::query_as(
                "SELECT generation, members_json FROM skill_snapshots WHERE status = 'active'",
            )
            .fetch_optional(&mut *tx)
            .await?;
            if active.as_ref() != Some(&(generation, members_json.clone())) {
                return Err(state_conflict(
                    "authoritative active snapshot changed before turn lease",
                ));
            }
            sqlx::query(
                r#"INSERT INTO skill_snapshot_leases
                   (lease_id, generation, members_json, expires_at, created_at, updated_at)
                   VALUES (?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&lease_id)
            .bind(generation)
            .bind(&members_json)
            .bind(expires_at.to_rfc3339())
            .bind(now.to_rfc3339())
            .bind(now.to_rfc3339())
            .execute(&mut *tx)
            .await?;
            for revision_id in revision_ids {
                sqlx::query(
                    "INSERT INTO skill_snapshot_lease_revisions (lease_id, revision_id) VALUES (?, ?)",
                )
                .bind(&lease_id)
                .bind(revision_id)
                .execute(&mut *tx)
                .await?;
            }
            Ok(lease_id.clone())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn refresh_snapshot_lease(&self, lease_id: &str) -> anyhow::Result<bool> {
        let now = Utc::now();
        let changed = sqlx::query(
            r#"UPDATE skill_snapshot_leases SET expires_at = ?, updated_at = ?
               WHERE lease_id = ? AND expires_at > ?"#,
        )
        .bind(lease_expiry(now).to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(lease_id)
        .bind(now.to_rfc3339())
        .execute(self.pool())
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    pub(crate) async fn snapshot_lease_authorizes_revision(
        &self,
        lease_id: &str,
        generation: u64,
        revision_id: &str,
    ) -> anyhow::Result<bool> {
        let authorized: Option<i64> = sqlx::query_scalar(
            r#"SELECT 1
               FROM skill_snapshot_leases AS leases
               JOIN skill_snapshot_lease_revisions AS revisions
                 ON revisions.lease_id = leases.lease_id
               WHERE leases.lease_id = ?
                 AND leases.generation = ?
                 AND leases.expires_at > ?
                 AND revisions.revision_id = ?"#,
        )
        .bind(lease_id)
        .bind(i64::try_from(generation)?)
        .bind(Utc::now().to_rfc3339())
        .bind(revision_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(authorized.is_some())
    }

    pub(crate) async fn snapshot_lease_is_authoritative(
        &self,
        lease_id: &str,
        generation: u64,
    ) -> anyhow::Result<bool> {
        let authoritative: Option<i64> = sqlx::query_scalar(
            r#"SELECT 1 FROM skill_snapshot_leases
               WHERE lease_id = ? AND generation = ? AND expires_at > ?"#,
        )
        .bind(lease_id)
        .bind(i64::try_from(generation)?)
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(self.pool())
        .await?;
        Ok(authoritative.is_some())
    }

    pub(crate) async fn release_snapshot_lease(&self, lease_id: &str) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM skill_snapshot_leases WHERE lease_id = ?")
            .bind(lease_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub(crate) async fn durable_snapshot_protections(
        &self,
    ) -> anyhow::Result<(BTreeSet<u64>, BTreeSet<String>)> {
        let now = Utc::now();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            delete_expired_leases(&mut tx, now).await?;
            let generations: Vec<i64> =
                sqlx::query_scalar("SELECT DISTINCT generation FROM skill_snapshot_leases")
                    .fetch_all(&mut *tx)
                    .await?;
            let revisions: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT revision_id FROM skill_snapshot_lease_revisions",
            )
            .fetch_all(&mut *tx)
            .await?;
            Ok((
                generations
                    .into_iter()
                    .map(u64::try_from)
                    .collect::<Result<BTreeSet<_>, _>>()?,
                revisions.into_iter().collect(),
            ))
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }
}

async fn delete_expired_leases(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now: chrono::DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM skill_snapshot_leases WHERE expires_at <= ?")
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn lease_expiry(now: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    now + Duration::from_std(SNAPSHOT_LEASE_TTL).expect("snapshot lease TTL must fit chrono")
}

fn state_conflict(message: &'static str) -> anyhow::Error {
    SkillStateBoundaryError::Conflict(anyhow::anyhow!(message)).into()
}

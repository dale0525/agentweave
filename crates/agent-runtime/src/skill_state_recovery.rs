use crate::skill_package::SkillPackageId;
use crate::skill_recovery::PersistedSnapshotMember;
use crate::skill_state::{
    SkillCircuitStateRecord, SkillRevisionStatus, SkillSnapshotRecord, SkillSnapshotStatus,
    SkillStateBoundaryError, SkillStateStore,
};
use crate::skill_state_rows::{
    CIRCUIT_COLUMNS, SNAPSHOT_COLUMNS, circuit_from_row, snapshot_from_row,
};
use chrono::{DateTime, Duration, Utc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InitialSnapshotPersistence {
    Inserted,
    ExistingExact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CircuitStateTransition {
    None,
    Opened,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CircuitOmissionRecord {
    pub revision_id: String,
    pub package_id: SkillPackageId,
    pub omitted_generation: u64,
    pub consumed: bool,
}

impl SkillStateStore {
    pub(crate) async fn snapshot_with_status(
        &self,
        status: SkillSnapshotStatus,
    ) -> anyhow::Result<Option<SkillSnapshotRecord>> {
        let query = format!(
            "SELECT {SNAPSHOT_COLUMNS} FROM skill_snapshots WHERE status = ? ORDER BY generation DESC LIMIT 1"
        );
        sqlx::query(&query)
            .bind(status.as_str())
            .fetch_optional(self.pool())
            .await?
            .map(|row| snapshot_from_row(&row))
            .transpose()
    }

    pub(crate) async fn persist_initial_active_snapshot(
        &self,
        generation: u64,
        members: &serde_json::Value,
    ) -> anyhow::Result<InitialSnapshotPersistence> {
        let generation = i64::try_from(generation)?;
        let members_json = serde_json::to_string(members)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let active: Option<(i64, String)> = sqlx::query_as(
                "SELECT generation, members_json FROM skill_snapshots WHERE status = 'active'",
            )
            .fetch_optional(&mut *tx)
            .await?;
            if let Some((active_generation, active_members)) = active {
                if active_generation == generation
                    && serde_json::from_str::<serde_json::Value>(&active_members)? == *members
                {
                    return Ok(InitialSnapshotPersistence::ExistingExact);
                }
                return Err(state_conflict("an active skill snapshot already exists"));
            }
            let existing: Option<String> =
                sqlx::query_scalar("SELECT status FROM skill_snapshots WHERE generation = ?")
                    .bind(generation)
                    .fetch_optional(&mut *tx)
                    .await?;
            if existing.is_some() {
                return Err(state_conflict("initial snapshot generation already exists"));
            }
            sqlx::query(
                r#"INSERT INTO skill_snapshots
                   (generation, status, members_json, created_at, activated_at)
                   VALUES (?, 'active', ?, ?, ?)"#,
            )
            .bind(generation)
            .bind(&members_json)
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            Ok(InitialSnapshotPersistence::Inserted)
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn restore_snapshot_as_active(
        &self,
        expected_active: &SkillSnapshotRecord,
        snapshot: &SkillSnapshotRecord,
        members: &[PersistedSnapshotMember],
    ) -> anyhow::Result<()> {
        let generation = i64::try_from(snapshot.generation)?;
        let now = Utc::now().to_rfc3339();
        let managed = members
            .iter()
            .filter(|member| member.layer == "managed")
            .collect::<Vec<_>>();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let snapshot_query =
                format!("SELECT {SNAPSHOT_COLUMNS} FROM skill_snapshots WHERE generation = ?");
            let active = sqlx::query(&snapshot_query)
                .bind(i64::try_from(expected_active.generation)?)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| snapshot_from_row(&row))
                .transpose()?;
            if active.as_ref() != Some(expected_active)
                || expected_active.status != SkillSnapshotStatus::Active
            {
                return Err(state_conflict(
                    "authoritative active snapshot changed during recovery",
                ));
            }
            let target = sqlx::query(&snapshot_query)
                .bind(generation)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| snapshot_from_row(&row))
                .transpose()?;
            if target.as_ref() != Some(snapshot)
                || snapshot.status != SkillSnapshotStatus::LastKnownGood
            {
                return Err(state_conflict(
                    "last-known-good snapshot changed during recovery",
                ));
            }
            for member in &managed {
                let revision_id = member
                    .revision_id
                    .as_deref()
                    .ok_or_else(|| state_conflict("managed snapshot member has no revision"))?;
                let row: Option<(String, String, String, String)> = sqlx::query_as(
                    r#"SELECT package_id, version, content_hash, lifecycle_status
                       FROM skill_revisions WHERE revision_id = ?"#,
                )
                .bind(revision_id)
                .fetch_optional(&mut *tx)
                .await?;
                if row.as_ref()
                    != Some(&(
                        member.package_id.clone(),
                        member.version.clone(),
                        member.content_hash.clone(),
                        "managed".into(),
                    ))
                {
                    return Err(state_conflict("persisted snapshot member is stale"));
                }
            }

            sqlx::query(
                r#"UPDATE skill_installations
                   SET enabled = 0, install_status = 'inactive', updated_at = ?
                   WHERE source_layer = 'managed' AND install_status = 'active'"#,
            )
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            for member in managed {
                let revision_id = member
                    .revision_id
                    .as_deref()
                    .expect("managed member revision checked above");
                let package_id = SkillPackageId::parse(&member.package_id)?;
                sqlx::query(
                    r#"INSERT INTO skill_installations
                       (package_id, source_layer, active_revision_id, enabled, trust_level,
                        install_status, installed_at, updated_at)
                       VALUES (?, 'managed', ?, 1, 'recovered', 'active', ?, ?)
                       ON CONFLICT(package_id) DO UPDATE SET
                         source_layer = 'managed', active_revision_id = excluded.active_revision_id,
                         enabled = 1, install_status = 'active', updated_at = excluded.updated_at"#,
                )
                .bind(&member.package_id)
                .bind(revision_id)
                .bind(&now)
                .bind(&now)
                .execute(&mut *tx)
                .await?;
                crate::skill_state::insert_audit(
                    &mut *tx,
                    "system-recovery",
                    "restore_last_known_good",
                    &package_id,
                    Some(revision_id),
                    "ok",
                    serde_json::json!({
                        "generation": snapshot.generation,
                        "outcome": "restored"
                    }),
                )
                .await?;
            }

            let demoted = sqlx::query(
                "UPDATE skill_snapshots SET status = 'candidate' WHERE generation = ? AND status = 'active' AND members_json = ?",
            )
            .bind(i64::try_from(expected_active.generation)?)
            .bind(serde_json::to_string(&expected_active.members_json)?)
            .execute(&mut *tx)
            .await?;
            if demoted.rows_affected() != 1 {
                return Err(state_conflict(
                    "authoritative active snapshot changed during recovery",
                ));
            }
            let changed = sqlx::query(
                "UPDATE skill_snapshots SET status = 'active', activated_at = ? WHERE generation = ? AND status = 'last_known_good' AND members_json = ?",
            )
            .bind(&now)
            .bind(generation)
            .bind(serde_json::to_string(&snapshot.members_json)?)
            .execute(&mut *tx)
            .await?;
            if changed.rows_affected() != 1 {
                return Err(state_conflict("last-known-good snapshot changed during recovery"));
            }
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn persist_recovery_candidate(
        &self,
        expected_active: &SkillSnapshotRecord,
        generation: u64,
        members: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let generation = i64::try_from(generation)?;
        let previous_generation = i64::try_from(expected_active.generation)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            if expected_active.status != SkillSnapshotStatus::Active {
                return Err(state_conflict("expected recovery snapshot is not active"));
            }
            crate::skill_state_lifecycle::persist_snapshot_transition(
                &mut tx,
                previous_generation,
                &expected_active.members_json,
                generation,
                members,
                &now,
            )
            .await
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn record_managed_execution_result(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        success: bool,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<(SkillCircuitStateRecord, CircuitStateTransition)>> {
        let Some(revision) = self.get_revision(revision_id).await? else {
            return Ok(None);
        };
        if revision.package_id != *package_id || revision.status != SkillRevisionStatus::Managed {
            return Ok(None);
        }

        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let query =
                format!("SELECT {CIRCUIT_COLUMNS} FROM skill_circuit_state WHERE revision_id = ?");
            let existing = sqlx::query(&query)
                .bind(revision_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| circuit_from_row(&row))
                .transpose()?;
            let was_open = existing
                .as_ref()
                .and_then(|state| state.open_until)
                .is_some_and(|deadline| deadline > now);
            let had_open_transition = existing
                .as_ref()
                .is_some_and(|state| state.open_until.is_some());
            let (failures, open_until) = if success {
                (0_u64, None)
            } else if let Some(existing) = &existing {
                if existing.open_until.is_some_and(|deadline| deadline > now) {
                    (existing.consecutive_failures, existing.open_until)
                } else {
                    let base = if existing.open_until.is_some() {
                        0
                    } else {
                        existing.consecutive_failures
                    };
                    let failures = base.saturating_add(1);
                    let open_until = (failures >= 3).then_some(now + Duration::minutes(5));
                    (failures, open_until)
                }
            } else {
                (1, None)
            };
            let failures = i64::try_from(failures)?;
            sqlx::query(
                r#"INSERT INTO skill_circuit_state
                   (revision_id, consecutive_failures, open_until, updated_at)
                   VALUES (?, ?, ?, ?)
                   ON CONFLICT(revision_id) DO UPDATE SET
                     consecutive_failures = excluded.consecutive_failures,
                     open_until = excluded.open_until,
                     updated_at = excluded.updated_at"#,
            )
            .bind(revision_id)
            .bind(failures)
            .bind(open_until.map(|value| value.to_rfc3339()))
            .bind(now.to_rfc3339())
            .execute(&mut *tx)
            .await?;
            let row = sqlx::query(&query)
                .bind(revision_id)
                .fetch_one(&mut *tx)
                .await?;
            let state = circuit_from_row(&row)?;
            let opened = !was_open && state.open_until.is_some_and(|deadline| deadline > now);
            let transition = if opened {
                CircuitStateTransition::Opened
            } else if success && had_open_transition {
                CircuitStateTransition::Closed
            } else {
                CircuitStateTransition::None
            };
            Ok(Some((state, transition)))
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn circuit_is_open(
        &self,
        revision_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<bool> {
        crate::skill_state::validate_revision_id(revision_id)?;
        let open_until: Option<String> =
            sqlx::query_scalar("SELECT open_until FROM skill_circuit_state WHERE revision_id = ?")
                .bind(revision_id)
                .fetch_optional(self.pool())
                .await?
                .flatten();
        open_until
            .map(|value| Ok(DateTime::parse_from_rfc3339(&value)?.with_timezone(&Utc) > now))
            .transpose()
            .map(|value| value.unwrap_or(false))
    }

    pub(crate) async fn circuit_omission(
        &self,
        revision_id: &str,
    ) -> anyhow::Result<Option<CircuitOmissionRecord>> {
        crate::skill_state::validate_revision_id(revision_id)?;
        let row: Option<(String, String, i64, Option<i64>)> = sqlx::query_as(
            r#"SELECT revision_id, package_id, omitted_generation, consumed_generation
               FROM skill_circuit_omissions WHERE revision_id = ?"#,
        )
        .bind(revision_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(
            |(revision_id, package_id, omitted_generation, consumed_generation)| {
                Ok(CircuitOmissionRecord {
                    revision_id,
                    package_id: SkillPackageId::parse(&package_id)?,
                    omitted_generation: u64::try_from(omitted_generation)?,
                    consumed: consumed_generation.is_some(),
                })
            },
        )
        .transpose()
    }
}

fn state_conflict(message: &'static str) -> anyhow::Error {
    SkillStateBoundaryError::Conflict(anyhow::anyhow!(message)).into()
}

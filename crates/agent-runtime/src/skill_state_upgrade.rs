use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    SkillInstallStatus, SkillInstallationRecord, SkillSnapshotRecord, SkillSnapshotStatus,
    SkillStateBoundaryError, SkillStateStore,
};
use crate::skill_state_rows::{INSTALLATION_COLUMNS, installation_from_row};
use chrono::Utc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApplicationGraphState {
    pub(crate) fingerprint: String,
    pub(crate) snapshot_generation: u64,
}

pub(crate) struct ApplicationUpdateTransition {
    pub(crate) installation: SkillInstallationRecord,
    pub(crate) reason: String,
}

pub(crate) struct ApplicationUpdatePublication<'a> {
    pub(crate) expected_snapshot: &'a SkillSnapshotRecord,
    pub(crate) expected_graph: Option<&'a ApplicationGraphState>,
    pub(crate) fingerprint: &'a str,
    pub(crate) generation: u64,
    pub(crate) members: &'a serde_json::Value,
    pub(crate) transitions: &'a [ApplicationUpdateTransition],
}

impl SkillStateStore {
    pub(crate) async fn application_graph_state(
        &self,
    ) -> anyhow::Result<Option<ApplicationGraphState>> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT graph_fingerprint, snapshot_generation FROM skill_application_state WHERE singleton = 1",
        )
        .fetch_optional(self.pool())
        .await?;
        row.map(|(fingerprint, generation)| {
            validate_fingerprint(&fingerprint)?;
            Ok(ApplicationGraphState {
                fingerprint,
                snapshot_generation: u64::try_from(generation)?,
            })
        })
        .transpose()
    }

    pub(crate) async fn record_initial_application_graph(
        &self,
        generation: u64,
        fingerprint: &str,
    ) -> anyhow::Result<()> {
        validate_fingerprint(fingerprint)?;
        let generation = i64::try_from(generation)?;
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let active: Option<i64> = sqlx::query_scalar(
                "SELECT generation FROM skill_snapshots WHERE status = 'active'",
            )
            .fetch_optional(&mut *tx)
            .await?;
            if active != Some(generation) {
                return Err(state_conflict(
                    "active snapshot changed before graph recording",
                ));
            }
            sqlx::query(
                r#"INSERT INTO skill_application_state
                   (singleton, graph_fingerprint, snapshot_generation, updated_at)
                   VALUES (1, ?, ?, ?)
                   ON CONFLICT(singleton) DO NOTHING"#,
            )
            .bind(fingerprint)
            .bind(generation)
            .bind(Utc::now().to_rfc3339())
            .execute(&mut *tx)
            .await?;
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }

    pub(crate) async fn commit_application_update(
        &self,
        input: ApplicationUpdatePublication<'_>,
    ) -> anyhow::Result<()> {
        validate_fingerprint(input.fingerprint)?;
        if input.expected_snapshot.status != SkillSnapshotStatus::Active {
            return Err(state_conflict(
                "application update requires an active snapshot",
            ));
        }
        let generation = i64::try_from(input.generation)?;
        let previous_generation = i64::try_from(input.expected_snapshot.generation)?;
        let now = Utc::now().to_rfc3339();
        let mut tx = crate::skill_state_transactions::begin_immediate(self.pool()).await?;
        let result = async {
            let graph: Option<(String, i64)> = sqlx::query_as(
                "SELECT graph_fingerprint, snapshot_generation FROM skill_application_state WHERE singleton = 1",
            )
            .fetch_optional(&mut *tx)
            .await?;
            let expected_graph = input.expected_graph.map(|state| {
                (
                    state.fingerprint.clone(),
                    i64::try_from(state.snapshot_generation).expect("validated graph generation"),
                )
            });
            if graph != expected_graph {
                return Err(state_conflict("application graph changed during reconciliation"));
            }
            for transition in input.transitions {
                let query = format!(
                    "SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE package_id = ?"
                );
                let current = sqlx::query(&query)
                    .bind(transition.installation.package_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(|row| installation_from_row(&row))
                    .transpose()?;
                if current.as_ref() != Some(&transition.installation) {
                    return Err(state_conflict(
                        "managed installation changed during application update",
                    ));
                }
                let changed = sqlx::query(
                    r#"UPDATE skill_installations
                       SET enabled = 0, install_status = 'inactive', updated_at = ?
                       WHERE package_id = ? AND source_layer = 'managed'
                         AND active_revision_id = ? AND enabled = 1 AND install_status = 'active'"#,
                )
                .bind(&now)
                .bind(transition.installation.package_id.as_str())
                .bind(transition.installation.active_revision_id.as_deref())
                .execute(&mut *tx)
                .await?;
                if changed.rows_affected() != 1 {
                    return Err(state_conflict(
                        "managed installation changed during application update",
                    ));
                }
            }
            crate::skill_state_lifecycle::persist_snapshot_transition(
                &mut tx,
                previous_generation,
                &input.expected_snapshot.members_json,
                generation,
                input.members,
                &now,
            )
            .await?;
            sqlx::query(
                r#"INSERT INTO skill_application_state
                   (singleton, graph_fingerprint, snapshot_generation, updated_at)
                   VALUES (1, ?, ?, ?)
                   ON CONFLICT(singleton) DO UPDATE SET
                     graph_fingerprint = excluded.graph_fingerprint,
                     snapshot_generation = excluded.snapshot_generation,
                     updated_at = excluded.updated_at"#,
            )
            .bind(input.fingerprint)
            .bind(generation)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
            for transition in input.transitions {
                crate::skill_state::insert_audit(
                    &mut *tx,
                    "system-application-update",
                    "application_update_inactivated",
                    &transition.installation.package_id,
                    transition.installation.active_revision_id.as_deref(),
                    "ok",
                    serde_json::json!({
                        "from": SkillInstallStatus::Active.as_str(),
                        "to": SkillInstallStatus::Inactive.as_str(),
                        "reason": transition.reason,
                        "generation": generation
                    }),
                )
                .await?;
            }
            Ok(())
        }
        .await;
        crate::skill_state_transactions::finish(tx, result).await
    }
}

fn validate_fingerprint(value: &str) -> anyhow::Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SkillStateBoundaryError::InvalidInput(anyhow::anyhow!(
            "invalid application graph fingerprint"
        ))
        .into());
    }
    Ok(())
}

fn state_conflict(message: &'static str) -> anyhow::Error {
    SkillStateBoundaryError::Conflict(anyhow::anyhow!(message)).into()
}

#[allow(dead_code)]
fn _package_id_type_anchor(_: &SkillPackageId) {}

use crate::skill_package::SkillPackageId;
use crate::skill_state::SkillStateStore;
use crate::skill_state_rows::{
    INSTALLATION_COLUMNS, REVISION_COLUMNS, SkillInstallationRecord, SkillRevisionRecord,
    installation_from_row, revision_from_row,
};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

const MANAGED_INSTALLATION_VIEW_COLUMNS: &str = "i.package_id AS package_id, i.source_layer AS source_layer, i.active_revision_id AS active_revision_id, i.enabled AS enabled, i.trust_level AS trust_level, i.install_status AS install_status, i.installed_at AS installed_at, i.updated_at AS updated_at, r.revision_id AS joined_revision_id, r.package_id AS active_revision_package_id, r.version AS active_version, r.lifecycle_status AS active_revision_lifecycle";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedSkillInstallationView {
    pub installation: SkillInstallationRecord,
    pub active_version: Option<String>,
}

impl SkillStateStore {
    pub async fn list_package_revisions(
        &self,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<Vec<SkillRevisionRecord>> {
        let query = format!(
            "SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE package_id = ? ORDER BY created_at DESC, revision_id DESC"
        );
        sqlx::query(&query)
            .bind(package_id.as_str())
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(revision_from_row)
            .collect()
    }

    pub async fn list_staging_revisions(&self) -> anyhow::Result<Vec<SkillRevisionRecord>> {
        let query = format!(
            "SELECT {REVISION_COLUMNS} FROM skill_revisions WHERE lifecycle_status = 'staging' ORDER BY created_at DESC, revision_id DESC"
        );
        sqlx::query(&query)
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(revision_from_row)
            .collect()
    }

    pub async fn get_installation(
        &self,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<Option<SkillInstallationRecord>> {
        let query =
            format!("SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE package_id = ?");
        sqlx::query(&query)
            .bind(package_id.as_str())
            .fetch_optional(self.pool())
            .await?
            .map(|row| installation_from_row(&row))
            .transpose()
    }

    pub async fn list_active_installations(&self) -> anyhow::Result<Vec<SkillInstallationRecord>> {
        let query = format!(
            "SELECT {INSTALLATION_COLUMNS} FROM skill_installations WHERE enabled = 1 AND install_status = 'active' ORDER BY package_id"
        );
        sqlx::query(&query)
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(installation_from_row)
            .collect()
    }

    pub async fn list_installations(&self) -> anyhow::Result<Vec<SkillInstallationRecord>> {
        let query =
            format!("SELECT {INSTALLATION_COLUMNS} FROM skill_installations ORDER BY package_id");
        sqlx::query(&query)
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(installation_from_row)
            .collect()
    }

    pub async fn list_managed_installations_with_revisions(
        &self,
    ) -> anyhow::Result<Vec<ManagedSkillInstallationView>> {
        let query = format!(
            "SELECT {MANAGED_INSTALLATION_VIEW_COLUMNS} FROM skill_installations i LEFT JOIN skill_revisions r ON r.revision_id = i.active_revision_id AND r.package_id = i.package_id WHERE i.source_layer = 'managed' ORDER BY i.package_id"
        );
        sqlx::query(&query)
            .fetch_all(self.pool())
            .await?
            .iter()
            .map(managed_installation_view_from_row)
            .collect()
    }
}

fn managed_installation_view_from_row(
    row: &SqliteRow,
) -> anyhow::Result<ManagedSkillInstallationView> {
    let installation = installation_from_row(row)?;
    let joined_revision_id: Option<String> = row.try_get("joined_revision_id")?;
    let revision_package_id: Option<String> = row.try_get("active_revision_package_id")?;
    let active_version: Option<String> = row.try_get("active_version")?;
    let lifecycle: Option<String> = row.try_get("active_revision_lifecycle")?;
    match installation.active_revision_id.as_deref() {
        None => {
            if joined_revision_id.is_some()
                || revision_package_id.is_some()
                || active_version.is_some()
                || lifecycle.is_some()
            {
                anyhow::bail!(
                    "managed installation consistency error for {}: inactive row joined an active revision",
                    installation.package_id.as_str()
                );
            }
        }
        Some(active_revision_id) => {
            if joined_revision_id.as_deref() != Some(active_revision_id)
                || revision_package_id.as_deref() != Some(installation.package_id.as_str())
                || active_version.is_none()
            {
                anyhow::bail!(
                    "managed installation consistency error for {}: active revision {} is missing or belongs to another package",
                    installation.package_id.as_str(),
                    active_revision_id
                );
            }
            if lifecycle.as_deref() != Some("managed") {
                anyhow::bail!(
                    "managed installation consistency error for {}: active revision {} lifecycle is not managed",
                    installation.package_id.as_str(),
                    active_revision_id
                );
            }
        }
    }
    Ok(ManagedSkillInstallationView {
        installation,
        active_version,
    })
}

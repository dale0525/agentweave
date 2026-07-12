use crate::skill_state::{SkillRevisionRecord, SkillRevisionStatus};
use crate::skill_store::SkillRevisionStore;
use crate::skill_store_locks::acquire_revision_lock;
use crate::skill_store_prepared_fs::open_regular_file;
use crate::skill_store_secure_roots::{open_prepared_directory, opened_package_snapshot};
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

pub(crate) struct SkillRevisionInspection {
    pub(crate) instructions: String,
}

impl SkillRevisionStore {
    pub(crate) async fn inspect_revision_content(
        &self,
        record: &SkillRevisionRecord,
    ) -> anyhow::Result<SkillRevisionInspection> {
        self.paths.verify_identity()?;
        let _guard =
            acquire_revision_lock(&self.paths.identity, &record.revision_id, &self.faults).await?;
        if self.state.get_revision(&record.revision_id).await?.as_ref() != Some(record) {
            anyhow::bail!("skill revision changed while waiting for inspection lock");
        }
        let (identity, relative, expected_path) = match record.status {
            SkillRevisionStatus::Staging => {
                let (expected, relative) = self.staging_revision_path(record)?;
                (self.paths.staging_identity(), relative, expected)
            }
            SkillRevisionStatus::Managed => {
                let relative = PathBuf::from(record.package_id.as_str())
                    .join("revisions")
                    .join(&record.revision_id);
                (
                    self.paths.managed_identity(),
                    relative.clone(),
                    self.paths.managed.join(relative),
                )
            }
            SkillRevisionStatus::Quarantined => (
                self.paths.quarantine_identity(),
                PathBuf::from(&record.revision_id),
                self.paths.quarantine.join(&record.revision_id),
            ),
        };
        if Path::new(&record.storage_path) != expected_path {
            anyhow::bail!("skill revision storage binding is invalid");
        }
        let directory = open_prepared_directory(identity, &relative).await?;
        if record.status == SkillRevisionStatus::Staging {
            let descriptor: crate::skill_package::SkillPackageDescriptor =
                serde_json::from_value(record.descriptor_json.clone())?;
            let instructions = if descriptor.package.include_instructions {
                let (mut file, length, _) =
                    open_regular_file(&directory, Path::new("SKILL.md")).await?;
                if length > self.limits.max_file_bytes {
                    anyhow::bail!("skill instruction file exceeds inspection limit");
                }
                let capacity = usize::try_from(length)?;
                let mut bytes = Vec::with_capacity(capacity);
                file.read_to_end(&mut bytes).await?;
                if bytes.len() != capacity {
                    anyhow::bail!("skill instruction file changed while reading");
                }
                String::from_utf8(bytes)?
            } else {
                String::new()
            };
            directory.verify()?;
            return Ok(SkillRevisionInspection { instructions });
        }
        let snapshot = opened_package_snapshot(&directory, self.limits.package_limits()).await?;
        directory.verify()?;
        if snapshot.content_hash != record.content_hash
            || snapshot.descriptor.descriptor.id != record.package_id
            || serde_json::to_value(&snapshot.descriptor.descriptor)? != record.descriptor_json
        {
            anyhow::bail!("skill revision content does not match recorded metadata");
        }
        let instructions = match snapshot.instructions_file {
            Some(bytes) => String::from_utf8(bytes)?,
            None => String::new(),
        };
        Ok(SkillRevisionInspection { instructions })
    }
}

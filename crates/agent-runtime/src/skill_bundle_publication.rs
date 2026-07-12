use super::{BundleSkillSource, SkillBundleGeneration};
use crate::skill_store_atomic_write::OwnedAtomicReplace;
use crate::skill_store_secure_roots::PreparedStoreDirectory;
use anyhow::Context;

#[cfg(test)]
use super::builder_gates::checkpoint_after_current_commit;

#[cfg(not(test))]
async fn checkpoint_after_current_commit(_output_root: &std::path::Path) {}

pub(super) async fn verify_first_publication_or_neutralize(
    output: &PreparedStoreDirectory,
    expected: &SkillBundleGeneration,
    marker: OwnedAtomicReplace,
) -> anyhow::Result<()> {
    checkpoint_after_current_commit(output.path()).await;
    let verification = async {
        let source = BundleSkillSource::open(output.path()).await?;
        let selected = source.current_generation().await?;
        anyhow::ensure!(
            selected.as_ref() == Some(expected),
            "committed first bundle publication did not select the expected active generation"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;
    match verification {
        Ok(()) => Ok(()),
        Err(error) => {
            marker.neutralize().await.with_context(|| {
                format!(
                    "first bundle publication verification failed ({error:#}); exact current marker neutralization failed"
                )
            })?;
            Err(error.context(
                "first bundle publication verification failed; exact current marker was neutralized",
            ))
        }
    }
}

use super::*;

pub(super) async fn quarantine_invalid_snapshot_members(
    backend: &ManagedRuntimeBackend,
    record: &crate::skill_state::SkillSnapshotRecord,
) -> SnapshotQuarantineResult {
    let Ok(members) = crate::skill_recovery::parse_snapshot_members(record.members_json.clone())
    else {
        return SnapshotQuarantineResult {
            revisions: Vec::new(),
            failures: 1,
        };
    };
    let source = crate::skill_source::ManagedSkillSource::from_store(backend.revisions.clone());
    let mut result = SnapshotQuarantineResult::default();
    for member in members
        .into_iter()
        .filter(|member| member.layer == "managed")
    {
        let (Ok(package_id), Some(revision_id)) = (
            SkillPackageId::parse(&member.package_id),
            member.revision_id.as_deref(),
        ) else {
            result.failures += 1;
            continue;
        };
        let revision_record = match backend.state.get_revision(revision_id).await {
            Ok(record) => record,
            Err(_) => {
                result.failures += 1;
                continue;
            }
        };
        let descriptor_matches = revision_record
            .as_ref()
            .and_then(|record| {
                serde_json::from_value::<crate::skill_package::SkillPackageDescriptor>(
                    record.descriptor_json.clone(),
                )
                .ok()
                .map(|descriptor| {
                    descriptor.id == package_id && descriptor.version.to_string() == member.version
                })
            })
            .unwrap_or(false);
        let row_matches = revision_record.as_ref().is_some_and(|record| {
            record.package_id == package_id
                && record.revision_id == revision_id
                && record.version == member.version
                && record.content_hash == member.content_hash
                && record.status == crate::skill_state::SkillRevisionStatus::Managed
                && descriptor_matches
        });
        let identity_matches = if row_matches {
            backend
                .revisions
                .proves_managed_revision_identity(
                    revision_record
                        .as_ref()
                        .expect("matching revision checked above"),
                )
                .await
                .unwrap_or(false)
        } else {
            false
        };
        if !identity_matches {
            let key = format!(
                "snapshot-member:{}:{}:{}",
                record.generation, member.package_id, revision_id
            );
            if backend
                .state
                .record_maintenance_diagnostic_once(
                    &key,
                    Some(revision_id),
                    "managed",
                    "snapshot_member_ownership_mismatch",
                    serde_json::json!({
                        "generation": record.generation,
                        "ownership": "unproven"
                    }),
                )
                .await
                .is_err()
            {
                result.failures += 1;
            }
            continue;
        }
        if source
            .load_managed_revision(&package_id, revision_id)
            .await
            .is_ok()
        {
            continue;
        }
        match backend
            .revisions
            .quarantine_revision(revision_id, "startup active snapshot verification failed")
            .await
        {
            Ok(_) => result.revisions.push(revision_id.to_string()),
            Err(_) => result.failures += 1,
        }
    }
    result.revisions.sort();
    result
}

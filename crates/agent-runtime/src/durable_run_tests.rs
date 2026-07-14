use super::*;
use crate::approval::{ApprovalBinding, ApprovalDecision, ApprovalRisk, ApprovalStatus};
use chrono::Duration;

fn scope() -> RunScope {
    RunScope {
        app_id: "com.example.secretary".into(),
        agent_id: "secretary".into(),
        tenant_id: "local".into(),
        user_id: "user".into(),
        session_id: Some("session-1".into()),
    }
}

async fn run_step_action(
    store: &DurableRunStore,
    now: DateTime<Utc>,
    approval_required: bool,
) -> (DurableRun, DurableStep, DurableAction) {
    let run = store
        .create_run(scope(), "Send an approved reply", now)
        .await
        .unwrap();
    assert!(
        store
            .transition_run(
                &run.run_id,
                1,
                RunStatus::Running,
                serde_json::json!({}),
                now
            )
            .await
            .unwrap()
    );
    let step = store
        .add_step(&run.run_id, 0, "tool_action", serde_json::json!({}), now)
        .await
        .unwrap();
    let action = store
        .queue_action(
            QueueActionRequest {
                run_id: &run.run_id,
                step_id: &step.step_id,
                action_name: "mail.send",
                arguments: serde_json::json!({"draft_id": "draft-1", "revision": 2}),
                resource_target: "mail-account:primary",
                idempotency_key: "outbox-1",
                approval_required,
            },
            now,
        )
        .await
        .unwrap();
    (run, step, action)
}

fn binding(action: &DurableAction, now: DateTime<Utc>) -> ApprovalBinding {
    ApprovalBinding {
        actor_id: "user".into(),
        app_id: "com.example.secretary".into(),
        run_id: action.run_id.clone(),
        action_id: action.action_id.clone(),
        action_name: action.action_name.clone(),
        arguments_sha256: action.arguments_sha256.clone(),
        resource_target: action.resource_target.clone(),
        policy_version: "policy-v1".into(),
        risk: ApprovalRisk::ExternalWrite,
        risk_summary: "Send one email from the primary account".into(),
        session_id: Some("session-1".into()),
        expires_at: now + Duration::minutes(10),
    }
}

#[tokio::test]
async fn approval_survives_restart_and_resumes_exact_action_once() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("runs.db");
    let url = format!("sqlite://{}?mode=rwc", database.display());
    let now = Utc::now();
    let store = DurableRunStore::connect(&url).await.unwrap();
    let (run, _step, action) = run_step_action(&store, now, true).await;
    let binding = binding(&action, now);
    let approval = store.request_approval(binding.clone(), now).await.unwrap();
    assert!(
        store
            .bind_action_approval(&action.action_id, &approval.approval_id, now)
            .await
            .unwrap()
    );
    assert!(
        store
            .transition_run(
                &run.run_id,
                2,
                RunStatus::WaitingApproval,
                serde_json::json!({"approval_id": approval.approval_id}),
                now,
            )
            .await
            .unwrap()
    );
    store.close().await;

    let restarted = DurableRunStore::connect(&url).await.unwrap();
    let recovered = restarted.recoverable_runs().await.unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, RunStatus::WaitingApproval);
    let resolved = restarted
        .resolve_approval(
            &approval.approval_id,
            ApprovalDecision::ApproveOnce,
            "approver",
            now + Duration::seconds(1),
        )
        .await
        .unwrap();
    assert_eq!(resolved.status, ApprovalStatus::Approved);
    assert!(
        restarted
            .consume_approval(&approval.approval_id, &binding, now + Duration::seconds(2))
            .await
            .unwrap()
    );
    assert!(
        !restarted
            .consume_approval(&approval.approval_id, &binding, now + Duration::seconds(3))
            .await
            .unwrap_or(false)
    );
    let ready = restarted
        .get_action(&action.action_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.status, ActionStatus::Ready);
    assert!(
        restarted
            .begin_action(&ready.action_id, ready.version, now + Duration::seconds(3))
            .await
            .unwrap()
    );
    assert!(
        restarted
            .complete_action(
                &ready.action_id,
                ActionOutcome::Succeeded,
                serde_json::json!({"delivery_id": "delivery-1"}),
                None,
                now + Duration::seconds(4),
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn approval_is_first_answer_wins_and_argument_drift_fails_closed() {
    let store = DurableRunStore::connect("sqlite::memory:").await.unwrap();
    let now = Utc::now();
    let (_run, _step, action) = run_step_action(&store, now, true).await;
    let original = binding(&action, now);
    let approval = store.request_approval(original.clone(), now).await.unwrap();
    store
        .bind_action_approval(&action.action_id, &approval.approval_id, now)
        .await
        .unwrap();
    let left = store.clone();
    let right = store.clone();
    let id_left = approval.approval_id.clone();
    let id_right = approval.approval_id.clone();
    let (first, second) = tokio::join!(
        left.resolve_approval(&id_left, ApprovalDecision::ApproveOnce, "a", now),
        right.resolve_approval(&id_right, ApprovalDecision::Reject, "b", now)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.status, second.status);
    assert_eq!(first.decision, second.decision);
    let mut drifted = original;
    drifted.arguments_sha256 =
        immutable_arguments_hash(&serde_json::json!({"draft_id": "other"})).unwrap();
    assert!(
        store
            .consume_approval(&approval.approval_id, &drifted, now)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn action_and_outbox_idempotency_prevent_duplicate_or_uncertain_retry() {
    let store = DurableRunStore::connect("sqlite::memory:").await.unwrap();
    let now = Utc::now();
    let (run, step, action) = run_step_action(&store, now, false).await;
    let replay = store
        .queue_action(
            QueueActionRequest {
                run_id: &run.run_id,
                step_id: &step.step_id,
                action_name: &action.action_name,
                arguments: action.arguments.clone(),
                resource_target: &action.resource_target,
                idempotency_key: &action.idempotency_key,
                approval_required: false,
            },
            now,
        )
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.action_id, action.action_id);
    let outbox = store
        .enqueue_outbox(
            &action.action_id,
            "delivery-1",
            serde_json::json!({"message": "one"}),
            now,
        )
        .await
        .unwrap();
    assert!(store.claim_outbox(&outbox.outbox_id, now).await.unwrap());
    assert!(
        store
            .finish_outbox(
                &outbox.outbox_id,
                OutboxStatus::Uncertain,
                None,
                Some("connection dropped after DATA"),
                now,
            )
            .await
            .unwrap()
    );
    assert!(!store.claim_outbox(&outbox.outbox_id, now).await.unwrap());
}

#[tokio::test]
async fn invalid_transition_does_not_mutate_recoverable_truth() {
    let store = DurableRunStore::connect("sqlite::memory:").await.unwrap();
    let now = Utc::now();
    let run = store.create_run(scope(), "test", now).await.unwrap();
    assert!(
        store
            .transition_run(&run.run_id, 1, RunStatus::Succeeded, Value::Null, now)
            .await
            .is_err()
    );
    let recovered = store.recoverable_runs().await.unwrap();
    assert_eq!(recovered[0].status, RunStatus::Queued);
    assert_eq!(recovered[0].version, 1);
}

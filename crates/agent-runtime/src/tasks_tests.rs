use super::*;

fn scope(user: &str) -> TaskScope {
    TaskScope {
        app_id: "com.example.app".into(),
        tenant_id: "local".into(),
        user_id: user.into(),
    }
}
fn content(title: &str) -> TaskContent {
    TaskContent {
        title: title.into(),
        notes: None,
        due_at: None,
        timezone: None,
        recurrence: None,
        priority: TaskPriority::Normal,
        tags: vec!["work".into()],
    }
}

#[tokio::test]
async fn task_provider_is_scoped_idempotent_and_versioned() {
    let provider = FakeTaskProvider::default();
    let first = provider
        .create(&scope("user"), content("Prepare brief"), "task-1")
        .await
        .unwrap();
    let repeated = provider
        .create(&scope("user"), content("Prepare brief"), "task-1")
        .await
        .unwrap();
    assert_eq!(first.id, repeated.id);
    assert!(
        provider
            .list(
                &scope("other"),
                TaskQuery {
                    status: None,
                    due_before: None,
                    tag: None,
                    text: None,
                    limit: 10
                }
            )
            .await
            .unwrap()
            .is_empty()
    );
    let completed = provider
        .set_status(&scope("user"), &first.id, 1, TaskStatus::Completed)
        .await
        .unwrap();
    assert_eq!(completed.version, 2);
    assert!(completed.completed_at.is_some());
    assert!(
        provider
            .update(&scope("user"), &first.id, 1, content("Stale"))
            .await
            .unwrap_err()
            .to_string()
            .contains("version")
    );
}

#[test]
fn recurrence_requires_due_time_and_valid_timezone() {
    let mut value = content("Recurring");
    value.recurrence = Some("FREQ=DAILY".into());
    assert!(value.validate().is_err());
    value.due_at = Some(Utc::now());
    value.timezone = Some("Asia/Shanghai".into());
    value.validate().unwrap();
}

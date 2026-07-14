use super::*;
use chrono::Duration;

fn scope(user: &str) -> CalendarScope {
    CalendarScope {
        app_id: "com.example.app".into(),
        tenant_id: "local".into(),
        user_id: user.into(),
        account_id: "primary".into(),
    }
}

fn content(start: DateTime<Utc>, title: &str) -> CalendarEventContent {
    CalendarEventContent {
        calendar_id: "main".into(),
        title: title.into(),
        description: None,
        start,
        end: start + Duration::hours(1),
        timezone: "Asia/Shanghai".into(),
        location: None,
        attendees: vec![],
        recurrence: None,
    }
}

#[tokio::test]
async fn fake_calendar_surfaces_conflicts_and_applies_approved_preview_once() {
    let connector = FakeCalendarConnector::default();
    let now = Utc::now();
    connector.seed(
        scope("user"),
        CalendarEvent {
            id: "existing".into(),
            content: content(now, "Existing"),
            status: CalendarEventStatus::Confirmed,
            version: 1,
            provider_id: None,
            updated_at: now,
        },
    );
    let preview = connector
        .preview_create(
            &scope("user"),
            content(now + Duration::minutes(30), "Overlap"),
            "create-1".into(),
        )
        .await
        .unwrap();
    assert_eq!(preview.conflicts.len(), 1);
    let approval = ApprovedCalendarMutation {
        preview_id: preview.preview_id.clone(),
        preview_hash: preview.preview_hash.clone(),
        approval_id: "approval-1".into(),
    };
    let first = connector
        .apply(&scope("user"), approval.clone())
        .await
        .unwrap();
    let second = connector.apply(&scope("user"), approval).await.unwrap();
    assert_eq!(first.id, second.id);
    assert!(
        connector
            .list_events(
                &scope("other"),
                now - Duration::hours(1),
                now + Duration::hours(2)
            )
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn fake_calendar_rejects_stale_or_mismatched_approval() {
    let connector = FakeCalendarConnector::default();
    let preview = connector
        .preview_create(
            &scope("user"),
            content(Utc::now(), "Meeting"),
            "create-2".into(),
        )
        .await
        .unwrap();
    let error = connector
        .apply(
            &scope("user"),
            ApprovedCalendarMutation {
                preview_id: preview.preview_id,
                preview_hash: "wrong".into(),
                approval_id: "approval-2".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("does not match"));
}

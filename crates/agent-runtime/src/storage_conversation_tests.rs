use super::*;
use crate::session::messages_to_model_history;

#[tokio::test]
async fn conversation_scope_isolates_apps_users_and_devices() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let scope_a = ConversationScope::local("com.example.app-a");
    let mut scope_b = ConversationScope::local("com.example.app-b");
    scope_b.user_id = "other-user".into();
    scope_b.device_id = "other-device".into();
    let session = storage
        .create_scoped_session(&scope_a, "Scoped")
        .await
        .unwrap();
    storage
        .append_scoped_turn(&scope_a, &session.id, "hello", "hi")
        .await
        .unwrap();

    assert!(
        storage
            .session_exists_scoped(&scope_a, &session.id)
            .await
            .unwrap()
    );
    assert!(
        !storage
            .session_exists_scoped(&scope_b, &session.id)
            .await
            .unwrap()
    );
    assert!(
        storage
            .list_scoped_messages(&scope_b, &session.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        storage
            .list_scoped_sessions(&scope_b)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn runtime_events_are_committed_with_the_turn_and_can_be_replayed() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let scope = ConversationScope::default();
    let session = storage
        .create_scoped_session(&scope, "Events")
        .await
        .unwrap();
    let events = vec![
        RuntimeEvent::TurnStarted {
            turn_id: "turn-1".into(),
        },
        RuntimeEvent::TurnFinished {
            turn_id: "turn-1".into(),
        },
    ];

    storage
        .append_scoped_turn_with_events(&scope, &session.id, "hello", "hi", &events)
        .await
        .unwrap();
    let persisted = storage
        .list_conversation_events(&scope, &session.id)
        .await
        .unwrap();

    assert_eq!(persisted.len(), 2);
    assert_eq!(persisted[0].event_index, 0);
    assert_eq!(persisted[0].kind, "turn_started");
    assert_eq!(persisted[1].kind, "turn_finished");
    assert_eq!(persisted[0].payload["turn_id"], "turn-1");
}

#[tokio::test]
async fn messages_survive_restart_and_form_safe_model_history() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("conversation.db");
    let url = format!("sqlite://{}?mode=rwc", database.display());
    let scope = ConversationScope::local("com.example.restart");
    let storage = Storage::connect(&url).await.unwrap();
    let session = storage
        .create_scoped_session(&scope, "Restart")
        .await
        .unwrap();
    storage
        .append_scoped_turn(&scope, &session.id, "我喜欢下午开会", "记住了")
        .await
        .unwrap();
    storage.close().await;

    let restarted = Storage::connect(&url).await.unwrap();
    let messages = restarted
        .list_scoped_messages(&scope, &session.id)
        .await
        .unwrap();
    let history = messages_to_model_history(&messages).unwrap();

    assert_eq!(history.len(), 2);
    assert_eq!(history[0]["role"], "user");
    assert_eq!(history[0]["content"], "我喜欢下午开会");
    assert_eq!(history[1]["role"], "assistant");
}

#[tokio::test]
async fn conversation_search_is_scoped_and_bounded() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let scope = ConversationScope::local("com.example.search");
    let other = ConversationScope::local("com.example.other");
    let session = storage
        .create_scoped_session(&scope, "Search")
        .await
        .unwrap();
    let hidden = storage
        .create_scoped_session(&other, "Hidden")
        .await
        .unwrap();
    storage
        .append_scoped_turn(&scope, &session.id, "北京会议", "下午三点")
        .await
        .unwrap();
    storage
        .append_scoped_turn(&other, &hidden.id, "北京秘密", "hidden")
        .await
        .unwrap();

    let matches = storage
        .search_scoped_messages(&scope, "北京", 10)
        .await
        .unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].session_id, session.id);
    assert!(
        storage
            .search_scoped_messages(&scope, "", 10)
            .await
            .is_err()
    );
}

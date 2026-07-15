use super::*;
use agent_runtime::connector::ConnectorRuntime;
use agent_runtime::connector_tools::{ConnectorToolRuntime, EphemeralConnectorContextProvider};
use agent_runtime::credential::{CredentialScope, CredentialVault, InMemorySecretStore};
use agent_runtime::credential_sqlite::SqliteCredentialMetadataStore;
use agent_runtime::foundation_actions::MailActionService;
use agent_runtime::mail::{DraftContent, MailAccount, MailAddress, OutgoingBody};
use agent_runtime::mail_connector_transport::MailConnectorTransport;
use agent_runtime::mail_fake::FakeMailConnector;
use agent_runtime::mail_imap_smtp_accounts::{
    ImapSmtpMailAccountManager, ManagedImapSmtpMailConnector, SqliteImapSmtpMailAccountStore,
};
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use std::time::Duration;
use tower::ServiceExt;

#[tokio::test]
async fn foundation_mail_approval_api_resumes_exactly_once() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let mail = Arc::new(FakeMailConnector::new());
    mail.add_account(MailAccount {
        id: "primary".into(),
        display_name: "Work Mail".into(),
        primary_address: MailAddress {
            name: Some("Local User".into()),
            address: "local@example.test".into(),
        },
        addresses: Vec::new(),
        provider_reference: None,
    })
    .unwrap();
    let runtime = Arc::new(ConnectorRuntime::new(None, 256 * 1024).unwrap());
    runtime
        .register(
            MailConnectorTransport::descriptor("Fake Mail", true),
            Arc::new(MailConnectorTransport::new(mail.clone())),
        )
        .await
        .unwrap();
    let scope = CredentialScope {
        app_id: "agentweave.default".into(),
        tenant_id: "local".into(),
        user_id: "local-user".into(),
    };
    let context = Arc::new(
        EphemeralConnectorContextProvider::fail_closed(scope.clone(), Duration::from_secs(2))
            .unwrap(),
    );
    let tools = ConnectorToolRuntime::load(runtime, context.clone()).unwrap();
    let draft = tools
        .execute_trusted_host_action(
            "mail_draft_create",
            "api-draft",
            serde_json::to_value(agent_runtime::mail::CreateDraftRequest {
                account_id: "primary".into(),
                content: DraftContent {
                    to: vec![MailAddress {
                        name: Some("Recipient".into()),
                        address: "recipient@example.test".into(),
                    }],
                    cc: Vec::new(),
                    bcc: Vec::new(),
                    subject: "Approval API".into(),
                    body: OutgoingBody {
                        plain_text: "Please review".into(),
                        html: None,
                    },
                    attachments: Vec::new(),
                    reply_context: None,
                    forward_context: None,
                },
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let draft_id = draft["output"]["id"].as_str().unwrap();
    let actions = MailActionService::new(&storage, tools.clone(), context, scope, "api-test-v1")
        .await
        .unwrap();
    let app = router(Arc::new(
        AppState::new(storage).with_mail_foundation(tools, actions),
    ));

    let requested = app
        .clone()
        .oneshot(json_request(
            "/foundation/mail/send-approvals",
            json!({
                "accountId": "primary",
                "draftId": draft_id,
                "expectedRevision": 1,
                "idempotencyKey": "api-send-1",
                "sessionId": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(requested.status(), StatusCode::OK);
    let requested = read_json(requested).await;
    let approval_id = requested["approval"]["approval_id"].as_str().unwrap();

    let listed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/foundation/actions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(read_json(listed).await.as_array().unwrap().len(), 1);

    for _ in 0..2 {
        let resolved = app
            .clone()
            .oneshot(json_request(
                &format!("/foundation/actions/{approval_id}"),
                json!({"decision": "approve_once"}),
            ))
            .await
            .unwrap();
        assert_eq!(resolved.status(), StatusCode::OK);
        assert_eq!(read_json(resolved).await["action"]["status"], "succeeded");
    }
    assert_eq!(mail.provider_submission_count(), 1);
    assert_eq!(mail.logical_delivery_count(), 1);
}

#[tokio::test]
async fn trusted_mail_configuration_api_cruds_without_returning_secrets() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let metadata = SqliteCredentialMetadataStore::from_storage(&storage)
        .await
        .unwrap();
    let vault = Arc::new(CredentialVault::new_persistent(
        Arc::new(InMemorySecretStore::default()),
        metadata,
    ));
    let manager = Arc::new(ImapSmtpMailAccountManager::new(
        SqliteImapSmtpMailAccountStore::from_storage(&storage)
            .await
            .unwrap(),
        vault,
        Arc::new(ManagedImapSmtpMailConnector::new()),
    ));
    let app = router(Arc::new(
        AppState::new(storage.clone()).with_mail_account_manager(manager),
    ));
    let first_password = "first-credential-marker";
    let created = app
        .clone()
        .oneshot(json_request_with_method(
            Method::PUT,
            "/foundation/mail/account-configurations/primary",
            mail_configuration(first_password, "first@example.test"),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created_text = read_text(created).await;
    assert!(!created_text.contains(first_password));
    assert!(!created_text.contains("password"));
    let created: Value = serde_json::from_str(&created_text).unwrap();
    assert_eq!(created["id"], "primary");
    assert_eq!(created["credentialConfigured"], true);

    let second_password = "rotated-credential-marker";
    let updated = app
        .clone()
        .oneshot(json_request_with_method(
            Method::PUT,
            "/foundation/mail/account-configurations/primary",
            mail_configuration(second_password, "updated@example.test"),
        ))
        .await
        .unwrap();
    assert_eq!(updated.status(), StatusCode::OK);
    let updated_text = read_text(updated).await;
    assert!(!updated_text.contains(second_password));
    assert_eq!(
        serde_json::from_str::<Value>(&updated_text).unwrap()["username"],
        "updated@example.test"
    );

    for uri in [
        "/foundation/mail/account-configurations",
        "/foundation/mail/account-configurations/primary",
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let text = read_text(response).await;
        assert!(!text.contains(first_password));
        assert!(!text.contains(second_password));
        assert!(!text.contains("password"));
    }

    let deleted = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/foundation/mail/account-configurations/primary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(read_json(deleted).await["deleted"], true);
    let missing = app
        .oneshot(
            Request::builder()
                .uri("/foundation/mail/account-configurations/primary")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mail_configuration_api_rejects_insecure_remote_hosts_without_echoing_password() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let metadata = SqliteCredentialMetadataStore::from_storage(&storage)
        .await
        .unwrap();
    let manager = Arc::new(ImapSmtpMailAccountManager::new(
        SqliteImapSmtpMailAccountStore::from_storage(&storage)
            .await
            .unwrap(),
        Arc::new(CredentialVault::new_persistent(
            Arc::new(InMemorySecretStore::default()),
            metadata,
        )),
        Arc::new(ManagedImapSmtpMailConnector::new()),
    ));
    let app = router(Arc::new(
        AppState::new(storage).with_mail_account_manager(manager),
    ));
    let password = "rejected-credential-marker";
    let mut body = mail_configuration(password, "user@example.test");
    body["imapTls"] = json!("none");

    let response = app
        .oneshot(json_request_with_method(
            Method::PUT,
            "/foundation/mail/account-configurations/primary",
            body,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!read_text(response).await.contains(password));
}

fn mail_configuration(password: &str, username: &str) -> Value {
    json!({
        "displayName": "Primary Mail",
        "primaryName": "Local User",
        "primaryAddress": "user@example.test",
        "username": username,
        "password": password,
        "imapHost": "imap.example.test",
        "imapPort": 993,
        "imapTls": "implicit",
        "smtpHost": "smtp.example.test",
        "smtpPort": 587,
        "smtpTls": "start_tls",
        "archiveMailbox": "Archive",
        "sentMailbox": "Sent",
        "draftsMailbox": "Drafts",
        "trashMailbox": "Trash",
        "allowInsecureLocalhost": false
    })
}

fn json_request(uri: &str, body: Value) -> Request<Body> {
    json_request_with_method(Method::POST, uri, body)
}

fn json_request_with_method(method: Method, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn read_json(response: axum::response::Response) -> Value {
    serde_json::from_str(&read_text(response).await).unwrap()
}

async fn read_text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

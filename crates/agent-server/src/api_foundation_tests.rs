use super::*;
use agent_runtime::connector::ConnectorRuntime;
use agent_runtime::connector_tools::{ConnectorToolRuntime, EphemeralConnectorContextProvider};
use agent_runtime::credential::CredentialScope;
use agent_runtime::foundation_actions::MailActionService;
use agent_runtime::mail::{DraftContent, MailAccount, MailAddress, OutgoingBody};
use agent_runtime::mail_connector_transport::MailConnectorTransport;
use agent_runtime::mail_fake::FakeMailConnector;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
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

fn json_request(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn read_json(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

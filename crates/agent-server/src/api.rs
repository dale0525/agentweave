use agent_runtime::{events::RuntimeEvent, session::Message, storage::Storage};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    storage: Storage,
}

impl AppState {
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct UserMessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct UserMessageResponse {
    pub accepted: bool,
    pub user_message: Message,
    pub assistant_message: Message,
    pub events: Vec<RuntimeEvent>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug)]
enum ApiError {
    NotFound(&'static str),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message.to_string()),
            Self::Internal(error) => {
                tracing::error!(?error, "agent-server request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };

        (status, Json(ErrorResponse { error })).into_response()
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/sessions", post(create_session))
        .route("/sessions/{session_id}/messages", post(post_message))
        .layer(desktop_cors_layer())
        .with_state(state)
}

fn desktop_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("http://127.0.0.1:5173"),
            HeaderValue::from_static("http://localhost:5173"),
        ])
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE])
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, ApiError> {
    let title = request.title.unwrap_or_else(|| "New Session".to_string());
    let session = state
        .storage
        .create_session(&title)
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(CreateSessionResponse {
        id: session.id,
        title: session.title,
    }))
}

async fn post_message(
    Path(session_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(request): Json<UserMessageRequest>,
) -> Result<Json<UserMessageResponse>, ApiError> {
    let session_exists = state
        .storage
        .session_exists(&session_id)
        .await
        .map_err(ApiError::Internal)?;
    if !session_exists {
        return Err(ApiError::NotFound("session not found"));
    }

    let turn_id = uuid::Uuid::new_v4().to_string();
    let assistant_text = deterministic_assistant_reply(&request.content);
    let (user_message, assistant_message) = state
        .storage
        .append_turn(&session_id, &request.content, &assistant_text)
        .await
        .map_err(ApiError::Internal)?;

    let events = vec![
        RuntimeEvent::TurnStarted {
            turn_id: turn_id.clone(),
        },
        RuntimeEvent::AssistantTextDelta {
            text: assistant_text.clone(),
        },
        RuntimeEvent::AssistantMessageFinished {
            text: assistant_text,
        },
        RuntimeEvent::TurnFinished { turn_id },
    ];

    Ok(Json(UserMessageResponse {
        accepted: true,
        user_message,
        assistant_message,
        events,
    }))
}

fn deterministic_assistant_reply(content: &str) -> String {
    format!("MVP agent received: {content}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::storage::Storage;
    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage)));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_session_and_post_message_returns_runtime_events() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage.clone())));

        let create_response = app
            .clone()
            .oneshot(json_request(
                "/sessions",
                json!({ "title": "MVP Verification" }),
            ))
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);
        let created = read_json(create_response).await;
        let session_id = created["id"].as_str().unwrap();
        assert_eq!(created["title"], "MVP Verification");

        let message_response = app
            .oneshot(json_request(
                &format!("/sessions/{session_id}/messages"),
                json!({ "content": "Run the renderer smoke test" }),
            ))
            .await
            .unwrap();

        assert_eq!(message_response.status(), StatusCode::OK);
        let message = read_json(message_response).await;
        assert_eq!(message["accepted"], true);
        assert_eq!(
            message["assistant_message"]["content"],
            "MVP agent received: Run the renderer smoke test"
        );
        assert_eq!(message["events"][0]["type"], "turn_started");
        assert_eq!(message["events"][1]["type"], "assistant_text_delta");
        assert_eq!(
            message["events"][2],
            json!({
                "type": "assistant_message_finished",
                "text": "MVP agent received: Run the renderer smoke test"
            })
        );
        assert_eq!(message["events"][3]["type"], "turn_finished");

        let stored_messages = storage.list_messages(session_id).await.unwrap();
        assert_eq!(stored_messages.len(), 2);
        assert_eq!(stored_messages[0].role, "user");
        assert_eq!(stored_messages[0].content, "Run the renderer smoke test");
        assert_eq!(stored_messages[1].role, "assistant");
        assert_eq!(
            stored_messages[1].content,
            "MVP agent received: Run the renderer smoke test"
        );
    }

    #[tokio::test]
    async fn post_message_rejects_missing_session() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage)));

        let response = app
            .oneshot(json_request(
                "/sessions/missing-session/messages",
                json!({ "content": "hello" }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = read_json(response).await;
        assert_eq!(body["error"], "session not found");
    }

    #[test]
    fn internal_api_errors_return_500() {
        let response = ApiError::Internal(anyhow::anyhow!("storage unavailable")).into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn supports_vite_desktop_cors_preflight() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage)));

        for origin in ["http://127.0.0.1:5173", "http://localhost:5173"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("OPTIONS")
                        .uri("/sessions/session-1/messages")
                        .header(header::ORIGIN, origin)
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                origin
            );
            assert!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_METHODS]
                    .to_str()
                    .unwrap()
                    .contains("POST")
            );
            assert!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
                    .to_str()
                    .unwrap()
                    .contains("content-type")
            );
        }
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
}

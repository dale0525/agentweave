use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct AppState;

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
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/sessions", post(create_session))
        .route("/sessions/{session_id}/messages", post(post_message))
        .with_state(state)
}

async fn create_session(
    State(_state): State<Arc<AppState>>,
    Json(request): Json<CreateSessionRequest>,
) -> Json<CreateSessionResponse> {
    Json(CreateSessionResponse {
        id: uuid::Uuid::new_v4().to_string(),
        title: request.title.unwrap_or_else(|| "New Session".to_string()),
    })
}

async fn post_message(
    Path(_session_id): Path<String>,
    State(_state): State<Arc<AppState>>,
    Json(request): Json<UserMessageRequest>,
) -> Json<UserMessageResponse> {
    let _content = request.content;

    Json(UserMessageResponse { accepted: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let app = router(Arc::new(AppState));
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
}

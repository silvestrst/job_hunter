//! Localhost ingest server. Two routes: an open health check for the
//! extension's status dot, and a token-guarded POST that feeds
//! `core::ingest::ingest_payload` — the same function the stage-2 scraper
//! will call.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use job_hunter_core::db::Db;
use job_hunter_core::ingest::{ingest_payload, ScrapedPayload};
use serde_json::json;
use std::sync::{Arc, Mutex};

pub const TOKEN_HEADER: &str = "x-api-token";
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Db>>,
    pub token: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/jobs", post(save_job))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")}))
}

async fn save_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<ScrapedPayload>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let provided = headers
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !job_hunter_core::token::verify(&state.token, provided) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "missing or invalid X-Api-Token"})),
        );
    }

    let payload = match body {
        Ok(Json(payload)) => payload,
        Err(rejection) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": rejection.body_text()})),
            );
        }
    };

    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let db = db.lock().expect("db mutex poisoned");
        ingest_payload(&db, &payload)
    })
    .await;

    match result {
        Ok(Ok(saved)) => {
            let code = if saved.duplicate { StatusCode::OK } else { StatusCode::CREATED };
            (code, Json(json!({"id": saved.id, "duplicate": saved.duplicate})))
        }
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, Json(json!({"error": format!("{e:#}")}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            db: Arc::new(Mutex::new(Db::open_in_memory().unwrap())),
            token: "secret".into(),
        }
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn job_request(token: Option<&str>, payload: &str) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/jobs")
            .header("content-type", "application/json");
        if let Some(t) = token {
            builder = builder.header("x-api-token", t);
        }
        builder.body(Body::from(payload.to_string())).unwrap()
    }

    const PAYLOAD: &str = r#"{"url": "https://example.com/jobs/1?utm_source=x", "title": "Rust Dev", "text": "job text"}"#;

    #[tokio::test]
    async fn health_is_open() {
        let response = router(test_state())
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["status"], "ok");
    }

    #[tokio::test]
    async fn post_requires_token() {
        let app = router(test_state());
        let response = app.clone().oneshot(job_request(None, PAYLOAD)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app.oneshot(job_request(Some("wrong"), PAYLOAD)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn post_saves_then_dedupes() {
        let app = router(test_state());
        let response = app.clone().oneshot(job_request(Some("secret"), PAYLOAD)).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let first = body_json(response).await;
        assert_eq!(first["duplicate"], false);

        let same_job = r#"{"url": "https://EXAMPLE.com/jobs/1", "text": "seen again"}"#;
        let response = app.oneshot(job_request(Some("secret"), same_job)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let second = body_json(response).await;
        assert_eq!(second["duplicate"], true);
        assert_eq!(second["id"], first["id"]);
    }

    #[tokio::test]
    async fn post_rejects_bad_payload() {
        let app = router(test_state());
        let empty_text = r#"{"url": "https://example.com/x", "text": "  "}"#;
        let response = app.clone().oneshot(job_request(Some("secret"), empty_text)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app.oneshot(job_request(Some("secret"), "not json")).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

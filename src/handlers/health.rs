use crate::models::HealthResponse;
use axum::response::Json;

pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        service: "mailbuzz-api".to_string(),
        version: "0.1.0".to_string(),
    })
}

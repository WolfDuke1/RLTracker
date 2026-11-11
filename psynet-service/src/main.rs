use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

// --- State voor je API Token ---
#[derive(Clone)]
struct AppState {
    internal_api_token: String,
}

// --- Structs voor de JSON Response ---
#[derive(Serialize)]
struct RankData {
    playlist: String,
    mmr: i32,
    tier: i32,
    division: String,
    matches_played: i32,
    win_rate: i32,
    win_streak: i32,
    peak_mmr: i32,
    peak_tier: i32,
}

// --- Auth Middleware ---
fn check_auth(headers: &HeaderMap, expected_token: &str) -> Result<(), StatusCode> {
    if let Some(auth_header) = headers.get("Authorization") {
        if let Ok(auth_str) = auth_header.to_str() {
            if auth_str == format!("Bearer {}", expected_token) {
                return Ok(());
            }
        }
    }
    Err(StatusCode::UNAUTHORIZED)
}

// --- Health Check Endpoint ---
async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

// --- Ranks Endpoint ---
async fn get_ranks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((platform, id)): Path<(String, String)>,
) -> impl IntoResponse {
    // 1. Check Auth
    if let Err(status) = check_auth(&headers, &state.internal_api_token) {
        return (status, "Unauthorized").into_response();
    }

    println!("Fetching ranks for platform: {}, id: {}", platform, id);

    // 2. HIER KOMT JOUW PSYNET LOGICA
    // In een echt project roep je hier je PsyNet crate/API aan met de `platform` en `id`.
    // Voor nu sturen we een voorbeeld response terug om te testen of de connectie werkt.
    
    /* 
    let ranks = fetch_from_psynet(&platform, &id).await;
    if ranks.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Psynet error").into_response();
    }
    */

    // Tijdelijke mock data (vervang dit met echte data)
    let mock_data = vec![
        RankData {
            playlist: "11".to_string(),
            mmr: 1200,
            tier: 16,
            division: "II".to_string(),
            matches_played: 150,
            win_rate: 55,
            win_streak: 2,
            peak_mmr: 1250,
            peak_tier: 17,
        }
    ];

    (StatusCode::OK, Json(mock_data)).into_response()
}

// --- Main Function ---
#[tokio::main]
async fn main() {
    // Laad environment variables (.env bestand)
    dotenvy::dotenv().ok();

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let internal_api_token = std::env::var("INTERNAL_API_TOKEN")
        .unwrap_or_else(|_| "default_token".to_string());

    let state = Arc::new(AppState {
        internal_api_token,
    });

    // CORS instellen zodat je frontend (Cloudflare) verbinding mag maken
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Router opzetten
    let app = Router::new()
        .route("/health", get(health_check)) // Lost je Render Health Check op!
        .route("/v1/ranks/:platform/:id", get(get_ranks)) // De route die je frontend verwacht
        .layer(cors)
        .with_state(state);

    // Server starten op 0.0.0.0
    let addr = SocketAddr::from(([0, 0, 0, 0], port.parse().unwrap()));
    println!("PsyNet service on :{}", port);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

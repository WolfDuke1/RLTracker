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

// Import the PsyNet client from your crate
// Note: You might need to adjust these imports based on your exact psynet version.
use psynet::PsyNetClient; 

// --- State containing the Token and the PsyNet Client ---
#[derive(Clone)]
struct AppState {
    internal_api_token: String,
    psy_client: PsyNetClient,
}

// --- Struct matching what your index.html expects ---
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

// --- Auth Check ---
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

    println!("Fetching real PsyNet data for: {} / {}", platform, id);

    // 2. Fetch data from PsyNet
    // The exact method name might be `get_player_skills` or `get_skill` depending on the crate version.
    match state.psy_client.get_player_skills(&platform, &id).await {
        Ok(player_skills) => {
            let mut response_data = Vec::new();

            // 3. Map the PsyNet data to our frontend structure
            for skill in player_skills {
                // We only want Ranked 1v1 (10), 2v2 (11), and 3v3 (13)
                if skill.playlist_id == 10 || skill.playlist_id == 11 || skill.playlist_id == 13 {
                    response_data.push(RankData {
                        playlist: skill.playlist_id.to_string(),
                        mmr: skill.mmr,
                        tier: skill.tier,
                        division: skill.division.to_string(),
                        matches_played: skill.matches_played,
                        // Calculate win rate if not provided directly
                        win_rate: if skill.matches_played > 0 {
                            ((skill.wins as f32 / skill.matches_played as f32) * 100.0) as i32
                        } else {
                            0
                        },
                        win_streak: skill.win_streak,
                        peak_mmr: skill.peak_mmr,
                        peak_tier: skill.peak_tier,
                    });
                }
            }

            (StatusCode::OK, Json(response_data)).into_response()
        }
        Err(e) => {
            println!("PsyNet Error: {:?}", e);
            // If the player doesn't exist or the API fails
            (StatusCode::NOT_FOUND, "Player not found or PsyNet API error").into_response()
        }
    }
}

// --- Main Function ---
#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let internal_api_token = std::env::var("INTERNAL_API_TOKEN")
        .unwrap_or_else(|_| "default_token".to_string());

    // Initialize the PsyNet Client
    // This client automatically reads EPIC_ACCOUNT_ID, EPIC_DEVICE_ID, etc. from your env variables.
    println!("Initializing PsyNet Client...");
    let psy_client = PsyNetClient::new().await.expect("Failed to initialize PsyNet Client. Check your Epic credentials.");
    println!("PsyNet Client initialized successfully!");

    let state = Arc::new(AppState {
        internal_api_token,
        psy_client,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/v1/ranks/:platform/:id", get(get_ranks))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port.parse().unwrap()));
    println!("PsyNet service on :{}", port);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

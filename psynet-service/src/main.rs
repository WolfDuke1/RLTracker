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

use psynet::{PlayerId, PlayerPlatform, PsyNetClient, PsyNetRpc};

#[derive(Clone)]
struct AppState {
    internal_api_token: String,
    psy_client: PsyNetClient,
    epic_account_id: String,
    epic_access_token: String,
}

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

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

async fn get_ranks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((platform, id)): Path<(String, String)>,
) -> impl IntoResponse {
    // 1. Verify your frontend's INTERNAL_API_TOKEN
    if let Err(status) = check_auth(&headers, &state.internal_api_token) {
        return (status, "Unauthorized").into_response();
    }

    println!("Fetching PsyNet data for: {} / {}", platform, id);

    // 2. Map the platform string to the PlayerPlatform enum
    let player_platform = match platform.to_lowercase().as_str() {
        "epic" => PlayerPlatform::Epic,
        "steam" => PlayerPlatform::Steam,
        "psn" => PlayerPlatform::Psn,
        "xbl" => PlayerPlatform::Xbl,
        "switch" => PlayerPlatform::Switch,
        _ => return (StatusCode::BAD_REQUEST, "Invalid platform").into_response(),
    };

    let player_id = PlayerId::new(player_platform, id);

    // 3. Authenticate to create an RPC session
    let rpc = match state.psy_client.auth_player(&state.epic_account_id, &state.epic_access_token).await {
        Ok(rpc) => rpc,
        Err(e) => {
            println!("PsyNet Authentication failed: {:?}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "PsyNet authentication failed").into_response();
        }
    };

    // 4. Fetch the skills and map the data
    match rpc.get_players_skills(vec![player_id]).await {
        Ok(players_with_skills) => {
            let mut response_data = Vec::new();

            if let Some(player) = players_with_skills.first() {
                for skill in &player.skills {
                    // Filter for Ranked 1v1 (10), 2v2 (11), 3v3 (13)
                    if skill.playlist == 10 || skill.playlist == 11 || skill.playlist == 13 {
                        response_data.push(RankData {
                            playlist: skill.playlist.to_string(),
                            mmr: skill.mmr.round() as i32, // Convert f64 to i32
                            tier: skill.tier as i32,       // Convert i64 to i32
                            division: skill.division.to_string(),
                            matches_played: skill.matches_played as i32,
                            // Note: PsyNet does not provide total wins directly, so win_rate is calculated as 0.
                            // Peak MMR and Peak Tier are not provided by this endpoint; we use current values.
                            win_rate: 0, 
                            win_streak: skill.win_streak as i32,
                            peak_mmr: skill.mmr.round() as i32,
                            peak_tier: skill.tier as i32,
                        });
                    }
                }
            }

            (StatusCode::OK, Json(response_data)).into_response()
        }
        Err(e) => {
            println!("PsyNet Error: {:?}", e);
            (StatusCode::NOT_FOUND, "Player not found or PsyNet API error").into_response()
        }
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let internal_api_token = std::env::var("INTERNAL_API_TOKEN")
        .unwrap_or_else(|_| "default_token".to_string());
    let epic_account_id = std::env::var("EPIC_ACCOUNT_ID")
        .expect("EPIC_ACCOUNT_ID must be set");
    let epic_access_token = std::env::var("EPIC_ACCESS_TOKEN")
        .expect("EPIC_ACCESS_TOKEN must be set");

    println!("Initializing PsyNet Client...");
    
    // PsyNetClient::new() is synchronous
    let psy_client = PsyNetClient::new();
    
    println!("PsyNet Client initialized successfully!");

    let state = Arc::new(AppState {
        internal_api_token,
        psy_client,
        epic_account_id,
        epic_access_token,
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

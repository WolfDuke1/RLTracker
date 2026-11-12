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

use psynet::{PlayerId, PlayerPlatform, PsyNetClient};

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

// --- Epic Games Token Exchange ---
// FIXED: Read the response as text first, so we can both parse it AND use it for errors.
async fn get_epic_access_token(account_id: &str, device_id: &str, secret: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    
    let client_id = "3446cd72694c4a4485d81b77adbb2141";
    let client_secret = "98f2b4898a614a8c82e78b59f6a9119e";
    
    let params = [
        ("grant_type", "device_auth"),
        ("account_id", account_id),
        ("device_id", device_id),
        ("secret", secret),
    ];

    let res = client.post("https://account-public-service-prod03.ol.epicgames.com/account/api/oauth/token")
        .basic_auth(client_id, Some(client_secret))
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    let status = res.status();
    let text = res.text().await.unwrap_or_default();

    if status.is_success() {
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("JSON error: {}", e))?;
        if let Some(token) = json.get("access_token").and_then(|t| t.as_str()) {
            return Ok(token.to_string());
        }
    }
    
    Err(format!("Failed to get token. Status: {}. Body: {}", status, text))
}

// --- Auth Check for your Frontend ---
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
    if let Err(status) = check_auth(&headers, &state.internal_api_token) {
        return (status, "Unauthorized").into_response();
    }

    println!("Fetching PsyNet data for: {} / {}", platform, id);

    // FIXED: Removed Psn and Xbl because the psynet crate does not support them.
    let player_platform = match platform.to_lowercase().as_str() {
        "epic" => PlayerPlatform::Epic,
        "steam" => PlayerPlatform::Steam,
        _ => return (StatusCode::BAD_REQUEST, "Invalid platform. Supported: epic, steam").into_response(),
    };

    let player_id = PlayerId::new(player_platform, &id);

    let rpc = match state.psy_client.auth_player(&state.epic_account_id, &state.epic_access_token).await {
        Ok(rpc) => rpc,
        Err(e) => {
            println!("PsyNet Authentication failed: {:?}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "PsyNet authentication failed").into_response();
        }
    };

    match rpc.get_players_skills(vec![player_id]).await {
        Ok(players_with_skills) => {
            let mut response_data = Vec::new();

            if let Some(player) = players_with_skills.first() {
                for skill in &player.skills {
                    if skill.playlist == 10 || skill.playlist == 11 || skill.playlist == 13 {
                        response_data.push(RankData {
                            playlist: skill.playlist.to_string(),
                            mmr: skill.mmr.round() as i32, 
                            tier: skill.tier as i32,       
                            division: skill.division.to_string(),
                            matches_played: skill.matches_played as i32,
                            win_rate: 0, // Not provided by this endpoint
                            win_streak: skill.win_streak as i32,
                            peak_mmr: skill.mmr.round() as i32, // Using current MMR as peak
                            peak_tier: skill.tier as i32,       // Using current tier as peak
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

// --- Main Function ---
#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let internal_api_token = std::env::var("INTERNAL_API_TOKEN").unwrap_or_else(|_| "default_token".to_string());
    
    let epic_account_id = std::env::var("EPIC_ACCOUNT_ID").expect("EPIC_ACCOUNT_ID must be set");
    let epic_device_id = std::env::var("EPIC_DEVICE_ID").expect("EPIC_DEVICE_ID must be set");
    let epic_secret = std::env::var("EPIC_DEVICE_SECRET").expect("EPIC_DEVICE_SECRET must be set");

    println!("Exchanging Device Auth for Access Token...");
    let access_token = match get_epic_access_token(&epic_account_id, &epic_device_id, &epic_secret).await {
        Ok(token) => token,
        Err(e) => {
            eprintln!("CRITICAL ERROR: {}", e);
            std::process::exit(1);
        }
    };
    println!("Successfully obtained Epic Access Token!");

    println!("Initializing PsyNet Client...");
    let psy_client = PsyNetClient::new();
    println!("PsyNet Client initialized successfully!");

    let state = Arc::new(AppState {
        internal_api_token,
        psy_client,
        epic_account_id,
        epic_access_token: access_token,
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

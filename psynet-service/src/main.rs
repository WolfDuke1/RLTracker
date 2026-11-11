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

// Import the correct types from the psynet crate
use psynet::{PsyNetClient, PsyNetRpc, PlayerId};

#[derive(Clone)]
struct AppState {
    internal_api_token: String,
    psy_client: PsyNetClient,
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
    if let Err(status) = check_auth(&headers, &state.internal_api_token) {
        return (status, "Unauthorized").into_response();
    }

    println!("Fetching real PsyNet data for: {} / {}", platform, id);

    // Create the PlayerId required by the API
    let player_id = PlayerId {
        platform: platform,
        id: id,
    };

    // Use the correct RPC method: get_players_skills
    match state.psy_client.get_players_skills(vec![player_id]).await {
        Ok(players_with_skills) => {
            let mut response_data = Vec::new();

            if let Some(player) = players_with_skills.first() {
                for skill in &player.skills {
                    if skill.playlist_id == 10 || skill.playlist_id == 11 || skill.playlist_id == 13 {
                        response_data.push(RankData {
                            playlist: skill.playlist_id.to_string(),
                            mmr: skill.mmr,
                            tier: skill.tier,
                            division: skill.division.to_string(),
                            matches_played: skill.matches_played,
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

    println!("Initializing PsyNet Client...");
    
    // Initialize the client (this reads your Epic credentials from the environment)
    let psy_client = PsyNetClient::new().await.expect("Failed to initialize PsyNet. Check your Epic environment variables.");
    
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

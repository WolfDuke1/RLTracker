use axum::{extract::Path, routing::get, Json, Router};
use psynet::{PlayerId, PlayerPlatform, PsyNetClient};
use serde::Serialize;
use tower_http::cors::CorsLayer;

#[derive(Serialize)]
struct RankResponse {
    playlist: String,
    mmr: u32,
    tier: u32,
    division: u32,
    win_streak: u32,
    matches_played: u32,
}

async fn get_ranks(Path((platform, id)): Path<(String, String)>) -> Json<Vec<RankResponse>> {
    let client = PsyNetClient::new();

    // Authenticate with a dedicated Rocket League account
    let account_id = std::env::var("PSYNET_ACCOUNT_ID").unwrap();
    let access_token = std::env::var("PSYNET_ACCESS_TOKEN").unwrap();
    
    // This connects to PsyNet and manages the WebSocket session automatically
    let rpc = client.auth_player(&account_id, &access_token).await.unwrap();

    let player = PlayerId::new(
        match platform.as_str() {
            "epic" => PlayerPlatform::Epic,
            "steam" => PlayerPlatform::Steam,
            "psn" => PlayerPlatform::Psn,
            "xbl" => PlayerPlatform::Xbox,
            _ => PlayerPlatform::Epic,
        },
        &id,
    );

    let skills = rpc.get_players_skills(&[player]).await.unwrap();

    let response = skills
        .into_iter()
        .flat_map(|s| {
            s.skills.into_iter().map(|skill| RankResponse {
                playlist: skill.playlist.to_string(),
                mmr: skill.mmr as u32,
                tier: skill.tier,
                division: skill.division,
                win_streak: skill.win_streak,
                matches_played: skill.matches_played,
            })
        })
        .collect();

    Json(response)
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let app = Router::new()
        .route("/api/ranks/:platform/:id", get(get_ranks))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    println!("PsyNet service on :8080");
    axum::serve(listener, app).await.unwrap();
}

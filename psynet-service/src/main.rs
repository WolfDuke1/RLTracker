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

    let rpc = client
        .auth_player(&account_id, &access_token)
        .await
        .unwrap();

    let player = PlayerId::new(
        match platform.as_str() {
            "epic" => PlayerPlatform::Epic,
            "steam" => PlayerPlatform::Steam,
            "psn" => PlayerPlatform::PlayStation, // Fixed: Use PlayStation, not Psn
            "xbl" => PlayerPlatform::Xbox,
            "switch" => PlayerPlatform::Nintendo,
            _ => PlayerPlatform::Epic,
        },
        &id,
    );

    // Fixed: Pass a Vec, not a slice reference
    let skills = rpc.get_players_skills(vec![player]).await.unwrap();

    let response = skills
        .into_iter()
        .flat_map(|s| {
            s.skills.into_iter().map(|skill| RankResponse {
                playlist: skill.playlist.to_string(),
                mmr: skill.mmr as u32,
                // Fixed: PsyNet returns i64, so we cast to u32
                tier: skill.tier as u32,
                division: skill.division as u32,
                win_streak: skill.win_streak as u32,
                matches_played: skill.matches_played as u32,
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

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .unwrap();
    println!("PsyNet service on :8080");
    axum::serve(listener, app).await.unwrap();
}

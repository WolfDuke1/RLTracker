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
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};

use psynet::{PlayerId, PlayerPlatform, PsyNetClient};

struct TokenCache {
    access_token: String,
    expires_at: Instant,
}

struct AppState {
    internal_api_token: String,
    psy_client: PsyNetClient,
    http_client: reqwest::Client,
    epic_account_id: String,
    epic_device_id: String,
    epic_device_secret: String,
    token_cache: RwLock<TokenCache>,
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
    account_id: String,
}

// --- Epic Games Token Exchange ---
async fn fetch_epic_access_token(
    client: &reqwest::Client,
    account_id: &str,
    device_id: &str,
    secret: &str,
) -> Result<String, String> {
    let client_id = "3f69e56c7649492c8cc29f1af08a8a12";
    let client_secret = "b51ee9cb12234f50a69efa67ef53812e";

    let params = [
        ("grant_type", "device_auth"),
        ("account_id", account_id),
        ("device_id", device_id),
        ("secret", secret),
    ];

    let res = client
        .post("https://account-public-service-prod03.ol.epicgames.com/account/api/oauth/token")
        .basic_auth(client_id, Some(client_secret))
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    let status = res.status();
    let text = res.text().await.unwrap_or_default();

    if status.is_success() {
        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("JSON error: {}", e))?;
        if let Some(token) = json.get("access_token").and_then(|t| t.as_str()) {
            return Ok(token.to_string());
        }
    }

    Err(format!("Failed to get token. Status: {}. Body: {}", status, text))
}

async fn get_valid_epic_token(state: &AppState, force_refresh: bool) -> Result<String, String> {
    if !force_refresh {
        let guard = state.token_cache.read().await;
        if Instant::now() < guard.expires_at {
            return Ok(guard.access_token.clone());
        }
    }

    let mut guard = state.token_cache.write().await;
    if !force_refresh && Instant::now() < guard.expires_at {
        return Ok(guard.access_token.clone());
    }

    let fresh = fetch_epic_access_token(
        &state.http_client,
        &state.epic_account_id,
        &state.epic_device_id,
        &state.epic_device_secret,
    )
    .await?;

    guard.access_token = fresh.clone();
    guard.expires_at = Instant::now() + Duration::from_secs(3 * 3600);
    Ok(fresh)
}

// --- Resolve Epic Display Name -> 32-char Epic Account ID ---
async fn resolve_epic_account_id(
    state: &AppState,
    token: &str,
    input: &str,
) -> Result<String, (StatusCode, String)> {
    let trimmed = input.trim();
    if trimmed.len() == 32 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(trimmed.to_lowercase());
    }

    let mut last_diag = String::new();

    // 1. Epic User Search Service (requires ?prefix=...&platform=...)
    // Checks primary Epic names first, then linked external auth names (PSN/Xbox/Steam)
    for platform_hint in ["epic", "psn", "xbl", "steam"] {
        if let Ok(mut search_url) = reqwest::Url::parse(&format!(
            "https://user-search-service-prod.ol.epicgames.com/api/v1/search/{}",
            state.epic_account_id
        )) {
            search_url
                .query_pairs_mut()
                .append_pair("prefix", trimmed)
                .append_pair("platform", platform_hint);

            if let Ok(res) = state
                .http_client
                .get(search_url)
                .bearer_auth(token)
                .send()
                .await
            {
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                if status.is_success() {
                    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
                        for entry in &arr {
                            let exact = entry
                                .get("matches")
                                .and_then(|m| m.as_array())
                                .map(|list| {
                                    list.iter().any(|item| {
                                        item.get("value")
                                            .and_then(|v| v.as_str())
                                            .map(|val| val.eq_ignore_ascii_case(trimmed))
                                            .unwrap_or(false)
                                    })
                                })
                                .unwrap_or(false);

                            if exact {
                                if let Some(acc_id) =
                                    entry.get("accountId").and_then(|v| v.as_str())
                                {
                                    return Ok(acc_id.to_string());
                                }
                            }
                        }
                        if platform_hint == "epic" {
                            if let Some(acc_id) = arr
                                .first()
                                .and_then(|e| e.get("accountId"))
                                .and_then(|v| v.as_str())
                            {
                                return Ok(acc_id.to_string());
                            }
                        }
                    }
                    last_diag = format!("search[{}](empty)", platform_hint);
                } else {
                    let snippet: String = text.chars().take(100).collect();
                    last_diag = format!("search[{}]({}: {})", platform_hint, status, snippet);
                }
            }
        }
    }

    // 2. Fallback: Account Public Service exact displayName path
    for host in [
        "https://account-public-service-prod.ol.epicgames.com/account/api/public/account/displayName/",
        "https://account-public-service-prod03.ol.epicgames.com/account/api/public/account/displayName/",
    ] {
        if let Ok(mut url) = reqwest::Url::parse(host) {
            if let Ok(mut segs) = url.path_segments_mut() {
                segs.push(trimmed);
            }
            if let Ok(res) = state.http_client.get(url).bearer_auth(token).send().await {
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                if status.is_success() {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(acc_id) = json.get("id").and_then(|v| v.as_str()) {
                            return Ok(acc_id.to_string());
                        }
                    }
                }
                last_diag = format!("{} | path({})", last_diag, status);
            }
        }
    }

    Err((
        StatusCode::NOT_FOUND,
        format!("Epic player '{}' not found ({})", trimmed, last_diag),
    ))
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

async fn query_psynet_ranks(
    state: &AppState,
    platform: &str,
    id: &str,
) -> axum::response::Response {
    println!("Fetching PsyNet data for: {} / {}", platform, id);

    let player_platform = match platform.to_lowercase().as_str() {
        "epic" => PlayerPlatform::Epic,
        "steam" => PlayerPlatform::Steam,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Invalid platform. Supported by PsyNet: epic, steam",
            )
                .into_response()
        }
    };

    let mut token = match get_valid_epic_token(state, false).await {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    let resolved_id = if matches!(player_platform, PlayerPlatform::Epic) {
        match resolve_epic_account_id(state, &token, id).await {
            Ok(acc_id) => acc_id,
            Err((code, msg)) => return (code, msg).into_response(),
        }
    } else {
        id.trim().to_string()
    };

    let mut rpc_res = state
        .psy_client
        .auth_player(&state.epic_account_id, &token)
        .await;

    if rpc_res.is_err() {
        if let Ok(refreshed) = get_valid_epic_token(state, true).await {
            token = refreshed;
            rpc_res = state
                .psy_client
                .auth_player(&state.epic_account_id, &token)
                .await;
        }
    }

    let rpc = match rpc_res {
        Ok(rpc) => rpc,
        Err(e) => {
            eprintln!("PsyNet Auth Error: {:#}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("PsyNet auth failed: {}", e),
            )
                .into_response();
        }
    };

    let player_id = PlayerId::new(player_platform, &resolved_id);

    match rpc.get_players_skills(vec![player_id]).await {
        Ok(players_with_skills) => {
            let mut response_data = Vec::new();

            if let Some(player) = players_with_skills.first() {
                for skill in &player.skills {
                    let mmr_rounded = skill.mmr.round() as i32;
                    response_data.push(RankData {
                        playlist: skill.playlist.to_string(),
                        mmr: mmr_rounded,
                        tier: skill.tier as i32,
                        division: skill.division.to_string(),
                        matches_played: skill.matches_played as i32,
                        win_rate: 0,
                        win_streak: skill.win_streak as i32,
                        peak_mmr: mmr_rounded,
                        peak_tier: skill.tier as i32,
                        account_id: resolved_id.clone(),
                    });
                }
            }

            (StatusCode::OK, Json(response_data)).into_response()
        }
        Err(e) => {
            println!("PsyNet Error: {:?}", e);
            (
                StatusCode::NOT_FOUND,
                format!("Player '{}' not found on PsyNet", id),
            )
                .into_response()
        }
    }
}

async fn get_ranks_v1(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((platform, id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(status) = check_auth(&headers, &state.internal_api_token) {
        return (status, "Unauthorized").into_response();
    }
    query_psynet_ranks(&state, &platform, &id).await
}

async fn get_ranks_public(
    State(state): State<Arc<AppState>>,
    Path((platform, id)): Path<(String, String)>,
) -> impl IntoResponse {
    query_psynet_ranks(&state, &platform, &id).await
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .trim()
        .to_string();
    let internal_api_token = std::env::var("INTERNAL_API_TOKEN")
        .unwrap_or_else(|_| "default_token".to_string())
        .trim()
        .to_string();

    let epic_account_id = std::env::var("EPIC_ACCOUNT_ID")
        .expect("EPIC_ACCOUNT_ID must be set")
        .trim()
        .to_string();
    let epic_device_id = std::env::var("EPIC_DEVICE_ID")
        .expect("EPIC_DEVICE_ID must be set")
        .trim()
        .to_string();
    let epic_device_secret = std::env::var("EPIC_DEVICE_SECRET")
        .expect("EPIC_DEVICE_SECRET must be set")
        .trim()
        .to_string();

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();

    println!("Exchanging Device Auth for Access Token...");
    let access_token = match fetch_epic_access_token(
        &http_client,
        &epic_account_id,
        &epic_device_id,
        &epic_device_secret,
    )
    .await
    {
        Ok(token) => token,
        Err(e) => {
            eprintln!("CRITICAL ERROR: {}", e);
            std::process::exit(1);
        }
    };
    println!("Successfully obtained Epic Access Token!");

    let psy_client = PsyNetClient::new();

    let state = Arc::new(AppState {
        internal_api_token,
        psy_client,
        http_client,
        epic_account_id,
        epic_device_id,
        epic_device_secret,
        token_cache: RwLock::new(TokenCache {
            access_token,
            expires_at: Instant::now() + Duration::from_secs(3 * 3600),
        }),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/v1/ranks/:platform/:id", get(get_ranks_v1))
        .route("/api/ranks/:platform/:id", get(get_ranks_public))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port.parse().unwrap()));
    println!("PsyNet service on :{}", port);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

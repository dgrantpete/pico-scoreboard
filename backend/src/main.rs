use axum::{routing::get, Router};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

mod auth;
mod config;
mod error;
mod espn;
mod game;

use config::AppConfig;
use espn::EspnClient;

/// Shared application state
pub struct AppState {
    pub espn_client: EspnClient,
    pub config: AppConfig,
}

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Load configuration
    let config = AppConfig::load();
    let bind_address = config.bind_address();

    // Create ESPN client with config
    let espn_client = EspnClient::new(&config.espn);

    // Create shared application state
    let app_state = Arc::new(AppState {
        espn_client,
        config,
    });

    // Build CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build router
    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/api/game/{event_id}", get(game::get_game))
        .layer(cors)
        .with_state(app_state);

    // Run server
    let listener = tokio::net::TcpListener::bind(&bind_address).await.unwrap();
    tracing::info!("Server running on http://{}", bind_address);
    axum::serve(listener, app).await.unwrap();
}

async fn root() -> &'static str {
    "Pico Scoreboard API"
}

async fn health() -> &'static str {
    "OK"
}

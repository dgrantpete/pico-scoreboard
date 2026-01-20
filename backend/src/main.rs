use axum::{routing::get, Router};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod auth;
mod config;
mod error;
mod espn;
mod game;

use config::AppConfig;
use espn::EspnClient;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Pico Scoreboard API",
        description = "API for fetching NFL game data from ESPN, optimized for Pi Pico displays",
        version = "1.0.0",
        contact(name = "Pico Scoreboard"),
    ),
    paths(game::handler::get_game, game::handler::get_all_games),
    components(schemas(
        game::types::GameResponse,
        game::types::PregameGame,
        game::types::LiveGame,
        game::types::FinalGame,
        game::types::Team,
        game::types::TeamWithScore,
        game::types::Color,
        game::types::Weather,
        game::types::Quarter,
        game::types::Situation,
        game::types::Down,
        game::types::Possession,
        game::types::FinalStatus,
        game::types::Winner,
        error::ErrorResponse,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "games", description = "Game data endpoints")
    )
)]
struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "api_key",
                utoipa::openapi::security::SecurityScheme::ApiKey(
                    utoipa::openapi::security::ApiKey::Header(
                        utoipa::openapi::security::ApiKeyValue::new("X-Api-Key"),
                    ),
                ),
            );
        }
    }
}

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
        .merge(SwaggerUi::new("/docs").url("/docs/openapi.json", ApiDoc::openapi()))
        .route("/", get(root))
        .route("/health", get(health))
        .route("/api/games", get(game::get_all_games))
        .route("/api/games/{event_id}", get(game::get_game))
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

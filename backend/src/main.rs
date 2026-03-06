use axum::{routing::get, Router};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

mod auth;
mod basketball;
mod clock;
mod config;
mod error;
mod espn;
mod football;
mod mock;
mod shared;
mod sport;
mod team;

use config::AppConfig;
use espn::EspnClient;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Pico Scoreboard API",
        description = "Multi-sport API for fetching game data from ESPN (NFL, NCAAF, NBA, NCAAB), optimized for Pi Pico displays",
        version = "2.0.0",
        contact(name = "Pico Scoreboard"),
    ),
    paths(
        clock::time,
        football::handler::get_all_games,
        football::handler::get_game,
        basketball::handler::get_all_games,
        basketball::handler::get_game,
        team::handler::get_football_team_logo,
        team::handler::get_basketball_team_logo,
        mock::handler::list_mock_games,
        mock::handler::get_mock_game,
        mock::handler::create_mock_game,
        mock::handler::delete_mock_game,
    ),
    components(schemas(
        football::types::FootballGameResponse,
        football::types::FootballPregame,
        football::types::FootballLive,
        football::types::FootballFinal,
        football::types::FootballTeamScore,
        football::types::FootballPeriod,
        football::types::Situation,
        football::types::Down,
        football::types::Possession,
        football::types::LastPlay,
        football::types::PlayType,
        basketball::types::BasketballGameResponse,
        basketball::types::BasketballPregame,
        basketball::types::BasketballLive,
        basketball::types::BasketballFinal,
        basketball::types::BasketballTeamScore,
        basketball::types::BasketballGameDetail,
        basketball::types::BasketballLiveDetail,
        basketball::types::BasketballFinalDetail,
        basketball::types::BasketballTeamScoreDetail,
        basketball::types::BasketballPeriod,
        shared::types::Team,
        shared::types::Color,
        shared::types::Weather,
        shared::types::FinalStatus,
        shared::types::Winner,
        mock::simulation::CreateGameRequest,
        mock::simulation::CreatePregameOptions,
        mock::simulation::CreateLiveOptions,
        mock::simulation::CreateFinalOptions,
        clock::TimeResponse,
        error::ErrorResponse,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "football", description = "Football game data and team logo endpoints (NFL, NCAAF)"),
        (name = "basketball", description = "Basketball game data and team logo endpoints (NBA, NCAAB)"),
        (name = "mock", description = "Mock data endpoints for testing"),
        (name = "clock", description = "Time and timezone endpoint")
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
                        utoipa::openapi::security::ApiKeyValue::with_description(
                            "X-Api-Key",
                            "API key for authentication. When no key is configured on the server, authentication is disabled and this header is ignored.",
                        ),
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
    pub game_repository: mock::GameRepository,
    pub geoip_reader: Option<maxminddb::Reader<memmap2::Mmap>>,
}

#[tokio::main]
async fn main() {
    // Initialize tracing with environment filter
    // Supports RUST_LOG patterns like:
    //   - "info" (default)
    //   - "info,espn::deserialize=debug" (show raw JSON on errors)
    //   - "debug" (verbose everything)
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // Use JSON format for production (Fly.io), human-readable for local dev
    let use_json = std::env::var("LOG_FORMAT")
        .map(|v| v == "json")
        .unwrap_or(false);

    if use_json {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer())
            .init();
    }

    // Load configuration
    let config = AppConfig::load();

    if config.api_key.is_none() {
        tracing::warn!(
            "No API key configured - authentication is disabled. \
             Set APP_API_KEY for production use."
        );
    } else {
        tracing::info!("API key authentication is enabled");
    }

    let bind_address = config.bind_address();

    // Create ESPN client with config
    let espn_client = EspnClient::new(&config.espn);

    // Create game repository for mock simulations
    let game_repository = mock::GameRepository::new();

    // Load GeoIP database (optional — gracefully degrades if absent)
    let geoip_reader = match maxminddb::Reader::open_mmap(&config.geoip.mmdb_path) {
        Ok(reader) => {
            tracing::info!(path = %config.geoip.mmdb_path, "GeoIP database loaded");
            Some(reader)
        }
        Err(e) => {
            tracing::warn!(
                path = %config.geoip.mmdb_path,
                error = %e,
                "GeoIP database not available — /time will not include utc_offset"
            );
            None
        }
    };

    // Create shared application state
    let app_state = Arc::new(AppState {
        espn_client,
        config,
        game_repository,
        geoip_reader,
    });

    // Build CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build router
    let app = Router::new()
        .merge(Scalar::with_url("/", ApiDoc::openapi()))
        .route("/health", get(health))
        .route("/time", get(clock::time))
        // Football endpoints
        .route("/api/football/{league}/games", get(football::handler::get_all_games))
        .route("/api/football/{league}/games/{event_id}", get(football::handler::get_game))
        .route("/api/football/{league}/{team_id}/logo", get(team::get_football_team_logo))
        // Basketball endpoints
        .route("/api/basketball/{league}/games", get(basketball::handler::get_all_games))
        .route("/api/basketball/{league}/games/{event_id}", get(basketball::handler::get_game))
        .route("/api/basketball/{league}/{team_id}/logo", get(team::get_basketball_team_logo))
        // Mock endpoints (unchanged, NFL-only)
        .route(
            "/api/mock/games",
            get(mock::list_mock_games).post(mock::create_mock_game),
        )
        .route(
            "/api/mock/games/{id}",
            get(mock::get_mock_game).delete(mock::delete_mock_game),
        )
        .layer(cors)
        .with_state(app_state);

    // Run server
    let listener = tokio::net::TcpListener::bind(&bind_address).await.unwrap();
    tracing::info!("Server running on http://{}", bind_address);
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> &'static str {
    "OK"
}


use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod auth;
mod basketball;
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
        football::handler::get_all_games,
        football::handler::get_game,
        basketball::handler::get_all_games,
        basketball::handler::get_game,
        team::handler::get_team_logo,
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
        error::ErrorResponse,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "football", description = "Football game data endpoints (NFL, NCAAF)"),
        (name = "basketball", description = "Basketball game data endpoints (NBA, NCAAB)"),
        (name = "teams", description = "Team data endpoints"),
        (name = "mock", description = "Mock data endpoints for testing")
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
    pub game_repository: mock::GameRepository,
}

// -- League dispatch handlers --

/// Dispatch GET /api/{league}/games to the appropriate sport handler
async fn league_all_games(
    api_key: crate::auth::ApiKey,
    state: State<Arc<AppState>>,
    Path(league): Path<String>,
) -> Result<Response, crate::error::AppError> {
    let sport_league = crate::sport::SportLeague::from_league(&league)?;
    match sport_league {
        crate::sport::SportLeague::Nfl | crate::sport::SportLeague::Ncaaf => {
            football::handler::get_all_games(api_key, state, Path(league))
                .await
                .map(|json| json.into_response())
        }
        crate::sport::SportLeague::Nba | crate::sport::SportLeague::Ncaab => {
            basketball::handler::get_all_games(api_key, state, Path(league))
                .await
                .map(|json| json.into_response())
        }
    }
}

/// Dispatch GET /api/{league}/games/{event_id} to the appropriate sport handler
async fn league_get_game(
    api_key: crate::auth::ApiKey,
    state: State<Arc<AppState>>,
    Path((league, event_id)): Path<(String, String)>,
) -> Result<Response, crate::error::AppError> {
    let sport_league = crate::sport::SportLeague::from_league(&league)?;
    match sport_league {
        crate::sport::SportLeague::Nfl | crate::sport::SportLeague::Ncaaf => {
            football::handler::get_game(api_key, state, Path((league, event_id)))
                .await
                .map(|json| json.into_response())
        }
        crate::sport::SportLeague::Nba | crate::sport::SportLeague::Ncaab => {
            basketball::handler::get_game(api_key, state, Path((league, event_id)))
                .await
                .map(|json| json.into_response())
        }
    }
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
    let bind_address = config.bind_address();

    // Create ESPN client with config
    let espn_client = EspnClient::new(&config.espn);

    // Create game repository for mock simulations
    let game_repository = mock::GameRepository::new();

    // Create shared application state
    let app_state = Arc::new(AppState {
        espn_client,
        config,
        game_repository,
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
        // Game endpoints — dispatch by league
        .route("/api/{league}/games", get(league_all_games))
        .route("/api/{league}/games/{event_id}", get(league_get_game))
        // Logo endpoint
        .route(
            "/api/{league}/teams/{team_id}/logo",
            get(team::get_team_logo),
        )
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

async fn root() -> &'static str {
    "Pico Scoreboard API"
}

async fn health() -> &'static str {
    "OK"
}

use axum::{Router, routing::get};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

pub mod app_update;
pub mod auth;
pub mod clock;
pub mod config;
pub mod error;
pub mod espn;
pub mod logo;
pub mod mlb;
pub mod shared;
pub mod soccer;
pub mod team;
pub mod wire;

use config::AppConfig;
use espn::EspnClient;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Pico Scoreboard API",
        description = "API for the Pi Pico scoreboard display",
        version = "2.0.0",
        contact(name = "Pico Scoreboard"),
    ),
    paths(
        clock::time,
        team::get_team_logo,
        mlb::handler::list_active_games,
        mlb::handler::get_live_game,
        soccer::handler::list_active_games,
        soccer::handler::get_live_game,
        app_update::handler::get_app_manifest,
        app_update::handler::get_app_image,
    ),
    components(schemas(
        clock::TimeResponse,
        app_update::AppManifest,
        error::ErrorResponse,
        logo::LogoQuery,
        logo::OutputFormat,
        mlb::LiveGame,
        mlb::PregameGame,
        mlb::TeamState,
        mlb::TeamColors,
        mlb::Count,
        mlb::Bases,
        mlb::AtBat,
        mlb::LastPlay,
        mlb::Inning,
        mlb::InningHalf,
        soccer::SoccerGame,
        soccer::SoccerTeam,
        soccer::SoccerTeamState,
        soccer::LastEvent,
        soccer::Side,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "clock", description = "Time and timezone endpoint"),
        (name = "team", description = "Team logo endpoint"),
        (name = "mlb", description = "MLB live game data (ESPN-backed)"),
        (name = "soccer", description = "Soccer live game data (ESPN-backed)"),
        (name = "app", description = "Device app OTA image + manifest"),
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
    pub geoip_reader: Option<maxminddb::Reader<memmap2::Mmap>>,
    /// Current device app image for OTA (None when not published)
    pub app_image: Option<app_update::AppImage>,
}

/// Initialize tracing, load config, build the router, and serve until shutdown.
pub async fn run() {
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

    // Load the device app image for OTA (optional — endpoints 404 if absent)
    let app_image = app_update::AppImage::load(&config.app.image_path);

    // Create shared application state
    let app_state = Arc::new(AppState {
        espn_client,
        config,
        geoip_reader,
        app_image,
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
        .route(
            "/{sport}/{league}/teams/{abbrev}/logo",
            get(team::get_team_logo),
        )
        .route("/baseball/mlb/games", get(mlb::list_active_games))
        .route("/baseball/mlb/games/{game_id}", get(mlb::get_live_game))
        .route("/soccer/{league}/games", get(soccer::list_active_games))
        .route(
            "/soccer/{league}/games/{game_id}",
            get(soccer::get_live_game),
        )
        .route("/app/manifest", get(app_update::get_app_manifest))
        .route("/app/image", get(app_update::get_app_image))
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

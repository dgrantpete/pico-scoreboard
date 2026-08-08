use axum::{
    Router,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::get,
};
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
pub mod football;
pub mod logo;
pub mod mlb;
pub mod nba;
pub mod shared;
pub mod soccer;
pub mod team;
pub mod wire;
#[cfg(test)]
mod wire_corpus;

use config::AppConfig;
use error::AppError;
use espn::EspnClient;
use espn::league::AnyLeague;

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
        mlb::handler::list_games,
        mlb::handler::get_game,
        nba::handler::list_games,
        nba::handler::get_game,
        football::handler::list_games,
        football::handler::get_game,
        soccer::handler::list_games,
        soccer::handler::get_game,
        app_update::handler::get_app_manifest,
        app_update::handler::get_app_image,
    ),
    components(schemas(
        clock::TimeResponse,
        app_update::AppManifest,
        error::ErrorResponse,
        logo::LogoQuery,
        logo::OutputFormat,
        shared::game::GameListEntry,
        shared::game::GameState,
        shared::game::Record,
        shared::game::LivePhase,
        shared::game::Side,
        shared::game::LastPlay,
        shared::team::TeamColors,
        shared::team::TeamState,
        mlb::MlbGame,
        mlb::MlbLiveGame,
        mlb::MlbPregameGame,
        mlb::MlbFinalGame,
        mlb::MlbPregameTeam,
        mlb::MlbFinalTeam,
        mlb::MlbWeather,
        mlb::MlbCount,
        mlb::MlbBases,
        mlb::MlbAtBat,
        mlb::MlbInning,
        mlb::InningHalf,
        nba::NbaGame,
        nba::NbaLiveGame,
        nba::NbaPregameGame,
        nba::NbaFinalGame,
        nba::NbaPregameTeam,
        nba::NbaFinalTeam,
        football::FootballGame,
        football::FootballLiveGame,
        football::FootballPregameGame,
        football::FootballFinalGame,
        football::FootballPregameTeam,
        football::FootballFinalTeam,
        football::FootballSituation,
        football::Timeouts,
        soccer::SoccerGame,
        soccer::SoccerLiveGame,
        soccer::SoccerPregameGame,
        soccer::SoccerFinalGame,
        soccer::SoccerPregameTeam,
        soccer::SoccerFinalTeam,
        soccer::SoccerFinalFlavor,
        soccer::LastEvent,
        soccer::EventKind,
        soccer::Commentary,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "clock", description = "Time and timezone endpoint"),
        (name = "team", description = "Team logo endpoint"),
        (name = "mlb", description = "MLB live game data (ESPN-backed)"),
        (name = "nba", description = "NBA live game data (ESPN-backed)"),
        (name = "football", description = "Football live game data — NFL + NCAAF (ESPN-backed)"),
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
                            "API key for the OTA endpoints (/app/*) only — game data, logos, and /time are unauthenticated (the scoreboard polls them over plain HTTP). When no key is configured on the server, authentication is disabled and this header is ignored.",
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
        .route("/{sport}/{league}/games", get(games_list))
        .route("/{sport}/{league}/games/{game_id}", get(games_detail))
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

/// `GET /{sport}/{league}/games` — resolve the (sport, league) pair to a
/// concrete league and defer to that sport's list handler. An unknown pair is
/// the existing `InvalidLeague` 404. The concrete per-sport paths remain in the
/// OpenAPI doc (see the `#[utoipa::path]` handlers); this is only the router.
async fn games_list(
    State(state): State<Arc<AppState>>,
    Path((sport, league)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    match AnyLeague::from_path(&sport, &league)? {
        AnyLeague::Mlb => mlb::list_games(&state, &headers).await,
        AnyLeague::Nba => nba::list_games(&state, &headers).await,
        AnyLeague::Football(league) => football::list_games(&state, league, &headers).await,
        AnyLeague::Soccer(league) => soccer::list_games(&state, league, &headers).await,
    }
}

/// `GET /{sport}/{league}/games/{game_id}` — same dispatch as [`games_list`]
/// for the per-game detail.
async fn games_detail(
    State(state): State<Arc<AppState>>,
    Path((sport, league, game_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    match AnyLeague::from_path(&sport, &league)? {
        AnyLeague::Mlb => mlb::get_game(&state, &game_id, &headers).await,
        AnyLeague::Nba => nba::get_game(&state, &game_id, &headers).await,
        AnyLeague::Football(league) => football::get_game(&state, league, &game_id, &headers).await,
        AnyLeague::Soccer(league) => soccer::get_game(&state, league, &game_id, &headers).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R1 gate: utoipa 5 must produce a valid schema for the internally-tagged
    /// newtype-variant `MlbGame` enum. It does — as a `oneOf` whose arms
    /// `allOf`-compose each inner struct with a `state` enum discriminator
    /// (pregame/live/final). This asserts the doc serializes and the enum
    /// discriminator survives, so no soccer-style inlining fallback is needed.
    #[test]
    fn openapi_serializes_with_mlb_state_discriminator() {
        let doc = ApiDoc::openapi();
        let json = serde_json::to_string(&doc).expect("OpenAPI doc serializes");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mlb = &value["components"]["schemas"]["MlbGame"];
        let arms = mlb["oneOf"]
            .as_array()
            .expect("MlbGame is a oneOf over its states");
        assert_eq!(arms.len(), 3, "one arm per state");
        // Collect the `state` enum literal from each arm's allOf composition.
        let states: Vec<&str> = arms
            .iter()
            .filter_map(|arm| arm["allOf"][1]["properties"]["state"]["enum"][0].as_str())
            .collect();
        assert_eq!(states, ["pregame", "live", "final"]);
    }

    /// Football is a full sibling in the doc: `FootballGame` is the same
    /// three-state `oneOf` as the others, and its routes sit under their own
    /// tag. (The shared `LivePhase`/`Side`/`LastPlay` components are registered
    /// once from `shared::game`, so there is no per-sport enum shape to assert.)
    #[test]
    fn openapi_registers_football_as_a_sibling_sport() {
        let doc = ApiDoc::openapi();
        let json = serde_json::to_string(&doc).expect("OpenAPI doc serializes");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        let arms = value["components"]["schemas"]["FootballGame"]["oneOf"]
            .as_array()
            .expect("FootballGame is a oneOf over its states");
        assert_eq!(arms.len(), 3, "one arm per state");
        let states: Vec<&str> = arms
            .iter()
            .filter_map(|arm| arm["allOf"][1]["properties"]["state"]["enum"][0].as_str())
            .collect();
        assert_eq!(states, ["pregame", "live", "final"]);

        // The football routes are present under their own tag.
        assert!(value["paths"]["/football/{league}/games"].is_object());
        assert!(value["paths"]["/football/{league}/games/{game_id}"].is_object());
    }

    /// The generic games routes share the `/{sport}/{league}/...` prefix with
    /// the logo route and sit beside the static `/health`, `/time`, `/app/*`
    /// routes. matchit conflicts panic at registration, so build a router with
    /// the same path shapes (dummy handlers) to prove they coexist — the router
    /// in `run()` is otherwise never exercised by a unit test.
    #[test]
    fn generic_games_routes_register_without_conflict() {
        async fn ok() -> &'static str {
            "ok"
        }
        let _router: Router = Router::new()
            .route("/health", get(ok))
            .route("/time", get(ok))
            .route("/{sport}/{league}/teams/{abbrev}/logo", get(ok))
            .route("/{sport}/{league}/games", get(ok))
            .route("/{sport}/{league}/games/{game_id}", get(ok))
            .route("/app/manifest", get(ok))
            .route("/app/image", get(ok));
    }
}

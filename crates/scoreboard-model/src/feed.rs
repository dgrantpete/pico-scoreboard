//! The data-source seam.
//!
//! Everything upstream of the state machine reaches it through [`GameFeed`].
//! The parity firmware supplies [`WireFeed`], which decodes the backend's
//! packed format; Phase S's direct-to-ESPN fallback supplies its own
//! implementation and this crate does not change (SPEC §13). The trait is
//! deliberately about *decoding bytes into games*, not about fetching: URLs,
//! ETags, sockets and retry policy stay in the app, where the I/O is.

use scoreboard_wire::{DecodeError, GameState, football, hockey, list, mlb, nba, soccer};

pub use crate::snapshot::Sport;

/// The endpoint key, e.g. "football/college-football" — the longest a
/// registered league produces is 25 bytes.
pub const LEAGUE_KEY: usize = 32;

/// One pollable league. `key` namespaces logo-cache slots and rotation
/// identity across leagues — a soccer "POR" crest is not an MLB "POR".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueId {
    /// e.g. "baseball/mlb", "soccer/usa.1".
    pub key: crate::text::Text<LEAGUE_KEY>,
    pub sport: Sport,
    /// User-facing, e.g. "PREMIER LEAGUE". Shown in the league menu, and —
    /// for the multi-league sports — on the pregame info line.
    pub display_name: crate::text::Text<{ crate::snapshot::MENU_LABEL }>,
}

impl LeagueId {
    /// The league a config slug names, with the endpoint key and display name
    /// the firmware uses for it. An unknown slug falls back to its own
    /// upper-cased form, so a league the backend adds works without a firmware
    /// release.
    pub fn from_slug(sport: Sport, slug: &str) -> Self {
        let mut id = Self {
            key: crate::text::Text::new(),
            sport,
            display_name: crate::text::Text::new(),
        };
        let prefix = match sport {
            Sport::Mlb => "baseball/mlb",
            Sport::Nba => "basketball/nba",
            Sport::Hockey => "hockey",
            Sport::Football => "football",
            Sport::Soccer => "soccer",
        };
        crate::text::set_plain(&mut id.key, prefix);
        if matches!(sport, Sport::Hockey | Sport::Football | Sport::Soccer) {
            let _ = id.key.push('/');
            crate::text::set_plain_append(&mut id.key, slug);
        }
        let registered = LEAGUES
            .iter()
            .find(|league| league.sport == sport && league.slug == slug);
        match registered {
            Some(league) => crate::text::set_plain(&mut id.display_name, league.display_name),
            None => crate::text::push_folded_upper(&mut id.display_name, slug),
        }
        id
    }
}

/// A league the firmware knows by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredLeague {
    pub sport: Sport,
    /// The config slug, which is also the ESPN slug the endpoint key ends in.
    pub slug: &'static str,
    pub display_name: &'static str,
}

/// Every league registered by name, mirroring the backend's registries
/// (`backend/src/espn/league.rs`), in rotation and menu order (NHL.md D6).
///
/// A table rather than a match per sport so the set can be enumerated:
/// [`crate::slate::MAX_SOURCES`] is sized to exactly this many, and
/// `scoreboard-config` proves every row fits at once. A slug that is not here
/// still polls, under its own upper-cased name.
pub const LEAGUES: &[RegisteredLeague] = &[
    league(Sport::Mlb, "mlb", "MLB"),
    league(Sport::Nba, "nba", "NBA"),
    // One hockey league today; another is a row here (NHL.md §6.4).
    league(Sport::Hockey, "nhl", "NHL"),
    league(Sport::Football, "nfl", "NFL"),
    league(Sport::Football, "college-football", "NCAA FOOTBALL"),
    league(Sport::Soccer, "usa.1", "MLS"),
    league(Sport::Soccer, "eng.1", "PREMIER LEAGUE"),
    league(Sport::Soccer, "mex.1", "LIGA MX"),
    league(Sport::Soccer, "fifa.world", "WORLD CUP"),
];

const fn league(sport: Sport, slug: &'static str, display_name: &'static str) -> RegisteredLeague {
    RegisteredLeague {
        sport,
        slug,
        display_name,
    }
}

/// A decoded game detail, borrowing the buffer it was decoded from.
///
/// These are `scoreboard-wire`'s own borrowed types rather than an owned copy:
/// the only consumer is [`crate::Store`], which copies exactly the fields it
/// keeps into bounded snapshot storage. An intermediate owned model would
/// double every string copy and serve no one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameDetail<'a> {
    Mlb(mlb::Game<'a>),
    Nba(nba::Game<'a>),
    Hockey(hockey::Game<'a>),
    Football(football::Game<'a>),
    Soccer(soccer::Game<'a>),
}

impl GameDetail<'_> {
    pub fn sport(&self) -> Sport {
        match self {
            GameDetail::Mlb(_) => Sport::Mlb,
            GameDetail::Nba(_) => Sport::Nba,
            GameDetail::Hockey(_) => Sport::Hockey,
            GameDetail::Football(_) => Sport::Football,
            GameDetail::Soccer(_) => Sport::Soccer,
        }
    }

    pub fn state(&self) -> GameState {
        match self {
            GameDetail::Mlb(game) => game.state(),
            GameDetail::Nba(game) => game.state(),
            GameDetail::Hockey(game) => game.state(),
            GameDetail::Football(game) => game.state(),
            GameDetail::Soccer(game) => game.state(),
        }
    }

    pub fn game_id(&self) -> &str {
        match self {
            GameDetail::Mlb(mlb::Game::Pregame(game)) => game.game_id,
            GameDetail::Mlb(mlb::Game::Live(game)) => game.game_id,
            GameDetail::Mlb(mlb::Game::Final(game)) => game.game_id,
            GameDetail::Nba(nba::Game::Pregame(game)) => game.game_id,
            GameDetail::Nba(nba::Game::Live(game)) => game.game_id,
            GameDetail::Nba(nba::Game::Final(game)) => game.game_id,
            GameDetail::Hockey(hockey::Game::Pregame(game)) => game.game_id,
            GameDetail::Hockey(hockey::Game::Live(game)) => game.game_id,
            GameDetail::Hockey(hockey::Game::Final(game)) => game.game_id,
            GameDetail::Football(football::Game::Pregame(game)) => game.game_id,
            GameDetail::Football(football::Game::Live(game)) => game.game_id,
            GameDetail::Football(football::Game::Final(game)) => game.game_id,
            GameDetail::Soccer(soccer::Game::Pregame(game)) => game.game_id,
            GameDetail::Soccer(soccer::Game::Live(game)) => game.game_id,
            GameDetail::Soccer(soccer::Game::Final(game)) => game.game_id,
        }
    }

    /// Both abbreviations, away then home. Present on every state of every
    /// sport, which is what lets the poller fetch crests without knowing which
    /// kind of game it is holding.
    pub fn abbreviations(&self) -> (&str, &str) {
        match self {
            GameDetail::Mlb(mlb::Game::Pregame(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Mlb(mlb::Game::Live(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Mlb(mlb::Game::Final(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Nba(nba::Game::Pregame(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Nba(nba::Game::Live(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Nba(nba::Game::Final(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Hockey(hockey::Game::Pregame(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Hockey(hockey::Game::Live(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Hockey(hockey::Game::Final(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Football(football::Game::Pregame(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Football(football::Game::Live(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Football(football::Game::Final(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Soccer(soccer::Game::Pregame(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Soccer(soccer::Game::Live(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
            GameDetail::Soccer(soccer::Game::Final(game)) => {
                (game.away.abbreviation, game.home.abbreviation)
            }
        }
    }
}

/// Where decoded games come from.
pub trait GameFeed {
    type Error: core::fmt::Debug;

    /// Decode one game-detail payload. The sport comes from the endpoint that
    /// was polled, never from sniffing the bytes.
    fn detail<'a>(&self, sport: Sport, payload: &'a [u8]) -> Result<GameDetail<'a>, Self::Error>;

    /// Decode a games list, handing each entry to `sink` in feed order.
    ///
    /// A visitor rather than an iterator: the consumer is a fixed-capacity
    /// slate that may stop early, and this keeps the trait free of associated
    /// types that every implementation would have to invent.
    fn list(&self, payload: &[u8], sink: &mut dyn ListSink) -> Result<(), Self::Error>;
}

/// Receives games-list entries from [`GameFeed::list`].
pub trait ListSink {
    /// Returns false to stop the walk — the slate is full.
    fn entry(&mut self, state: GameState, id: &str) -> bool;
}

/// The parity feed: `scoreboard-wire` over the backend's packed format.
#[derive(Debug, Clone, Copy, Default)]
pub struct WireFeed;

impl GameFeed for WireFeed {
    type Error = DecodeError;

    fn detail<'a>(&self, sport: Sport, payload: &'a [u8]) -> Result<GameDetail<'a>, Self::Error> {
        Ok(match sport {
            Sport::Mlb => GameDetail::Mlb(mlb::decode(payload)?),
            Sport::Nba => GameDetail::Nba(nba::decode(payload)?),
            Sport::Hockey => GameDetail::Hockey(hockey::decode(payload)?),
            Sport::Football => GameDetail::Football(football::decode(payload)?),
            Sport::Soccer => GameDetail::Soccer(soccer::decode(payload)?),
        })
    }

    fn list(&self, payload: &[u8], sink: &mut dyn ListSink) -> Result<(), Self::Error> {
        for entry in list::decode(payload)? {
            let entry = entry?;
            if !sink.entry(entry.state, entry.id) {
                break;
            }
        }
        Ok(())
    }
}

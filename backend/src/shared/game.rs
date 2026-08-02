//! Cross-sport outbound game primitives: the games-list contract and the
//! season record, shared verbatim by every sport's handlers and transforms.

use serde::Serialize;
use utoipa::ToSchema;

/// One entry in the games list: an ESPN event id and its current state. The
/// firmware needs the state to know which detail payload to expect and to
/// order its rotation (finals, then pregames) when no game is live.
#[derive(Serialize, ToSchema, Clone)]
pub struct GameListEntry {
    pub id: String,
    pub state: GameState,
}

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum GameState {
    Pregame,
    Live,
    Final,
}

impl GameState {
    /// The single source of the numeric state code, shared by the ETag cache
    /// tokens (`"{id}:{code}"`) and the wire `state` byte. Pregame=0, Live=1,
    /// Final=2.
    pub fn code(self) -> u8 {
        match self {
            GameState::Pregame => 0,
            GameState::Live => 1,
            GameState::Final => 2,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct Record {
    pub wins: u16,
    pub losses: u16,
}

/// The live sub-state shared by the clock-stopping sports (NBA + football):
/// breaks render without a meaningful clock, so the phase — not the clock
/// string — decides the live view. Same three states, same wire codes across
/// both sports.
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum LivePhase {
    InProgress,
    Halftime,
    EndOfPeriod,
}

impl LivePhase {
    /// The numeric wire `phase` byte: in-progress=0, halftime=1,
    /// end-of-period=2 (see the live layouts in `wire.rs`).
    pub fn code(self) -> u8 {
        match self {
            LivePhase::InProgress => 0,
            LivePhase::Halftime => 1,
            LivePhase::EndOfPeriod => 2,
        }
    }
}

/// Home or away side. Shared by football (which side has the ball) and soccer
/// (which side an event belongs to); the wire carries it as a single flag bit
/// per sport. Distinct from the inbound [`crate::espn::types::HomeAway`], which
/// is ESPN's deserialization contract.
#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Home,
    Away,
}

/// The most recent play: display text plus a change-detection `id` the firmware
/// compares between polls to trigger its flash. Shared by MLB, NBA, and football
/// (soccer's play-by-play `Commentary` is a distinct type).
#[derive(Serialize, ToSchema)]
pub struct LastPlay {
    pub id: String,
    pub text: String,
}

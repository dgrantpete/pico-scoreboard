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

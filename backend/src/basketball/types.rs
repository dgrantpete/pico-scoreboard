//! NBA outbound domain model — the JSON DTOs the handlers serialize and
//! `wire.rs` encodes. The inbound ESPN parse (scoreboard AND, for a live
//! game, the per-event summary) lives in `crates/scoreboard-espn::basketball`, which
//! also combines the two; `adapter.rs` copies its combined view here field
//! for field.

use serde::Serialize;
use utoipa::ToSchema;

use crate::shared::game::{LastPlay, LivePhase, Record, Side};
use crate::shared::team::{TeamColors, TeamState};

/// One NBA game, discriminated on the cross-sport `pre/in/post` state. The
/// firmware parses the `state` byte (0/1/2) first, then the matching payload
/// (`crates/scoreboard-wire/src/basketball.rs`).
#[derive(Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum BasketballGame {
    Pregame(BasketballPregameGame),
    Live(BasketballLiveGame),
    Final(BasketballFinalGame),
}

/// Pre-game snapshot: matchup, scheduled start, venue, the info cycle's
/// second line and (when available) season records.
#[derive(Serialize, ToSchema)]
pub struct BasketballPregameGame {
    pub game_id: String,
    /// Scheduled start as a unix epoch (seconds, UTC). The firmware applies
    /// the device's `utc_offset` for local display — it never parses dates.
    pub start_time: u32,
    pub venue: String,
    /// The info cycle's second line (NBA.md §3.8): the scoreboard note
    /// upper-cased ("NBA CUP - GROUP PLAY"), plus " - " and the series in the
    /// playoffs ("NBA FINALS - GAME 4 - NY LEADS SERIES 3-1"); empty when
    /// there is none. In the binary format it rides `ext=2` only.
    pub info_secondary: String,
    pub home: BasketballPregameTeam,
    pub away: BasketballPregameTeam,
}

/// Pre-game team: identity and pre-game context, but no score (there is no
/// game yet — a fake 0 would invite the firmware to render one).
#[derive(Serialize, ToSchema)]
pub struct BasketballPregameTeam {
    /// Team abbreviation, e.g. "LAL" — firmware uses this to fetch the logo.
    pub abbreviation: String,
    pub colors: TeamColors,
    /// Overall season record; absent when ESPN omits or malforms it.
    pub record: Option<Record>,
    /// Display-shaped betting line, spread then moneyline ("+3.5  +140"),
    /// from ESPN's first sportsbook. Both teams or neither. In the binary
    /// format it rides the `ext=1` extension only.
    pub line: Option<String>,
}

/// Live state snapshot for one NBA game. Everything below `last_play` comes
/// from the per-event summary (NBA.md D3); when its fetch failed the payload
/// is the scoreboard alone — no ledger, run or newest score, no makes — and
/// the flash is only what the scoreboard's own last play allows.
#[derive(Serialize, ToSchema)]
pub struct BasketballLiveGame {
    pub game_id: String,
    /// Quarter 1–4; overtime periods pass through as 5+.
    pub period: u8,
    /// Raw ESPN clock, display-shaped: "10:08", "53.0" under a minute; "0.0"
    /// or a reset "12:00" during breaks (see `phase`). NBA's clock stops
    /// unpredictably and ESPN sends no clock-running signal, so unlike
    /// soccer there is no `clock_seconds` — the string is exact at poll time
    /// and must not be extrapolated.
    pub clock: String,
    pub phase: LivePhase,
    pub home: TeamState,
    pub away: TeamState,
    /// The play to flash (NBA.md §3.7): the scoreboard's last play when it is
    /// a timeout, a challenge or review outcome, a technical or a flagrant,
    /// else the summary's newest such play, else absent. Made baskets,
    /// rebounds and substitutions never appear here — the court and the
    /// caption announce the scoring.
    pub last_play: Option<LastPlay>,
    /// Team fouls, bonus and timeouts (NBA.md §3.3). Absent without a
    /// summary, when the summary lacked a field on either side, and on the
    /// one-poll glitch body that zeroes every ledger field at once.
    pub ledger: Option<BasketballLedger>,
    /// The current run of unanswered points, from 6-0 (NBA.md §3.5).
    pub run: Option<BasketballRun>,
    /// The newest scoring play, free throws included (NBA.md §3.6).
    pub newest_score: Option<BasketballNewestScore>,
    /// Every made field goal with a mark, oldest first — the newest 128
    /// (NBA.md §3.4). Empty without a summary.
    pub makes: Vec<BasketballMake>,
}

/// Both ledger columns, together or not at all.
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct BasketballLedger {
    pub away: BasketballTeamLedger,
    pub home: BasketballTeamLedger,
}

/// One side's ledger column: the foul dots, their colour, the timeout blocks.
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct BasketballTeamLedger {
    /// Team fouls this period — ESPN resets the field each quarter.
    pub fouls: u8,
    /// This side shoots free throws on the opponent's next foul; equivalently
    /// the opponent has no fouls to give.
    pub in_bonus: bool,
    pub timeouts: u8,
}

/// Unanswered points by one side, period breaks included (NBA.md §3.5).
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct BasketballRun {
    pub side: Side,
    /// At least 6: a shorter run is not a run.
    pub points: u8,
}

/// The newest scoring play and what the caption says about it (NBA.md §3.6).
#[derive(Serialize, ToSchema)]
pub struct BasketballNewestScore {
    pub side: Side,
    /// The scorer's family name, upper-cased ("OUBRE"); empty when the play
    /// text named no one.
    pub scorer: String,
    /// The scorer's game points from the boxscore; absent when the boxscore
    /// has not listed the player yet (the caption shows the name alone).
    pub scorer_points: Option<u8>,
    /// ESPN's `sequenceNumber` for the play: the news-window key the device
    /// compares between polls.
    pub seq: u32,
    /// The play is also the newest make, so the blink has a mark to blink.
    pub marked: bool,
}

/// One made field goal, as the court draws it (NBA.md §3.4).
#[derive(Serialize, ToSchema, Clone, Copy)]
pub struct BasketballMake {
    pub side: Side,
    pub three: bool,
    /// Feet from the centre line, −25..=25, NBA.md §3.4's rotation applied
    /// (home's x negated, so each team attacks the basket under its own
    /// crest).
    pub x: i8,
    /// Feet from the backboard plane toward half court, −8..=47.
    pub y: i8,
}

/// Final snapshot: score, per-quarter line score, quarters played (4, or
/// more for overtime), the game's leaders, both records and (playoffs) the
/// series. No explicit winner — it is derivable from the scores, and a
/// second copy could disagree with them on a glitch.
#[derive(Serialize, ToSchema)]
pub struct BasketballFinalGame {
    pub game_id: String,
    pub periods_played: u8,
    pub home: BasketballFinalTeam,
    pub away: BasketballFinalTeam,
    /// The game's leaders in points, rebounds and assists (NBA.md §3.9) —
    /// all three or none; absent on the handful of final bodies ESPN posts
    /// without them, when the line-score rows stand in.
    pub leaders: Option<BasketballLeaders>,
    /// e.g. "NY leads series 3-1"; playoffs only.
    pub series: Option<String>,
    /// Both sides' season records, together or not at all.
    pub records: Option<BasketballRecords>,
}

#[derive(Serialize, ToSchema)]
pub struct BasketballFinalTeam {
    pub abbreviation: String,
    pub score: u32,
    pub colors: TeamColors,
    /// Points per quarter, quarter 1 first; overtime periods extend past 4.
    pub line_score: Vec<u8>,
}

/// The final screen's three rows (NBA.md §3.9).
#[derive(Serialize, ToSchema)]
pub struct BasketballLeaders {
    pub points: BasketballLeader,
    pub rebounds: BasketballLeader,
    pub assists: BasketballLeader,
}

#[derive(Serialize, ToSchema)]
pub struct BasketballLeader {
    /// Whose player — the name is drawn in that team's colour.
    pub side: Side,
    /// The stat, saturating at 255.
    pub value: u8,
    /// ESPN's short name, "J. Brunson".
    pub name: String,
}

#[derive(Serialize, ToSchema)]
pub struct BasketballRecords {
    pub away: Record,
    pub home: Record,
}

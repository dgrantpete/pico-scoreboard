//! NHL game detail (`GET /hockey/nhl/games/{game_id}`). What each field
//! *means* — how the backend derives strength, goal tags, coordinates and the
//! shootout from ESPN — is `firmware-rs/NHL.md` §3; this module is that
//! spec's §4, frozen.
//!
//! Hockey arrived after the format froze, on an endpoint no deployed decoder
//! has ever requested, so its layouts carry everything the screens need in
//! plain v2 bytes and there is no `encode_ext`. The extension mechanism (crate
//! docs) exists to keep bytes the field already decodes identical; nothing in
//! the field decodes these.
//!
//! The live and pregame fixed sections are NBA's byte for byte with hockey's
//! fields appended after the colors; the final is the shared final
//! ([`crate::common`]) whole, behind a 4-byte prefix. Flag bit 4 is "playoffs"
//! in all three states: ESPN's period 5 is a shootout in the regular season
//! but a second overtime in the playoffs, and only the season type tells them
//! apart (NHL.md §2).
//!
//! # Live (state = 1)
//!
//! Fixed 36-byte section at offset 2 (`<BBBHHIIIIBBBBBBBBHHB`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (see below)                                |
//! | 3   | u8   | period — ESPN's raw period (see [`Live::period`]) |
//! | 4   | u8   | phase (0=in progress, 2=end of period — the intermission) |
//! | 5   | u16  | away_score                                       |
//! | 7   | u16  | home_score                                       |
//! | 9   | u32  | away_colors.primary (0x00RRGGBB)                 |
//! | 13  | u32  | away_colors.alternate                            |
//! | 17  | u32  | home_colors.primary                              |
//! | 21  | u32  | home_colors.alternate                            |
//! | 25  | u8   | away shots on goal                               |
//! | 26  | u8   | home shots on goal                               |
//! | 27  | u8   | strength (0=even, 1=power play away, 2=power play home, 3=matched, 4=empty net away, 5=empty net home) |
//! | 28  | u8   | away skaters (0 = unknown)                       |
//! | 29  | u8   | home skaters                                     |
//! | 30  | u8   | goal count N, 0..=16                             |
//! | 31  | u8   | away shootout attempts, 0..=16                   |
//! | 32  | u8   | home shootout attempts                           |
//! | 33  | u16  | away shootout goals (bit i = attempt i scored)   |
//! | 35  | u16  | home shootout goals                              |
//! | 37  | u8   | next shooter (0=none, 1=away, 2=home)            |
//!
//! Flags: bit0 = last play present, bit1 = strength present (offsets 27–29),
//! bit2 = shootout present (31–37), bit3 = shots on goal present (25–26),
//! bit4 = playoffs; bits 5–7 zero. A group whose bit is unset is zeroed.
//! Phase 1 (halftime) is never sent — hockey's break is the intermission —
//! but the code set is the shared [`LivePhase`], so decode accepts it as
//! NBA's does.
//!
//! Then N goal records from offset 38, oldest first, 6 bytes each (`<BBHbb`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | +0  | u8   | bit0 = side (0=away, 1=home), bits1–2 = tag (0=GOAL, 1=PPG, 2=SHG, 3=ENG), bit3 = coordinate present; bits 4–7 zero |
//! | +1  | u8   | period (ESPN's raw period, as at offset 3)       |
//! | +2  | u16  | elapsed seconds in the period                    |
//! | +4  | i8   | x, feet from center ice, away attacking +x: −100..=100 (0 when bit3 unset) |
//! | +5  | i8   | y, −43..=43 (0 when bit3 unset)                  |
//!
//! Strings after the last record, in order: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `clock` (time remaining, display-shaped: "13:38";
//! verbatim, never extrapolated), then `last_scorer` **iff** N > 0 (the newest
//! goal's — the only scorer the screens show), then **iff** bit0:
//! `last_play.id` and `last_play.text`.
//!
//! # Pregame (state = 0)
//!
//! Fixed 33-byte section at offset 2 (`<BHHHHIIIIIHH`): the
//! [NBA pregame](crate::nba)'s 29 bytes, then the overtime losses.
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (see below)                                |
//! | 3   | u16  | away_wins  (0 if no record)                      |
//! | 5   | u16  | away_losses                                      |
//! | 7   | u16  | home_wins                                        |
//! | 9   | u16  | home_losses                                      |
//! | 11  | u32  | start_time (unix epoch, seconds, UTC)            |
//! | 15  | u32  | away_colors.primary                              |
//! | 19  | u32  | away_colors.alternate                            |
//! | 23  | u32  | home_colors.primary                              |
//! | 27  | u32  | home_colors.alternate                            |
//! | 31  | u16  | away_ot_losses (0 if no record)                  |
//! | 33  | u16  | home_ot_losses                                   |
//!
//! Flags: bit0 = away record, bit1 = home record, bit2 = away goalie, bit3 =
//! home goalie, bit4 = playoffs; bits 5–7 zero. Bits 0–3 are football's
//! pregame bits, the goalie riding the team-line slot: hockey has no betting
//! line, its per-team line is the probable starting goalie (NHL.md §3.9).
//!
//! Strings from offset 35, in order: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `venue`, `info_secondary` (always present, empty
//! outside the playoffs), then `away.goalie` **iff** bit2 and `home.goalie`
//! **iff** bit3.
//!
//! # Final (state = 2)
//!
//! A 4-byte prefix at offset 2 (`<BBBB`), then the shared final from offset 6
//! (`<BBBHHIIII` + line scores + `game_id`, `away.abbreviation`,
//! `home.abbreviation`; see [`crate::common`]):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (see below)                                |
//! | 3   | u8   | away shots on goal                               |
//! | 4   | u8   | home shots on goal                               |
//! | 5   | u8   | star sides: bits 0–1 first star, 2–3 second, 4–5 third (0=unknown, 1=away, 2=home); bits 6–7 zero |
//! | 6   | u8   | periods_played — ESPN's raw count (see [`Final::periods_played`]) |
//! | 7   | u8   | away line-score length                           |
//! | 8   | u8   | home line-score length                           |
//! | 9   | u16  | away_score                                       |
//! | 11  | u16  | home_score                                       |
//! | 13  | u32  | away_colors.primary                              |
//! | 17  | u32  | away_colors.alternate                            |
//! | 21  | u32  | home_colors.primary                              |
//! | 25  | u32  | home_colors.alternate                            |
//!
//! Flags: bit0 = stars present (offset 5), bit1 = series present, bit2 =
//! shots on goal present (3–4), bit4 = playoffs; bits 3 and 5–7 zero (bit3
//! stays free so playoffs is bit4 in every state). A group whose bit is unset
//! is zeroed.
//!
//! Hockey's bytes lead the shared ones rather than follow its strings, so
//! every number still precedes every string and the shared bytes go through
//! the one encoder and decoder every final uses. The line scores are ESPN's
//! raw per-period goals: one entry per overtime, a shootout as a fifth period
//! of 1/0.
//!
//! After `home.abbreviation`: the first, second and third star's names **iff**
//! bit0, then `series` **iff** bit1 ("CAR wins series 4-2").
//!
//! # Encoding
//!
//! Encode never fails on a field's value, as everywhere in this format
//! (strings truncate, scores saturate). [`Goals::new`] keeps the newest
//! [`MAX_GOALS`]; a side's shootout attempts are capped at
//! [`MAX_SHOOTOUT_ATTEMPTS`] — its goal mask's width — with goal bits past the
//! count dropped; a coordinate past the boards is pulled back to them. So
//! whatever encode writes, decode accepts.
//!
//! # Decoding
//!
//! Decode rejects what no encoder writes. Beyond the truncation, UTF-8 and
//! trailing-byte checks every payload gets, it rejects: a flags byte with a bit
//! its layout leaves undefined (the [`crate::nba`] extension flags' rule — the
//! newest and strictest in the crate; the older live flags ignore spare bits);
//! a code outside its set (phase, strength, a goal record's spare bits, next
//! shooter, a star's side); a goal count, or a side's shootout attempts, over
//! 16; a shootout goal bit at or past its side's attempt count; and a
//! coordinate past [`MAX_COORDINATE_X`] / [`MAX_COORDINATE_Y`]. A group whose
//! flag is unset is a placeholder: decode reports it absent without reading
//! it, as football's situation and every module's record flags do, so only
//! groups that are present are checked.

use core::fmt;

use crate::codec::{Sink, SinkExt, color_pairs, le_u16, le_u32};
use crate::common::{self, FinalParts, payload_reader, read_header, write_header};
use crate::error::{BufferFull, DecodeError, DecodeErrorKind};
use crate::{FinalTeam, GameState, HEADER_LEN, LastPlay, LivePhase, Side, TeamColors, TeamState};

/// The goals a live payload carries: the newest 16 (NHL.md §3.5).
pub const MAX_GOALS: usize = 16;

/// Per side — the width of a side's shootout goal mask (NHL.md §3.8).
pub const MAX_SHOOTOUT_ATTEMPTS: usize = 16;

/// The rink is 200 × 85 ft, so a coordinate in feet from center ice lies
/// within ±100 on x…
pub const MAX_COORDINATE_X: i8 = 100;

/// …and ±43 on y: the half-width rounded up, so a rounded 42.5 still fits.
pub const MAX_COORDINATE_Y: i8 = 43;

const LIVE_LEN: usize = 36;
const PRE_LEN: usize = 33;
const FINAL_PREFIX_LEN: usize = 4;
const GOAL_LEN: usize = 6;

const FLAG_LAST_PLAY: u8 = 0x01;
const FLAG_STRENGTH: u8 = 0x02;
const FLAG_SHOOTOUT: u8 = 0x04;
const FLAG_SHOTS: u8 = 0x08;
const FLAG_PLAYOFFS: u8 = 0x10;
const FLAGS: u8 = FLAG_LAST_PLAY | FLAG_STRENGTH | FLAG_SHOOTOUT | FLAG_SHOTS | FLAG_PLAYOFFS;

const PRE_FLAG_AWAY_RECORD: u8 = 0x01;
const PRE_FLAG_HOME_RECORD: u8 = 0x02;
const PRE_FLAG_AWAY_GOALIE: u8 = 0x04;
const PRE_FLAG_HOME_GOALIE: u8 = 0x08;
const PRE_FLAG_PLAYOFFS: u8 = 0x10;
const PRE_FLAGS: u8 = PRE_FLAG_AWAY_RECORD
    | PRE_FLAG_HOME_RECORD
    | PRE_FLAG_AWAY_GOALIE
    | PRE_FLAG_HOME_GOALIE
    | PRE_FLAG_PLAYOFFS;

const FINAL_FLAG_STARS: u8 = 0x01;
const FINAL_FLAG_SERIES: u8 = 0x02;
const FINAL_FLAG_SHOTS: u8 = 0x04;
const FINAL_FLAG_PLAYOFFS: u8 = 0x10;
const FINAL_FLAGS: u8 =
    FINAL_FLAG_STARS | FINAL_FLAG_SERIES | FINAL_FLAG_SHOTS | FINAL_FLAG_PLAYOFFS;

/// A goal record's first byte: side, tag, and whether a coordinate follows.
const GOAL_HOME: u8 = 0x01;
const GOAL_TAG_SHIFT: u32 = 1;
const GOAL_COORDINATE: u8 = 0x08;
const GOAL_BITS: u8 = 0x0F;

/// The three-way side code the next shooter and each star use: a side, or
/// none (the shootout is over; ESPN did not say whose star).
const SIDE_NONE: u8 = 0;
const SIDE_AWAY: u8 = 1;
const SIDE_HOME: u8 = 2;

/// Two bits per star, three stars; bits 6–7 must stay clear.
const STAR_SIDE_BITS: u32 = 2;
const STAR_SIDES: u8 = 0x3F;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Game<'a> {
    Pregame(Pregame<'a>),
    Live(Live<'a>),
    Final(Final<'a>),
}

impl Game<'_> {
    pub fn state(&self) -> GameState {
        match self {
            Game::Pregame(_) => GameState::Pregame,
            Game::Live(_) => GameState::Live,
            Game::Final(_) => GameState::Final,
        }
    }
}

/// A game in play. During a shootout the teams' scores stay the tied game
/// score; the tally is [`Live::shootout`]'s (NHL.md §3.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Live<'a> {
    pub game_id: &'a str,
    /// ESPN's raw period: 1–3 regulation, 4 the first overtime, 5 a
    /// regular-season shootout or a playoff second overtime ([`Live::playoffs`]
    /// says which), and on through further playoff overtimes.
    pub period: u8,
    /// `EndOfPeriod` is the intermission (ESPN's `STATUS_END_PERIOD`).
    pub phase: LivePhase,
    /// Time remaining, display-shaped and exact at poll time — the feed only
    /// moves it when a play is logged, so it is never extrapolated.
    pub clock: &'a str,
    pub playoffs: bool,
    pub away: TeamState<'a>,
    pub home: TeamState<'a>,
    /// Absent at puck drop, before ESPN has the stats it is derived from.
    pub shots: Option<Shots>,
    /// Absent when the backend had nothing to derive it from.
    pub strength: Option<Strength>,
    pub goals: Goals<'a>,
    /// The newest goal's scorer, ESPN's short name. It travels only with a
    /// goal, so a goalless game decodes it as empty.
    pub last_scorer: &'a str,
    /// Present while the shootout runs, and after it ends until the game goes
    /// final.
    pub shootout: Option<Shootout>,
    /// The newest goal or penalty (NHL.md §3.6); absent before either.
    pub last_play: Option<LastPlay<'a>>,
}

/// Shots on goal, both sides or neither (NHL.md §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shots {
    pub away: u8,
    pub home: u8,
}

/// Who is on the ice (NHL.md §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strength {
    pub kind: StrengthKind,
    /// Skaters, goalie excluded. 0 = unknown — what the play-strength
    /// fallback reports for both sides when the on-ice list is unusable.
    pub away_skaters: u8,
    pub home_skaters: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrengthKind {
    /// Full strength — and a pulled goalie the feed cannot see.
    Even,
    /// The side with more skaters.
    PowerPlay(Side),
    /// Equal and short-handed: 4-on-4, or 3-on-3 overtime.
    Matched,
    /// The side whose goalie is pulled, leaving its net empty. Wins over a
    /// power play.
    EmptyNet(Side),
}

impl StrengthKind {
    pub fn code(self) -> u8 {
        match self {
            StrengthKind::Even => 0,
            StrengthKind::PowerPlay(Side::Away) => 1,
            StrengthKind::PowerPlay(Side::Home) => 2,
            StrengthKind::Matched => 3,
            StrengthKind::EmptyNet(Side::Away) => 4,
            StrengthKind::EmptyNet(Side::Home) => 5,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(StrengthKind::Even),
            1 => Some(StrengthKind::PowerPlay(Side::Away)),
            2 => Some(StrengthKind::PowerPlay(Side::Home)),
            3 => Some(StrengthKind::Matched),
            4 => Some(StrengthKind::EmptyNet(Side::Away)),
            5 => Some(StrengthKind::EmptyNet(Side::Home)),
            _ => None,
        }
    }
}

/// The goals so far, oldest first — at most [`MAX_GOALS`].
///
/// A view, like every variable-length field in this crate: an encoder lends it
/// a slice of [`Goal`]s ([`Goals::new`]), and [`decode`] lends it the
/// payload's own validated records, each decoded as it is read. Neither
/// direction copies the run, so a decoded game borrows its receive buffer and
/// nothing else, as every sport's does (crate docs, "Both directions").
#[derive(Clone, Copy)]
pub struct Goals<'a>(GoalRun<'a>);

#[derive(Clone, Copy)]
enum GoalRun<'a> {
    Values(&'a [Goal]),
    /// Checked by [`decode`], so reading one back cannot fail.
    Records(&'a [[u8; GOAL_LEN]]),
}

impl<'a> Goals<'a> {
    /// The newest [`MAX_GOALS`] of `goals`, which are oldest first — the only
    /// ones the wire carries, and the ones the rink draws.
    pub fn new(goals: &'a [Goal]) -> Self {
        Self(GoalRun::Values(
            &goals[goals.len().saturating_sub(MAX_GOALS)..],
        ))
    }

    pub fn len(&self) -> usize {
        match self.0 {
            GoalRun::Values(goals) => goals.len(),
            GoalRun::Records(records) => records.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<Goal> {
        match self.0 {
            GoalRun::Values(goals) => goals.get(index).copied(),
            GoalRun::Records(records) => records.get(index).map(Goal::from_record),
        }
    }

    /// The newest goal — the bottom strip's "last goal".
    pub fn last(&self) -> Option<Goal> {
        self.len().checked_sub(1).and_then(|index| self.get(index))
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Goal> + ExactSizeIterator + use<'a> {
        let run = self.0;
        (0..self.len()).map(move |index| match run {
            GoalRun::Values(goals) => goals[index],
            GoalRun::Records(records) => Goal::from_record(&records[index]),
        })
    }
}

/// Equal when they hold the same goals, whichever side lent them.
impl PartialEq for Goals<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for Goals<'_> {}

impl fmt::Debug for Goals<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// One goal, as the rink and the bottom strip draw it (NHL.md §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Goal {
    pub side: Side,
    pub tag: GoalTag,
    /// ESPN's raw period, numbered as [`Live::period`] is. Never the shootout,
    /// whose goals are [`ShootoutAttempts`].
    pub period: u8,
    /// Seconds *elapsed* in the period — hockey's scoring-summary convention,
    /// unlike the live clock's time remaining.
    pub elapsed: u16,
    /// Absent when ESPN logged the goal without one: the goal still counts and
    /// can still be the last goal, it just has no mark on the rink.
    pub coordinate: Option<Coordinate>,
}

impl Goal {
    fn record(self) -> [u8; GOAL_LEN] {
        let mut head = self.tag.code() << GOAL_TAG_SHIFT;
        if self.side == Side::Home {
            head |= GOAL_HOME;
        }
        let (x, y) = match self.coordinate {
            Some(coordinate) => {
                head |= GOAL_COORDINATE;
                (
                    coordinate.x.clamp(-MAX_COORDINATE_X, MAX_COORDINATE_X),
                    coordinate.y.clamp(-MAX_COORDINATE_Y, MAX_COORDINATE_Y),
                )
            }
            None => (0, 0),
        };
        let [elapsed_low, elapsed_high] = self.elapsed.to_le_bytes();
        [
            head,
            self.period,
            elapsed_low,
            elapsed_high,
            x as u8,
            y as u8,
        ]
    }

    /// Total over every record [`check_goal`] passed: one side bit, a tag
    /// field whose four codes are all defined, and plain numbers.
    fn from_record(record: &[u8; GOAL_LEN]) -> Self {
        let head = record[0];
        Self {
            side: if head & GOAL_HOME != 0 {
                Side::Home
            } else {
                Side::Away
            },
            tag: GoalTag::from_bits(head >> GOAL_TAG_SHIFT),
            period: record[1],
            elapsed: u16::from_le_bytes([record[2], record[3]]),
            coordinate: (head & GOAL_COORDINATE != 0).then(|| Coordinate {
                x: record[4] as i8,
                y: record[5] as i8,
            }),
        }
    }
}

/// Where a goal was scored from: feet from center ice, normalized so the away
/// side attacks +x (NHL.md §3.5), within [`MAX_COORDINATE_X`] ×
/// [`MAX_COORDINATE_Y`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coordinate {
    pub x: i8,
    pub y: i8,
}

/// The goal's label on the bottom strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalTag {
    /// "GOAL" — none of the others.
    Goal,
    /// "PPG".
    PowerPlay,
    /// "SHG".
    ShortHanded,
    /// "ENG" — wins over the strength tags.
    EmptyNet,
}

impl GoalTag {
    pub fn code(self) -> u8 {
        match self {
            GoalTag::Goal => 0,
            GoalTag::PowerPlay => 1,
            GoalTag::ShortHanded => 2,
            GoalTag::EmptyNet => 3,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        (code <= 3).then(|| Self::from_bits(code))
    }

    /// The record's two tag bits, whatever sits above them: every value of a
    /// 2-bit field is a tag, so this cannot fail.
    fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0 => GoalTag::Goal,
            1 => GoalTag::PowerPlay,
            2 => GoalTag::ShortHanded,
            _ => GoalTag::EmptyNet,
        }
    }
}

/// The shootout (NHL.md §3.8): each side's attempts in shooting order, and who
/// shoots next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shootout {
    pub away: ShootoutAttempts,
    pub home: ShootoutAttempts,
    /// `None` once the shootout has ended.
    pub next_shooter: Option<Side>,
}

/// One side's shootout so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShootoutAttempts {
    /// Attempts taken, at most [`MAX_SHOOTOUT_ATTEMPTS`].
    pub taken: u8,
    /// Bit `i` set: attempt `i` scored, the first attempt being bit 0. No bit
    /// at or past `taken` is set, so the side's shootout goals are the count
    /// of set bits.
    pub scored: u16,
}

impl ShootoutAttempts {
    const NONE: Self = Self {
        taken: 0,
        scored: 0,
    };

    /// What the wire can carry: the count capped at the mask's width, and no
    /// goal bit past it.
    fn clamped(self) -> Self {
        let taken = self.taken.min(MAX_SHOOTOUT_ATTEMPTS as u8);
        Self {
            taken,
            scored: self.scored & attempt_mask(taken),
        }
    }
}

/// The goal bits `taken` attempts can set.
fn attempt_mask(taken: u8) -> u16 {
    match taken {
        0..=15 => (1 << taken) - 1,
        // 16 attempts fill the mask, and `1 << 16` does not fit a u16.
        _ => u16::MAX,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pregame<'a> {
    pub game_id: &'a str,
    /// Unix epoch seconds, UTC.
    pub start_time: u32,
    pub venue: &'a str,
    /// The info cycle's second line: the playoff round and game, then the
    /// series ("Stanley Cup Final - Game 6 - CAR leads series 3-2"); empty in
    /// the regular season (NHL.md §3.9).
    pub info_secondary: &'a str,
    pub playoffs: bool,
    pub away: PregameTeam<'a>,
    pub home: PregameTeam<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregameTeam<'a> {
    pub abbreviation: &'a str,
    pub colors: TeamColors,
    pub record: Option<Record>,
    /// The probable starting goalie's short name, e.g. "F. Andersen" —
    /// hockey's per-team line, where NBA and football show a betting line.
    pub goalie: Option<&'a str>,
}

/// W-L-OTL. Hockey's record has the third column the shared
/// [`crate::Record`] lacks, so it is its own type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub wins: u16,
    pub losses: u16,
    pub ot_losses: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Final<'a> {
    pub game_id: &'a str,
    /// ESPN's raw period count: 3, 4 after overtime, 5 after a regular-season
    /// shootout or a second playoff overtime ([`Final::playoffs`] says which),
    /// and on.
    pub periods_played: u8,
    pub playoffs: bool,
    /// ESPN's score — a shootout winner's includes the deciding goal — and its
    /// raw per-period line score, which the model collapses into the screen's
    /// columns (NHL.md §3.10).
    pub away: FinalTeam<'a>,
    pub home: FinalTeam<'a>,
    pub shots: Option<Shots>,
    /// First, second and third star — all three or none.
    pub stars: Option<[Star<'a>; 3]>,
    /// e.g. "CAR wins series 4-2"; playoffs only.
    pub series: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Star<'a> {
    /// ESPN's short name.
    pub name: &'a str,
    /// Whose player — the name is drawn in that team's color. `None` when
    /// ESPN did not say.
    pub side: Option<Side>,
}

pub fn encode<S: Sink + ?Sized>(game: &Game<'_>, out: &mut S) -> Result<(), BufferFull> {
    write_header(out, game.state())?;
    match game {
        Game::Pregame(pregame) => write_pregame(out, pregame),
        Game::Live(live) => write_live(out, live),
        Game::Final(game) => write_final(out, game),
    }
}

pub fn decode(buf: &[u8]) -> Result<Game<'_>, DecodeError> {
    match read_header(buf)? {
        GameState::Pregame => read_pregame(buf).map(Game::Pregame),
        GameState::Live => read_live(buf).map(Game::Live),
        GameState::Final => read_final(buf).map(Game::Final),
    }
}

fn write_live<S: Sink + ?Sized>(out: &mut S, game: &Live<'_>) -> Result<(), BufferFull> {
    let mut flags = 0u8;
    if game.last_play.is_some() {
        flags |= FLAG_LAST_PLAY;
    }
    if game.strength.is_some() {
        flags |= FLAG_STRENGTH;
    }
    if game.shootout.is_some() {
        flags |= FLAG_SHOOTOUT;
    }
    if game.shots.is_some() {
        flags |= FLAG_SHOTS;
    }
    if game.playoffs {
        flags |= FLAG_PLAYOFFS;
    }
    let shots = game.shots.unwrap_or(Shots { away: 0, home: 0 });
    let strength = game.strength.unwrap_or(Strength {
        kind: StrengthKind::Even,
        away_skaters: 0,
        home_skaters: 0,
    });
    let shootout = game.shootout.unwrap_or(Shootout {
        away: ShootoutAttempts::NONE,
        home: ShootoutAttempts::NONE,
        next_shooter: None,
    });
    let (away_shootout, home_shootout) = (shootout.away.clamped(), shootout.home.clamped());

    out.u8(flags)?;
    out.u8(game.period)?;
    out.u8(game.phase.code())?;
    out.u16(game.away.score)?;
    out.u16(game.home.score)?;
    out.color_pairs(game.away.colors, game.home.colors)?;
    out.u8(shots.away)?;
    out.u8(shots.home)?;
    out.u8(strength.kind.code())?;
    out.u8(strength.away_skaters)?;
    out.u8(strength.home_skaters)?;
    // `Goals` holds at most MAX_GOALS, so the count fits its byte.
    out.u8(game.goals.len() as u8)?;
    out.u8(away_shootout.taken)?;
    out.u8(home_shootout.taken)?;
    out.u16(away_shootout.scored)?;
    out.u16(home_shootout.scored)?;
    out.u8(side_code(shootout.next_shooter))?;
    for goal in game.goals.iter() {
        out.write_bytes(&goal.record())?;
    }

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    out.string(game.clock)?;
    if !game.goals.is_empty() {
        out.string(game.last_scorer)?;
    }
    if let Some(play) = &game.last_play {
        out.string(play.id)?;
        out.string(play.text)?;
    }
    Ok(())
}

fn read_live(buf: &[u8]) -> Result<Live<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<LIVE_LEN>()?;
    let flags = check_flags(fixed[0], FLAGS, "live flags")?;
    let phase = LivePhase::from_code(fixed[2])
        .ok_or_else(|| invalid_code(HEADER_LEN + 2, "live phase", fixed[2]))?;
    let (away_colors, home_colors) = color_pairs(fixed, 7);
    let shots = (flags & FLAG_SHOTS != 0).then(|| Shots {
        away: fixed[23],
        home: fixed[24],
    });
    let strength = if flags & FLAG_STRENGTH != 0 {
        Some(Strength {
            kind: StrengthKind::from_code(fixed[25])
                .ok_or_else(|| invalid_code(HEADER_LEN + 25, "strength", fixed[25]))?,
            away_skaters: fixed[26],
            home_skaters: fixed[27],
        })
    } else {
        None
    };
    let goal_count = fixed[28];
    if goal_count as usize > MAX_GOALS {
        return Err(out_of_range(HEADER_LEN + 28, "goal count", goal_count));
    }
    let shootout = if flags & FLAG_SHOOTOUT != 0 {
        Some(Shootout {
            away: read_attempts(fixed, Side::Away)?,
            home: read_attempts(fixed, Side::Home)?,
            next_shooter: side_from_code(fixed[35])
                .ok_or_else(|| invalid_code(HEADER_LEN + 35, "next shooter", fixed[35]))?,
        })
    } else {
        None
    };

    let records_at = reader.offset();
    let records = reader.records::<GOAL_LEN>("goal records", goal_count as usize)?;
    for (index, record) in records.iter().enumerate() {
        check_goal(record, records_at + index * GOAL_LEN)?;
    }

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let clock = reader.string("clock")?;
    let last_scorer = if goal_count > 0 {
        reader.string("last scorer")?
    } else {
        ""
    };
    let last_play = if flags & FLAG_LAST_PLAY != 0 {
        Some(LastPlay {
            id: reader.string("last play id")?,
            text: reader.string("last play text")?,
        })
    } else {
        None
    };
    reader.finish()?;

    Ok(Live {
        game_id,
        period: fixed[1],
        phase,
        clock,
        playoffs: flags & FLAG_PLAYOFFS != 0,
        away: TeamState {
            abbreviation: away_abbreviation,
            score: le_u16(fixed, 3),
            colors: away_colors,
        },
        home: TeamState {
            abbreviation: home_abbreviation,
            score: le_u16(fixed, 5),
            colors: home_colors,
        },
        shots,
        strength,
        goals: Goals(GoalRun::Records(records)),
        last_scorer,
        shootout,
        last_play,
    })
}

/// One side's shootout from the live fixed section.
fn read_attempts(fixed: &[u8; LIVE_LEN], side: Side) -> Result<ShootoutAttempts, DecodeError> {
    let (taken_at, scored_at, taken_field, scored_field) = match side {
        Side::Away => (29, 31, "away shootout attempts", "away shootout goals"),
        Side::Home => (30, 33, "home shootout attempts", "home shootout goals"),
    };
    let taken = fixed[taken_at];
    if taken as usize > MAX_SHOOTOUT_ATTEMPTS {
        return Err(out_of_range(HEADER_LEN + taken_at, taken_field, taken));
    }
    let scored = le_u16(fixed, scored_at);
    if scored & !attempt_mask(taken) != 0 {
        return Err(out_of_range(HEADER_LEN + scored_at, scored_field, scored));
    }
    Ok(ShootoutAttempts { taken, scored })
}

/// A goal record's spare bits and, when it has one, its coordinate. Everything
/// else in a record is a plain number.
fn check_goal(record: &[u8; GOAL_LEN], at: usize) -> Result<(), DecodeError> {
    let head = record[0];
    if head & !GOAL_BITS != 0 {
        return Err(invalid_code(at, "goal side/tag", head));
    }
    if head & GOAL_COORDINATE != 0 {
        let (x, y) = (record[4] as i8, record[5] as i8);
        if !(-MAX_COORDINATE_X..=MAX_COORDINATE_X).contains(&x) {
            return Err(out_of_range(at + 4, "goal x", x));
        }
        if !(-MAX_COORDINATE_Y..=MAX_COORDINATE_Y).contains(&y) {
            return Err(out_of_range(at + 5, "goal y", y));
        }
    }
    Ok(())
}

fn write_pregame<S: Sink + ?Sized>(out: &mut S, game: &Pregame<'_>) -> Result<(), BufferFull> {
    let mut flags = 0u8;
    if game.away.record.is_some() {
        flags |= PRE_FLAG_AWAY_RECORD;
    }
    if game.home.record.is_some() {
        flags |= PRE_FLAG_HOME_RECORD;
    }
    if game.away.goalie.is_some() {
        flags |= PRE_FLAG_AWAY_GOALIE;
    }
    if game.home.goalie.is_some() {
        flags |= PRE_FLAG_HOME_GOALIE;
    }
    if game.playoffs {
        flags |= PRE_FLAG_PLAYOFFS;
    }
    let no_record = Record {
        wins: 0,
        losses: 0,
        ot_losses: 0,
    };
    let away_record = game.away.record.unwrap_or(no_record);
    let home_record = game.home.record.unwrap_or(no_record);

    out.u8(flags)?;
    out.u16(away_record.wins)?;
    out.u16(away_record.losses)?;
    out.u16(home_record.wins)?;
    out.u16(home_record.losses)?;
    out.u32(game.start_time)?;
    out.color_pairs(game.away.colors, game.home.colors)?;
    out.u16(away_record.ot_losses)?;
    out.u16(home_record.ot_losses)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    out.string(game.venue)?;
    out.string(game.info_secondary)?;
    if let Some(goalie) = game.away.goalie {
        out.string(goalie)?;
    }
    if let Some(goalie) = game.home.goalie {
        out.string(goalie)?;
    }
    Ok(())
}

fn read_pregame(buf: &[u8]) -> Result<Pregame<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<PRE_LEN>()?;
    let flags = check_flags(fixed[0], PRE_FLAGS, "pregame flags")?;
    let (away_colors, home_colors) = color_pairs(fixed, 13);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let venue = reader.string("venue")?;
    let info_secondary = reader.string("info secondary")?;
    let away_goalie = if flags & PRE_FLAG_AWAY_GOALIE != 0 {
        Some(reader.string("away goalie")?)
    } else {
        None
    };
    let home_goalie = if flags & PRE_FLAG_HOME_GOALIE != 0 {
        Some(reader.string("home goalie")?)
    } else {
        None
    };
    reader.finish()?;

    Ok(Pregame {
        game_id,
        start_time: le_u32(fixed, 9),
        venue,
        info_secondary,
        playoffs: flags & PRE_FLAG_PLAYOFFS != 0,
        away: PregameTeam {
            abbreviation: away_abbreviation,
            colors: away_colors,
            record: (flags & PRE_FLAG_AWAY_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 1),
                losses: le_u16(fixed, 3),
                ot_losses: le_u16(fixed, 29),
            }),
            goalie: away_goalie,
        },
        home: PregameTeam {
            abbreviation: home_abbreviation,
            colors: home_colors,
            record: (flags & PRE_FLAG_HOME_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 5),
                losses: le_u16(fixed, 7),
                ot_losses: le_u16(fixed, 31),
            }),
            goalie: home_goalie,
        },
    })
}

fn write_final<S: Sink + ?Sized>(out: &mut S, game: &Final<'_>) -> Result<(), BufferFull> {
    let mut flags = 0u8;
    if game.stars.is_some() {
        flags |= FINAL_FLAG_STARS;
    }
    if game.series.is_some() {
        flags |= FINAL_FLAG_SERIES;
    }
    if game.shots.is_some() {
        flags |= FINAL_FLAG_SHOTS;
    }
    if game.playoffs {
        flags |= FINAL_FLAG_PLAYOFFS;
    }
    let shots = game.shots.unwrap_or(Shots { away: 0, home: 0 });
    let star_sides = game.stars.map_or(0, |stars| {
        stars.iter().enumerate().fold(0u8, |sides, (index, star)| {
            sides | (side_code(star.side) << (index as u32 * STAR_SIDE_BITS))
        })
    });

    out.u8(flags)?;
    out.u8(shots.away)?;
    out.u8(shots.home)?;
    out.u8(star_sides)?;
    common::write_final(
        out,
        game.game_id,
        game.periods_played,
        &game.away,
        &game.home,
    )?;
    if let Some(stars) = &game.stars {
        for star in stars {
            out.string(star.name)?;
        }
    }
    if let Some(series) = game.series {
        out.string(series)?;
    }
    Ok(())
}

fn read_final(buf: &[u8]) -> Result<Final<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let prefix = reader.fixed::<FINAL_PREFIX_LEN>()?;
    let flags = check_flags(prefix[0], FINAL_FLAGS, "final flags")?;
    let star_sides = if flags & FINAL_FLAG_STARS != 0 {
        Some(
            star_sides(prefix[3])
                .ok_or_else(|| invalid_code(HEADER_LEN + 3, "star sides", prefix[3]))?,
        )
    } else {
        None
    };
    let FinalParts {
        game_id,
        periods,
        away,
        home,
    } = common::read_final_parts(&mut reader)?;

    let stars = match star_sides {
        Some([first, second, third]) => Some([
            Star {
                name: reader.string("first star")?,
                side: first,
            },
            Star {
                name: reader.string("second star")?,
                side: second,
            },
            Star {
                name: reader.string("third star")?,
                side: third,
            },
        ]),
        None => None,
    };
    let series = if flags & FINAL_FLAG_SERIES != 0 {
        Some(reader.string("series")?)
    } else {
        None
    };
    reader.finish()?;

    Ok(Final {
        game_id,
        periods_played: periods,
        playoffs: flags & FINAL_FLAG_PLAYOFFS != 0,
        away,
        home,
        shots: (flags & FINAL_FLAG_SHOTS != 0).then(|| Shots {
            away: prefix[1],
            home: prefix[2],
        }),
        stars,
        series,
    })
}

/// The three stars' sides, first star in the low bits; `None` if any code is
/// outside the three-way set or a bit above the third star is set.
fn star_sides(byte: u8) -> Option<[Option<Side>; 3]> {
    if byte & !STAR_SIDES != 0 {
        return None;
    }
    let side = |index: u32| side_from_code((byte >> (index * STAR_SIDE_BITS)) & 0x03);
    Some([side(0)?, side(1)?, side(2)?])
}

fn side_code(side: Option<Side>) -> u8 {
    match side {
        None => SIDE_NONE,
        Some(Side::Away) => SIDE_AWAY,
        Some(Side::Home) => SIDE_HOME,
    }
}

/// The outer `None` is an unknown code; the inner one is the code for none.
fn side_from_code(code: u8) -> Option<Option<Side>> {
    match code {
        SIDE_NONE => Some(None),
        SIDE_AWAY => Some(Some(Side::Away)),
        SIDE_HOME => Some(Some(Side::Home)),
        _ => None,
    }
}

/// A flags byte must not set a bit its layout leaves undefined — a decoder
/// that ignored one would silently drop whatever a newer encoder meant by it.
fn check_flags(flags: u8, defined: u8, field: &'static str) -> Result<u8, DecodeError> {
    if flags & !defined != 0 {
        Err(invalid_code(HEADER_LEN, field, flags))
    } else {
        Ok(flags)
    }
}

fn invalid_code(offset: usize, field: &'static str, code: u8) -> DecodeError {
    DecodeError::at(offset, DecodeErrorKind::InvalidCode { field, code })
}

fn out_of_range(offset: usize, field: &'static str, value: impl Into<i32>) -> DecodeError {
    DecodeError::at(
        offset,
        DecodeErrorKind::OutOfRange {
            field,
            value: value.into(),
        },
    )
}

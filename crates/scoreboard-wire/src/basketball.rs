//! NBA game detail (`GET /basketball/nba/games/{game_id}`). What the `ext=2`
//! fields *mean* — how the backend derives the ledger, the makes, the runs
//! and the newest score from ESPN's summary — is `firmware-rs/NBA.md` §3;
//! this module is that spec's §4.
//!
//! The NBA shipped before the format froze, so its v2 layouts are what the
//! gift fleet decodes and **do not change**; everything since rides the
//! extension levels (crate docs): `ext=1` the pregame team lines, `ext=2` the
//! live, pregame and final blocks below. A payload may end after the v2
//! bytes, after the `ext=1` block or after the `ext=2` block, and [`decode`]
//! accepts all three; what a level does not carry decodes as absent.
//!
//! # Live (state = 1)
//!
//! Fixed 23-byte section at offset 2 (`<BBBHHIIII`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (bit0 = last play present)                 |
//! | 3   | u8   | period (quarter 1–4; overtime = 5+)              |
//! | 4   | u8   | phase (0=in progress, 1=halftime, 2=end of period) |
//! | 5   | u16  | away_score                                       |
//! | 7   | u16  | home_score                                       |
//! | 9   | u32  | away_colors.primary (0x00RRGGBB)                 |
//! | 13  | u32  | away_colors.alternate                            |
//! | 17  | u32  | home_colors.primary                              |
//! | 21  | u32  | home_colors.alternate                            |
//!
//! Strings from offset 25: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `clock` (display-shaped: "10:08"; "53.0" under a
//! minute; meaningless during breaks — render by `phase`; never extrapolated,
//! the firmware re-renders it each poll), then **iff** bit0: `last_play.id`
//! (change-detection key for the flash) and `last_play.text`. Plain v2 and
//! `ext=1` end here: the live game has no `ext=1` block.
//!
//! ## The `ext=2` block
//!
//! A fixed 15-byte section (`<BBBBBBBBBBIB`) where the v2 strings end:
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | +0  | u8   | flags (see below)                                |
//! | +1  | u8   | away team fouls this period                      |
//! | +2  | u8   | home team fouls this period                      |
//! | +3  | u8   | bonus: bit0 = away is in the bonus (shoots on home's next foul), bit1 = home is in the bonus; bits 2–7 zero |
//! | +4  | u8   | away timeouts remaining                          |
//! | +5  | u8   | home timeouts remaining                          |
//! | +6  | u8   | run side (0=none, 1=away, 2=home)                |
//! | +7  | u8   | run points, at least [`RUN_SHOW`] when the run is present |
//! | +8  | u8   | newest score side (0=none, 1=away, 2=home)       |
//! | +9  | u8   | the scorer's game points (0 = unknown: the caption shows the name alone) |
//! | +10 | u32  | the newest scoring play's `sequenceNumber` — the news-window key (NBA.md §3.6) |
//! | +14 | u8   | make count N, 0..=128                            |
//!
//! Flags: bit0 = ledger present (+1..+5), bit1 = run present (+6..+7), bit2
//! = newest score present (+8..+13 and the scorer string), bit3 = the newest
//! score is also the newest make (only with bit2); bits 4–7 zero. A group
//! whose bit is unset is zeroed.
//!
//! Then N make records, oldest first, 3 bytes each (`<Bbb`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | +0  | u8   | bit0 = side (0=away, 1=home), bit1 = three-pointer; bits 2–7 zero |
//! | +1  | i8   | x, feet from the centre line, −25..=25, NBA.md §3.4's rotation applied |
//! | +2  | i8   | y, feet from the backboard plane toward half court, −8..=47 |
//!
//! Then, **iff** bit2, the scorer's family name (NBA.md §3.6; empty when the
//! scoring play named no one). The play id and text for the flash stay in the
//! v2 fields (NBA.md §3.7).
//!
//! # Pregame (state = 0)
//!
//! Fixed 29-byte section at offset 2 (`<BHHHHIIIII`):
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | 2   | u8   | flags (bit0 = away record, bit1 = home record)   |
//! | 3   | u16  | away_wins  (0 if no record)                      |
//! | 5   | u16  | away_losses                                      |
//! | 7   | u16  | home_wins                                        |
//! | 9   | u16  | home_losses                                      |
//! | 11  | u32  | start_time (unix epoch, seconds, UTC)            |
//! | 15  | u32  | away_colors.primary                              |
//! | 19  | u32  | away_colors.alternate                            |
//! | 23  | u32  | home_colors.primary                              |
//! | 27  | u32  | home_colors.alternate                            |
//!
//! Strings from offset 31, in order: `game_id`, `away.abbreviation`,
//! `home.abbreviation`, `venue`. Plain v2 ends here.
//!
//! ## The team-lines block (`ext=1`)
//!
//! After `venue`: `u8 flags` (bit0 = away line present, bit1 = home line
//! present; every other bit zero), then `away.line` **iff** bit0 and
//! `home.line` **iff** bit1. A line is the team's display-shaped betting line,
//! "+3.5  +140" (spread, then moneyline) — the slot football's pregame fills
//! with the same text.
//!
//! ## The `ext=2` block
//!
//! After the `ext=1` block: one string, `info_secondary` — the info cycle's
//! second line (NBA.md §3.8: the scoreboard note, plus the series in the
//! playoffs, "NBA FINALS - GAME 4 - NY LEADS SERIES 2-1"), empty when there is
//! none.
//!
//! # Final (state = 2)
//!
//! The shared final layout (see [`crate::common`]) with per-quarter points in
//! the line scores; overtime extends both past 4. Plain v2 and `ext=1` end
//! with `home.abbreviation`: the final has no `ext=1` block.
//!
//! ## The `ext=2` block
//!
//! A fixed 13-byte section (`<BBBBBHHHH`) after `home.abbreviation`:
//!
//! | off | type | field                                            |
//! |-----|------|--------------------------------------------------|
//! | +0  | u8   | flags: bit0 = leaders, bit1 = series, bit2 = records; bits 3–7 zero |
//! | +1  | u8   | leader sides: bits 0–1 points, 2–3 rebounds, 4–5 assists (1=away, 2=home; 0 and 3 are rejected — a leader always has a side); bits 6–7 zero |
//! | +2  | u8   | points leader's value                            |
//! | +3  | u8   | rebounds leader's value                          |
//! | +4  | u8   | assists leader's value                           |
//! | +5  | u16  | away wins                                        |
//! | +7  | u16  | away losses                                      |
//! | +9  | u16  | home wins                                        |
//! | +11 | u16  | home losses                                      |
//!
//! Then, **iff** bit0, the points, rebounds and assists leaders' names
//! (ESPN's `shortName`, "J. Brunson"), then **iff** bit1 the series summary.
//! A group whose bit is unset is zeroed.
//!
//! # Encoding
//!
//! [`encode_at`] writes a level: 0 is exactly the frozen v2 bytes, 1 adds the
//! `ext=1` block, 2 adds the `ext=2` blocks, and a level above
//! [`crate::EXT_LEVEL_MAX`] writes the highest this crate defines (the server
//! serves what it knows, capped by the request). [`encode`] is level 0 by
//! name, so every device that did not ask keeps decoding the frozen bytes.
//!
//! Encode never fails on a field's value, as everywhere in this format
//! (strings truncate, scores saturate). [`Makes::new`] keeps the newest
//! [`MAX_MAKES`]; a coordinate past the lines is pulled back to them; a run
//! under [`RUN_SHOW`] is not a run on the wire and encodes as none. So whatever
//! encode writes, decode accepts.
//!
//! # Decoding
//!
//! [`decode`] accepts a payload that ends after any complete block; the groups
//! a shorter payload does not carry are absent, the makes empty and
//! `info_secondary` empty. Beyond the truncation, UTF-8 and trailing-byte
//! checks every payload gets, it rejects, in a block that is present: a flags
//! byte with a bit its layout leaves undefined, or the live block's bit3
//! without bit2; a run or newest-score side outside 1–2 when its group is
//! present; a run of fewer than [`RUN_SHOW`] points; a bonus byte with bits
//! 2–7 set; a make record with bits 2–7 set; N > 128; a coordinate outside
//! [`MAX_COORDINATE_X`] / [`MIN_COORDINATE_Y`]..=[`MAX_COORDINATE_Y`]; a
//! leader-sides byte with a 0 or 3 code or bits 6–7 set; truncation inside a
//! block. A group whose flag is unset is a placeholder: decode reports it
//! absent without reading it, as football's situation and every module's
//! record flags do, so only groups that are present are checked.

use core::fmt;

use crate::codec::{Reader, Sink, SinkExt, color_pairs, le_u16, le_u32};
use crate::common::{
    FinalParts, check_flags, invalid_code, out_of_range, payload_reader, read_final_parts,
    read_header, side_code, side_from_code, write_final, write_header,
};
use crate::error::{BufferFull, DecodeError};
use crate::{
    EXT_LEVEL_MAX, FinalTeam, GameState, HEADER_LEN, LastPlay, LivePhase, Record, Side, TeamColors,
    TeamState,
};

/// The makes a live payload carries: the newest 128 (NBA.md §3.4) — the
/// longest games run about 110 made field goals.
pub const MAX_MAKES: usize = 128;

/// A run is shown from 6-0 (NBA.md §3.5); a shorter one is not a run on the
/// wire.
pub const RUN_SHOW: u8 = 6;

/// The court is 50 ft wide, so x in feet from the centre line lies within ±25.
pub const MAX_COORDINATE_X: i8 = 25;

/// y runs from 8 ft behind the backboard plane — ESPN's floor for a baseline
/// shot…
pub const MIN_COORDINATE_Y: i8 = -8;

/// …to the half-court line, 47 ft out: a made heave from farther sits on it.
pub const MAX_COORDINATE_Y: i8 = 47;

/// The level that adds the pregame team lines, and the one that adds the
/// `ext=2` blocks: the crate's maximum, pinned so the two cannot drift.
const LEVEL_LINES: u8 = 1;
const LEVEL_BLOCKS: u8 = 2;
const _: () = assert!(LEVEL_BLOCKS == EXT_LEVEL_MAX);

const LIVE_LEN: usize = 23;
const PRE_LEN: usize = 29;
const LIVE_EXT_LEN: usize = 15;
const FINAL_EXT_LEN: usize = 13;
const MAKE_LEN: usize = 3;

const FLAG_LAST_PLAY: u8 = 0x01;

const PRE_FLAG_AWAY_RECORD: u8 = 0x01;
const PRE_FLAG_HOME_RECORD: u8 = 0x02;

/// The `ext=1` pregame block's flags.
const LINES_FLAG_AWAY: u8 = 0x01;
const LINES_FLAG_HOME: u8 = 0x02;
const LINES_FLAGS: u8 = LINES_FLAG_AWAY | LINES_FLAG_HOME;

/// The `ext=2` live block's flags.
const LIVE_EXT_FLAG_LEDGER: u8 = 0x01;
const LIVE_EXT_FLAG_RUN: u8 = 0x02;
const LIVE_EXT_FLAG_NEWEST_SCORE: u8 = 0x04;
const LIVE_EXT_FLAG_MARKED: u8 = 0x08;
const LIVE_EXT_FLAGS: u8 =
    LIVE_EXT_FLAG_LEDGER | LIVE_EXT_FLAG_RUN | LIVE_EXT_FLAG_NEWEST_SCORE | LIVE_EXT_FLAG_MARKED;

/// The ledger's bonus byte: one bit per side.
const BONUS_AWAY: u8 = 0x01;
const BONUS_HOME: u8 = 0x02;
const BONUS_BITS: u8 = BONUS_AWAY | BONUS_HOME;

/// A make record's first byte.
const MAKE_HOME: u8 = 0x01;
const MAKE_THREE: u8 = 0x02;
const MAKE_BITS: u8 = MAKE_HOME | MAKE_THREE;

/// The `ext=2` final block's flags.
const FINAL_EXT_FLAG_LEADERS: u8 = 0x01;
const FINAL_EXT_FLAG_SERIES: u8 = 0x02;
const FINAL_EXT_FLAG_RECORDS: u8 = 0x04;
const FINAL_EXT_FLAGS: u8 = FINAL_EXT_FLAG_LEADERS | FINAL_EXT_FLAG_SERIES | FINAL_EXT_FLAG_RECORDS;

/// Two bits per leader, three leaders; bits 6–7 must stay clear.
const LEADER_SIDE_BITS: u32 = 2;
const LEADER_SIDES: u8 = 0x3F;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Live<'a> {
    pub game_id: &'a str,
    /// Quarter 1–4; overtime periods pass through as 5+.
    pub period: u8,
    pub phase: LivePhase,
    /// Display-shaped and exact at poll time — never extrapolated.
    pub clock: &'a str,
    pub away: TeamState<'a>,
    pub home: TeamState<'a>,
    /// Absent before the opening tip.
    pub last_play: Option<LastPlay<'a>>,
    /// Team fouls, bonus and timeouts (NBA.md §3.3). `ext=2` only: absent on a
    /// shorter payload, when the summary lacked a field on either side, and on
    /// the glitch body the model holds the previous ledger through.
    pub ledger: Option<Ledger>,
    /// The current run of unanswered points, from [`RUN_SHOW`] (NBA.md §3.5).
    /// `ext=2` only.
    pub run: Option<Run>,
    /// The newest scoring play (NBA.md §3.6). `ext=2` only.
    pub newest_score: Option<NewestScore<'a>>,
    /// Every made field goal with a mark, oldest first — the newest
    /// [`MAX_MAKES`] of them (NBA.md §3.4). Empty below `ext=2`.
    pub makes: Makes<'a>,
}

/// Both ledger columns, together or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ledger {
    pub away: TeamLedger,
    pub home: TeamLedger,
}

/// One side's ledger column: the foul dots, their colour, the timeout blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamLedger {
    /// Team fouls this period — ESPN resets the field each quarter.
    pub fouls: u8,
    /// This side shoots free throws on the opponent's next foul; equivalently
    /// the opponent has no fouls to give, which is what turns the opponent's
    /// dots red.
    pub in_bonus: bool,
    pub timeouts: u8,
}

impl Ledger {
    const NONE: Self = Self {
        away: TeamLedger::NONE,
        home: TeamLedger::NONE,
    };
}

impl TeamLedger {
    const NONE: Self = Self {
        fouls: 0,
        in_bonus: false,
        timeouts: 0,
    };
}

/// Unanswered points by one side, period breaks included (NBA.md §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub side: Side,
    /// At least [`RUN_SHOW`] on the wire; the extractor caps it at 255.
    pub points: u8,
}

/// The newest scoring play — free throws included — and what the caption
/// says about it (NBA.md §3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewestScore<'a> {
    pub side: Side,
    /// The scorer's game points; 0 = not known yet (the boxscore has not
    /// listed the player), and the caption shows the name alone.
    pub scorer_points: u8,
    /// ESPN's `sequenceNumber` for the play: the store stamps the news window
    /// when it changes between commits of the same game.
    pub seq: u32,
    /// The play is also the newest make, so the blink has a mark to blink.
    pub marked: bool,
    /// The scorer's family name, upper-cased ("OUBRE"); empty when the play
    /// text named no one (a goaltending award).
    pub scorer: &'a str,
}

/// The makes so far, oldest first — at most [`MAX_MAKES`].
///
/// A view, like every variable-length field in this crate: an encoder lends it
/// a slice of [`Make`]s ([`Makes::new`]), and [`decode`] lends it the payload's
/// own validated records, each decoded as it is read. Neither direction copies
/// the run, so a decoded game borrows its receive buffer and nothing else, as
/// every sport's does (crate docs, "Both directions").
#[derive(Clone, Copy)]
pub struct Makes<'a>(MakeRun<'a>);

#[derive(Clone, Copy)]
enum MakeRun<'a> {
    Values(&'a [Make]),
    /// Checked by [`decode`], so reading one back cannot fail.
    Records(&'a [[u8; MAKE_LEN]]),
}

impl<'a> Makes<'a> {
    /// The newest [`MAX_MAKES`] of `makes`, which are oldest first — the only
    /// ones the wire carries, and the ones the court draws.
    pub fn new(makes: &'a [Make]) -> Self {
        Self(MakeRun::Values(
            &makes[makes.len().saturating_sub(MAX_MAKES)..],
        ))
    }

    pub fn len(&self) -> usize {
        match self.0 {
            MakeRun::Values(makes) => makes.len(),
            MakeRun::Records(records) => records.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, index: usize) -> Option<Make> {
        match self.0 {
            MakeRun::Values(makes) => makes.get(index).copied(),
            MakeRun::Records(records) => records.get(index).map(Make::from_record),
        }
    }

    /// The newest make — the one that blinks while the news window is open.
    pub fn last(&self) -> Option<Make> {
        self.len().checked_sub(1).and_then(|index| self.get(index))
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Make> + ExactSizeIterator + use<'a> {
        let run = self.0;
        (0..self.len()).map(move |index| match run {
            MakeRun::Values(makes) => makes[index],
            MakeRun::Records(records) => Make::from_record(&records[index]),
        })
    }
}

/// Equal when they hold the same makes, whichever side lent them.
impl PartialEq for Makes<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for Makes<'_> {}

impl fmt::Debug for Makes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// One made field goal, as the court draws it (NBA.md §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Make {
    pub side: Side,
    pub three: bool,
    /// Feet from the centre line, within ±[`MAX_COORDINATE_X`], NBA.md §3.4's
    /// rotation applied (home's x negated).
    pub x: i8,
    /// Feet from the backboard plane toward half court,
    /// [`MIN_COORDINATE_Y`]..=[`MAX_COORDINATE_Y`].
    pub y: i8,
}

impl Make {
    fn record(self) -> [u8; MAKE_LEN] {
        let mut head = 0u8;
        if self.side == Side::Home {
            head |= MAKE_HOME;
        }
        if self.three {
            head |= MAKE_THREE;
        }
        [
            head,
            self.x.clamp(-MAX_COORDINATE_X, MAX_COORDINATE_X) as u8,
            self.y.clamp(MIN_COORDINATE_Y, MAX_COORDINATE_Y) as u8,
        ]
    }

    /// Total over every record [`check_make`] passed: two flag bits and two
    /// plain numbers.
    fn from_record(record: &[u8; MAKE_LEN]) -> Self {
        let head = record[0];
        Self {
            side: if head & MAKE_HOME != 0 {
                Side::Home
            } else {
                Side::Away
            },
            three: head & MAKE_THREE != 0,
            x: record[1] as i8,
            y: record[2] as i8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pregame<'a> {
    pub game_id: &'a str,
    /// Unix epoch seconds, UTC.
    pub start_time: u32,
    pub venue: &'a str,
    /// The info cycle's second line: the scoreboard note, plus the series in
    /// the playoffs ("NBA FINALS - GAME 4 - NY LEADS SERIES 2-1"); empty when
    /// there is none (NBA.md §3.8). Carried only by `ext=2`: a shorter payload
    /// decodes it as empty.
    pub info_secondary: &'a str,
    pub away: PregameTeam<'a>,
    pub home: PregameTeam<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregameTeam<'a> {
    pub abbreviation: &'a str,
    pub colors: TeamColors,
    pub record: Option<Record>,
    /// The betting line, "+3.5  +140". Carried only by the `ext=1` block:
    /// [`encode`] drops it, and a plain v2 payload decodes it as `None`.
    pub line: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Final<'a> {
    pub game_id: &'a str,
    /// 4, or more with overtime.
    pub periods_played: u8,
    pub away: FinalTeam<'a>,
    pub home: FinalTeam<'a>,
    /// The game's leaders (NBA.md §3.9). `ext=2` only; absent on the handful
    /// of final bodies ESPN posts without them, when the line-score rows stand
    /// in.
    pub leaders: Option<Leaders<'a>>,
    /// e.g. "NY leads series 2-1"; playoffs only. `ext=2` only.
    pub series: Option<&'a str>,
    /// Both sides' season records, together or not at all. `ext=2` only.
    pub records: Option<Records>,
}

/// The game's leaders in points, rebounds and assists — all three or none
/// (NBA.md §3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leaders<'a> {
    pub points: Leader<'a>,
    pub rebounds: Leader<'a>,
    pub assists: Leader<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leader<'a> {
    /// Whose player — the name is drawn in that team's colour.
    pub side: Side,
    /// The stat, saturating.
    pub value: u8,
    /// ESPN's short name, "J. Brunson".
    pub name: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Records {
    pub away: Record,
    pub home: Record,
}

impl Records {
    const NONE: Self = Self {
        away: Record { wins: 0, losses: 0 },
        home: Record { wins: 0, losses: 0 },
    };
}

/// Level 0: exactly the frozen v2 bytes, what every device that did not ask
/// for extensions decodes.
pub fn encode<S: Sink + ?Sized>(game: &Game<'_>, out: &mut S) -> Result<(), BufferFull> {
    encode_at(game, 0, out)
}

/// `game` at an extension level (crate docs): 0 is the frozen v2 bytes, 1
/// adds the `ext=1` block, 2 adds the `ext=2` blocks. A level above
/// [`crate::EXT_LEVEL_MAX`] writes `EXT_LEVEL_MAX`: the server serves the
/// highest level it knows, capped by the request.
pub fn encode_at<S: Sink + ?Sized>(
    game: &Game<'_>,
    level: u8,
    out: &mut S,
) -> Result<(), BufferFull> {
    write_header(out, game.state())?;
    match game {
        Game::Pregame(pregame) => {
            write_pregame(out, pregame)?;
            if level >= LEVEL_LINES {
                write_pregame_lines(out, pregame)?;
            }
            if level >= LEVEL_BLOCKS {
                out.string(pregame.info_secondary)?;
            }
        }
        Game::Live(live) => {
            write_live(out, live)?;
            if level >= LEVEL_BLOCKS {
                write_live_ext(out, live)?;
            }
        }
        Game::Final(game) => {
            write_final(
                out,
                game.game_id,
                game.periods_played,
                &game.away,
                &game.home,
            )?;
            if level >= LEVEL_BLOCKS {
                write_final_ext(out, game)?;
            }
        }
    }
    Ok(())
}

pub fn decode(buf: &[u8]) -> Result<Game<'_>, DecodeError> {
    match read_header(buf)? {
        GameState::Pregame => read_pregame(buf).map(Game::Pregame),
        GameState::Live => read_live(buf).map(Game::Live),
        GameState::Final => read_final(buf).map(Game::Final),
    }
}

fn write_live<S: Sink + ?Sized>(out: &mut S, game: &Live<'_>) -> Result<(), BufferFull> {
    out.u8(if game.last_play.is_some() {
        FLAG_LAST_PLAY
    } else {
        0
    })?;
    out.u8(game.period)?;
    out.u8(game.phase.code())?;
    out.u16(game.away.score)?;
    out.u16(game.home.score)?;
    out.color_pairs(game.away.colors, game.home.colors)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    out.string(game.clock)?;
    if let Some(play) = &game.last_play {
        out.string(play.id)?;
        out.string(play.text)?;
    }
    Ok(())
}

fn write_live_ext<S: Sink + ?Sized>(out: &mut S, game: &Live<'_>) -> Result<(), BufferFull> {
    // A run under RUN_SHOW is not a run (NBA.md §3.5): its flag stays clear
    // and its bytes are the placeholder's, so the payload decodes.
    let run = game.run.filter(|run| run.points >= RUN_SHOW);
    let score = game.newest_score;
    let mut flags = 0u8;
    if game.ledger.is_some() {
        flags |= LIVE_EXT_FLAG_LEDGER;
    }
    if run.is_some() {
        flags |= LIVE_EXT_FLAG_RUN;
    }
    if let Some(score) = score {
        flags |= LIVE_EXT_FLAG_NEWEST_SCORE;
        if score.marked {
            flags |= LIVE_EXT_FLAG_MARKED;
        }
    }
    let ledger = game.ledger.unwrap_or(Ledger::NONE);
    let mut bonus = 0u8;
    if ledger.away.in_bonus {
        bonus |= BONUS_AWAY;
    }
    if ledger.home.in_bonus {
        bonus |= BONUS_HOME;
    }

    out.u8(flags)?;
    out.u8(ledger.away.fouls)?;
    out.u8(ledger.home.fouls)?;
    out.u8(bonus)?;
    out.u8(ledger.away.timeouts)?;
    out.u8(ledger.home.timeouts)?;
    out.u8(side_code(run.map(|run| run.side)))?;
    out.u8(run.map_or(0, |run| run.points))?;
    out.u8(side_code(score.map(|score| score.side)))?;
    out.u8(score.map_or(0, |score| score.scorer_points))?;
    out.u32(score.map_or(0, |score| score.seq))?;
    // `Makes` holds at most MAX_MAKES, so the count fits its byte.
    out.u8(game.makes.len() as u8)?;
    for make in game.makes.iter() {
        out.write_bytes(&make.record())?;
    }
    if let Some(score) = score {
        out.string(score.scorer)?;
    }
    Ok(())
}

fn read_live(buf: &[u8]) -> Result<Live<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<LIVE_LEN>()?;
    let flags = fixed[0];
    let phase = LivePhase::from_code(fixed[2])
        .ok_or_else(|| invalid_code(HEADER_LEN + 2, "live phase", fixed[2]))?;
    let (away_colors, home_colors) = color_pairs(fixed, 7);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let clock = reader.string("clock")?;
    let last_play = if flags & FLAG_LAST_PLAY != 0 {
        Some(LastPlay {
            id: reader.string("last play id")?,
            text: reader.string("last play text")?,
        })
    } else {
        None
    };
    let ext = if reader.remaining() > 0 {
        read_live_ext(&mut reader)?
    } else {
        LiveExt::NONE
    };
    reader.finish()?;

    Ok(Live {
        game_id,
        period: fixed[1],
        phase,
        clock,
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
        last_play,
        ledger: ext.ledger,
        run: ext.run,
        newest_score: ext.newest_score,
        makes: ext.makes,
    })
}

/// What the `ext=2` live block decodes to; all absent on a shorter payload.
struct LiveExt<'a> {
    ledger: Option<Ledger>,
    run: Option<Run>,
    newest_score: Option<NewestScore<'a>>,
    makes: Makes<'a>,
}

impl LiveExt<'_> {
    const NONE: Self = Self {
        ledger: None,
        run: None,
        newest_score: None,
        makes: Makes(MakeRun::Values(&[])),
    };
}

fn read_live_ext<'a>(reader: &mut Reader<'a>) -> Result<LiveExt<'a>, DecodeError> {
    let at = reader.offset();
    let fixed = reader.fixed::<LIVE_EXT_LEN>()?;
    let flags = check_flags(at, fixed[0], LIVE_EXT_FLAGS, "live extension flags")?;
    // The mark belongs to the newest score; no encoder sets it alone.
    if flags & LIVE_EXT_FLAG_MARKED != 0 && flags & LIVE_EXT_FLAG_NEWEST_SCORE == 0 {
        return Err(invalid_code(at, "live extension flags", flags));
    }
    let ledger = if flags & LIVE_EXT_FLAG_LEDGER != 0 {
        let bonus = fixed[3];
        if bonus & !BONUS_BITS != 0 {
            return Err(invalid_code(at + 3, "bonus", bonus));
        }
        Some(Ledger {
            away: TeamLedger {
                fouls: fixed[1],
                in_bonus: bonus & BONUS_AWAY != 0,
                timeouts: fixed[4],
            },
            home: TeamLedger {
                fouls: fixed[2],
                in_bonus: bonus & BONUS_HOME != 0,
                timeouts: fixed[5],
            },
        })
    } else {
        None
    };
    let run = if flags & LIVE_EXT_FLAG_RUN != 0 {
        let side = present_side(fixed[6], at + 6, "run side")?;
        let points = fixed[7];
        if points < RUN_SHOW {
            return Err(out_of_range(at + 7, "run points", points));
        }
        Some(Run { side, points })
    } else {
        None
    };
    let newest_side = if flags & LIVE_EXT_FLAG_NEWEST_SCORE != 0 {
        Some(present_side(fixed[8], at + 8, "newest score side")?)
    } else {
        None
    };
    let count = fixed[14];
    if count as usize > MAX_MAKES {
        return Err(out_of_range(at + 14, "make count", count));
    }

    let records_at = reader.offset();
    let records = reader.records::<MAKE_LEN>("make records", count as usize)?;
    for (index, record) in records.iter().enumerate() {
        check_make(record, records_at + index * MAKE_LEN)?;
    }
    let newest_score = match newest_side {
        Some(side) => Some(NewestScore {
            side,
            scorer_points: fixed[9],
            seq: le_u32(fixed, 10),
            marked: flags & LIVE_EXT_FLAG_MARKED != 0,
            scorer: reader.string("scorer")?,
        }),
        None => None,
    };

    Ok(LiveExt {
        ledger,
        run,
        newest_score,
        makes: Makes(MakeRun::Records(records)),
    })
}

/// A side code where a flag says its group is present: away or home, nothing
/// else — the none code is the placeholder's.
fn present_side(code: u8, at: usize, field: &'static str) -> Result<Side, DecodeError> {
    side_from_code(code)
        .flatten()
        .ok_or_else(|| invalid_code(at, field, code))
}

/// A make record's spare bits and its coordinate.
fn check_make(record: &[u8; MAKE_LEN], at: usize) -> Result<(), DecodeError> {
    let head = record[0];
    if head & !MAKE_BITS != 0 {
        return Err(invalid_code(at, "make side/three", head));
    }
    let (x, y) = (record[1] as i8, record[2] as i8);
    if !(-MAX_COORDINATE_X..=MAX_COORDINATE_X).contains(&x) {
        return Err(out_of_range(at + 1, "make x", x));
    }
    if !(MIN_COORDINATE_Y..=MAX_COORDINATE_Y).contains(&y) {
        return Err(out_of_range(at + 2, "make y", y));
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
    let away_record = game.away.record.unwrap_or(Record { wins: 0, losses: 0 });
    let home_record = game.home.record.unwrap_or(Record { wins: 0, losses: 0 });

    out.u8(flags)?;
    out.u16(away_record.wins)?;
    out.u16(away_record.losses)?;
    out.u16(home_record.wins)?;
    out.u16(home_record.losses)?;
    out.u32(game.start_time)?;
    out.color_pairs(game.away.colors, game.home.colors)?;

    out.string(game.game_id)?;
    out.string(game.away.abbreviation)?;
    out.string(game.home.abbreviation)?;
    out.string(game.venue)
}

fn write_pregame_lines<S: Sink + ?Sized>(
    out: &mut S,
    game: &Pregame<'_>,
) -> Result<(), BufferFull> {
    let mut flags = 0u8;
    if game.away.line.is_some() {
        flags |= LINES_FLAG_AWAY;
    }
    if game.home.line.is_some() {
        flags |= LINES_FLAG_HOME;
    }
    out.u8(flags)?;
    if let Some(line) = game.away.line {
        out.string(line)?;
    }
    if let Some(line) = game.home.line {
        out.string(line)?;
    }
    Ok(())
}

fn read_pregame(buf: &[u8]) -> Result<Pregame<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let fixed = reader.fixed::<PRE_LEN>()?;
    let flags = fixed[0];
    let (away_colors, home_colors) = color_pairs(fixed, 13);

    let game_id = reader.string("game_id")?;
    let away_abbreviation = reader.string("away abbreviation")?;
    let home_abbreviation = reader.string("home abbreviation")?;
    let venue = reader.string("venue")?;
    // The `ext=1` team lines, when the server appended them…
    let (mut away_line, mut home_line) = (None, None);
    if reader.remaining() > 0 {
        let at = reader.offset();
        let lines = check_flags(
            at,
            reader.u8("team lines flags")?,
            LINES_FLAGS,
            "team lines flags",
        )?;
        if lines & LINES_FLAG_AWAY != 0 {
            away_line = Some(reader.string("away line")?);
        }
        if lines & LINES_FLAG_HOME != 0 {
            home_line = Some(reader.string("home line")?);
        }
    }
    // …and the `ext=2` second info line after them.
    let info_secondary = if reader.remaining() > 0 {
        reader.string("info secondary")?
    } else {
        ""
    };
    reader.finish()?;

    Ok(Pregame {
        game_id,
        start_time: le_u32(fixed, 9),
        venue,
        info_secondary,
        away: PregameTeam {
            abbreviation: away_abbreviation,
            colors: away_colors,
            record: (flags & PRE_FLAG_AWAY_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 1),
                losses: le_u16(fixed, 3),
            }),
            line: away_line,
        },
        home: PregameTeam {
            abbreviation: home_abbreviation,
            colors: home_colors,
            record: (flags & PRE_FLAG_HOME_RECORD != 0).then(|| Record {
                wins: le_u16(fixed, 5),
                losses: le_u16(fixed, 7),
            }),
            line: home_line,
        },
    })
}

fn write_final_ext<S: Sink + ?Sized>(out: &mut S, game: &Final<'_>) -> Result<(), BufferFull> {
    let mut flags = 0u8;
    if game.leaders.is_some() {
        flags |= FINAL_EXT_FLAG_LEADERS;
    }
    if game.series.is_some() {
        flags |= FINAL_EXT_FLAG_SERIES;
    }
    if game.records.is_some() {
        flags |= FINAL_EXT_FLAG_RECORDS;
    }
    let sides = game.leaders.map_or(0, |leaders| {
        [leaders.points, leaders.rebounds, leaders.assists]
            .iter()
            .enumerate()
            .fold(0u8, |sides, (index, leader)| {
                sides | (side_code(Some(leader.side)) << (index as u32 * LEADER_SIDE_BITS))
            })
    });
    let values = game.leaders.map_or([0; 3], |leaders| {
        [
            leaders.points.value,
            leaders.rebounds.value,
            leaders.assists.value,
        ]
    });
    let records = game.records.unwrap_or(Records::NONE);

    out.u8(flags)?;
    out.u8(sides)?;
    out.write_bytes(&values)?;
    out.u16(records.away.wins)?;
    out.u16(records.away.losses)?;
    out.u16(records.home.wins)?;
    out.u16(records.home.losses)?;
    if let Some(leaders) = &game.leaders {
        out.string(leaders.points.name)?;
        out.string(leaders.rebounds.name)?;
        out.string(leaders.assists.name)?;
    }
    if let Some(series) = game.series {
        out.string(series)?;
    }
    Ok(())
}

fn read_final(buf: &[u8]) -> Result<Final<'_>, DecodeError> {
    let mut reader = payload_reader(buf);
    let FinalParts {
        game_id,
        periods,
        away,
        home,
    } = read_final_parts(&mut reader)?;
    let ext = if reader.remaining() > 0 {
        read_final_ext(&mut reader)?
    } else {
        FinalExt::NONE
    };
    reader.finish()?;

    Ok(Final {
        game_id,
        periods_played: periods,
        away,
        home,
        leaders: ext.leaders,
        series: ext.series,
        records: ext.records,
    })
}

/// What the `ext=2` final block decodes to; all absent on a shorter payload.
struct FinalExt<'a> {
    leaders: Option<Leaders<'a>>,
    series: Option<&'a str>,
    records: Option<Records>,
}

impl FinalExt<'_> {
    const NONE: Self = Self {
        leaders: None,
        series: None,
        records: None,
    };
}

fn read_final_ext<'a>(reader: &mut Reader<'a>) -> Result<FinalExt<'a>, DecodeError> {
    let at = reader.offset();
    let fixed = reader.fixed::<FINAL_EXT_LEN>()?;
    let flags = check_flags(at, fixed[0], FINAL_EXT_FLAGS, "final extension flags")?;
    let sides = if flags & FINAL_EXT_FLAG_LEADERS != 0 {
        Some(leader_sides(fixed[1]).ok_or_else(|| invalid_code(at + 1, "leader sides", fixed[1]))?)
    } else {
        None
    };
    let leaders = match sides {
        Some([points, rebounds, assists]) => Some(Leaders {
            points: Leader {
                side: points,
                value: fixed[2],
                name: reader.string("points leader")?,
            },
            rebounds: Leader {
                side: rebounds,
                value: fixed[3],
                name: reader.string("rebounds leader")?,
            },
            assists: Leader {
                side: assists,
                value: fixed[4],
                name: reader.string("assists leader")?,
            },
        }),
        None => None,
    };
    let series = if flags & FINAL_EXT_FLAG_SERIES != 0 {
        Some(reader.string("series")?)
    } else {
        None
    };
    let records = (flags & FINAL_EXT_FLAG_RECORDS != 0).then(|| Records {
        away: Record {
            wins: le_u16(fixed, 5),
            losses: le_u16(fixed, 7),
        },
        home: Record {
            wins: le_u16(fixed, 9),
            losses: le_u16(fixed, 11),
        },
    });

    Ok(FinalExt {
        leaders,
        series,
        records,
    })
}

/// The three leaders' sides, points in the low bits; `None` if any code is
/// not a side (a leader always has one) or a bit above the third leader is
/// set.
fn leader_sides(byte: u8) -> Option<[Side; 3]> {
    if byte & !LEADER_SIDES != 0 {
        return None;
    }
    let side = |index: u32| side_from_code((byte >> (index * LEADER_SIDE_BITS)) & 0x03).flatten();
    Some([side(0)?, side(1)?, side(2)?])
}

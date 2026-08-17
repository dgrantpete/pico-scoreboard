//! SOCCER extraction tables — S1 sport lane (see DESIGN.md and the soccer
//! inventory report).
//!
//! Two streamed documents feed one live payload: the league SCOREBOARD body
//! (`{"events":[…]}`, [`GameExtractor`] / [`ListExtractor`]) and — for live
//! games only — the per-event SUMMARY body ([`SummaryExtractor`]), whose
//! single wire-relevant field is the latest commentary line. Commentary rides
//! the extract as an `Option` the summary pass fills ([`SoccerExtract::set_commentary`]).
//!
//! Parity notes the tables are built around (backend `soccer/{types,transform}.rs`):
//! - A postponed match arrives as `pre` and stays a pregame card — the `pre`
//!   arm never consults `status.type.description`.
//! - `is_break` keys on the exact description sets; `"Shootout"` is ACTIVE
//!   play (the firmware renders it off `half == 5`).
//! - `clock_seconds` comes from `displayClock` (floor minutes × 60, stoppage
//!   summed); the numeric `status.clock` caps at regulation and is only the
//!   fallback for an unparseable string.
//! - Last event = LAST of the equal clock maxima among goals/red cards
//!   (`Iterator::max_by` semantics); red card outranks goal on a detail
//!   flagged both ways. Scorer lists are STABLE-sorted by clock. Both are
//!   byte-load-bearing (ruling 13) and pinned by named tests.
//! - Side attribution string-compares `detail.team.id` against both
//!   competitor ids at finalize — order-independent (ruling 4).

use core::cmp::Ordering;
use core::fmt::Write as _;

use scoreboard_wire::soccer::{
    Commentary as WireCommentary, Event as WireEvent, EventKind, Final as WireFinal, FinalFlavor,
    FinalTeam as WireFinalTeam, Game as WireGame, Live as WireLive, Pregame as WirePregame,
    PregameTeam as WirePregameTeam,
};
use scoreboard_wire::{GameState, Side, TeamColors, TeamState, truncate_utf8};

use crate::common::{
    EText, HomeAway, IgnoreQuirks, Quirk, Quirks, order_home_away, parse_hex_rgb,
    parse_start_time, saturate_score, set_text,
};
use crate::path::{Directive, Error, Pattern, Seg, Sink, StreamMatcher, Value};

use Seg::{AnyIndex, Index, Key};

// ---------------------------------------------------------------- bounds

/// ESPN ids (event and team) are numeric strings, ≤ 10 digits in every
/// sampled league (corpus max 6 bytes). These are compare keys, never wire
/// strings, so overflow marks the field invalid instead of truncating — a
/// silently-truncated key could false-match a side.
const ID_BYTES: usize = 24;
/// Scores parse to `u32` (10 digits max) or 502; margin for `+`/leading
/// zeros. Overflow ⇒ the same parse failure the backend reports.
const SCORE_BYTES: usize = 16;
/// Valid colors are ≤ 7 bytes (`#RRGGBB`); anything longer fails the
/// exactly-6-hex-digits rule in the backend too, so overflow ⇒ invalid is
/// byte-identical behavior.
const COLOR_BYTES: usize = 8;
/// Longest known `status.type.description` is 30 bytes ("Final Score - After
/// Extra Time"). An overflowing description cannot equal any known label and
/// degrades through the same unknown-label arm the backend uses.
const DESC_BYTES: usize = 48;
/// Valid `event.date` forms are ≤ 20 bytes; longer fails chrono in the
/// backend and `parse_start_time` here.
const DATE_BYTES: usize = 32;
/// Scoring details buffered for the final scorer lists (side-attribution
/// waits for finalize, so both sides share the pool). Corpus max is 7 scoring
/// details in a match (`final_after_penalties`, 12 details total); 4× margin.
/// Overflow drops the excess — beyond anything ESPN-covered soccer produces.
const SCORING_MAX: usize = 32;
/// Games kept by the list extractor: an ESPN soccer league plays at most
/// ~15 matches a day (a full MLS slate); 4× margin. Overflow is reported via
/// [`GamesList::overflowed`], never silent.
const LIST_MAX: usize = 64;
/// `u32::MAX` stringifies to 10 digits — the commentary id is generated, not
/// copied, so this bound is structural.
const SEQ_BYTES: usize = 10;

// ---------------------------------------------------------------- extracts

/// One transformed soccer game — the post-transform, domain-shaped seam
/// (DESIGN.md): the backend adapter builds its DTOs from these fields, the
/// firmware borrows a wire view via [`SoccerExtract::as_game`].
#[derive(Debug, Clone, PartialEq)]
// The variants intentionally differ in size and boxing is unavailable
// (no_std, no alloc); one extract exists per polled game.
#[allow(clippy::large_enum_variant)]
pub enum SoccerExtract {
    Pregame(PregameExtract),
    Live(LiveExtract),
    Final(FinalExtract),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PregameExtract {
    pub game_id: EText,
    /// Unix epoch seconds UTC, from `event.date`.
    pub start_time: u32,
    /// Stadium `venue.fullName`.
    pub venue: EText,
    pub away: PregameTeamExtract,
    pub home: PregameTeamExtract,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PregameTeamExtract {
    pub abbreviation: EText,
    pub colors: TeamColors,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveExtract {
    pub game_id: EText,
    /// Raw display-shaped clock (e.g. `"45'+6'"`) — JSON-DTO-only; the wire
    /// carries only [`Self::clock_seconds`].
    pub clock: EText,
    pub clock_seconds: u16,
    /// ESPN's raw period: halves 1/2, extra time 3/4, shootout 5 — passed
    /// through even outside that range, exactly like the backend.
    pub half: u8,
    pub on_break: bool,
    pub away: LiveTeamExtract,
    pub home: LiveTeamExtract,
    pub last_event: Option<LastEventExtract>,
    /// Filled by the summary pass; `None` on the scoreboard-only path (the
    /// committed goldens encode this shape).
    pub commentary: Option<CommentaryExtract>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveTeamExtract {
    pub abbreviation: EText,
    /// JSON carries `u32`; the wire saturates to `u16` at encode.
    pub score: u32,
    pub colors: TeamColors,
}

/// The most recent goal or red card.
#[derive(Debug, Clone, PartialEq)]
pub struct LastEventExtract {
    pub kind: EventKind,
    pub side: Option<Side>,
    /// Display-shaped event clock, e.g. `"90'+3'"`.
    pub clock: EText,
    /// Athlete short name; empty when ESPN lists none.
    pub athlete: EText,
    /// JSON-DTO-only composed line ("Goal - R. Lukaku"); no wire slot.
    pub text: EText,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinalExtract {
    pub game_id: EText,
    pub flavor: FinalFlavor,
    pub away: FinalTeamExtract,
    pub home: FinalTeamExtract,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinalTeamExtract {
    pub abbreviation: EText,
    pub score: u32,
    pub colors: TeamColors,
    /// Pre-formatted scorer list ("M. Merino 90'+1', …"); empty when the side
    /// didn't score — still encoded (the field is never omitted).
    pub scorers: EText,
}

/// One commentary line from the summary endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentaryExtract {
    /// The ESPN sequence number re-stringified — the firmware's
    /// change-detection key.
    pub id: heapless::String<SEQ_BYTES>,
    pub text: EText,
}

impl SoccerExtract {
    /// A borrowed wire-shaped view over the extract's own storage.
    pub fn as_game(&self) -> WireGame<'_> {
        match self {
            SoccerExtract::Pregame(game) => WireGame::Pregame(WirePregame {
                game_id: game.game_id.as_str(),
                start_time: game.start_time,
                venue: game.venue.as_str(),
                away: WirePregameTeam {
                    abbreviation: game.away.abbreviation.as_str(),
                    colors: game.away.colors,
                },
                home: WirePregameTeam {
                    abbreviation: game.home.abbreviation.as_str(),
                    colors: game.home.colors,
                },
            }),
            SoccerExtract::Live(game) => WireGame::Live(WireLive {
                game_id: game.game_id.as_str(),
                half: game.half,
                clock_seconds: game.clock_seconds,
                on_break: game.on_break,
                away: TeamState {
                    abbreviation: game.away.abbreviation.as_str(),
                    score: saturate_score(game.away.score),
                    colors: game.away.colors,
                },
                home: TeamState {
                    abbreviation: game.home.abbreviation.as_str(),
                    score: saturate_score(game.home.score),
                    colors: game.home.colors,
                },
                last_event: game.last_event.as_ref().map(|event| WireEvent {
                    kind: event.kind,
                    side: event.side,
                    clock: event.clock.as_str(),
                    athlete: event.athlete.as_str(),
                }),
                commentary: game.commentary.as_ref().map(|commentary| WireCommentary {
                    id: commentary.id.as_str(),
                    text: commentary.text.as_str(),
                }),
            }),
            SoccerExtract::Final(game) => WireGame::Final(WireFinal {
                game_id: game.game_id.as_str(),
                flavor: game.flavor,
                away: WireFinalTeam {
                    abbreviation: game.away.abbreviation.as_str(),
                    score: saturate_score(game.away.score),
                    colors: game.away.colors,
                    scorers: game.away.scorers.as_str(),
                },
                home: WireFinalTeam {
                    abbreviation: game.home.abbreviation.as_str(),
                    score: saturate_score(game.home.score),
                    colors: game.home.colors,
                    scorers: game.home.scorers.as_str(),
                },
            }),
        }
    }

    /// Attach (or clear) the summary pass's commentary. A no-op for pregame
    /// and final extracts — the backend only fetches the summary for live
    /// games, and only the live payload has a commentary slot.
    pub fn set_commentary(&mut self, commentary: Option<CommentaryExtract>) {
        if let SoccerExtract::Live(game) = self {
            game.commentary = commentary;
        }
    }
}

// ---------------------------------------------------------------- semantics

/// The backend's `parse_display_clock`, byte for byte: floor minutes × 60,
/// stoppage segments summed, all-or-nothing (one unparseable `+`-segment
/// poisons the whole string), capped at `u16::MAX`. The fallback is the
/// numeric `status.clock` clamped to `[0, 65535]`; absent ⇒ 0.
pub fn parse_display_clock(display_clock: &str, numeric_fallback: Option<f64>) -> u16 {
    fn minutes(text: &str) -> Option<u32> {
        let mut sum: u32 = 0;
        for part in text.split('+') {
            let value = part.trim().trim_end_matches('\'').parse::<u32>().ok()?;
            // The backend's `Sum<Option<u32>>` would overflow-panic in debug;
            // saturating is byte-identical for anything below 71 582 788
            // minutes, i.e. everything.
            sum = sum.saturating_add(value);
        }
        Some(sum)
    }
    match minutes(display_clock) {
        Some(minutes) => minutes.saturating_mul(60).min(u16::MAX as u32) as u16,
        None => numeric_fallback.unwrap_or(0.0).clamp(0.0, u16::MAX as f64) as u16,
    }
}

/// Soccer's own live sub-state map (NOT `parse_live_phase`): the exact break
/// and active string sets from the backend's `is_break`. `"Shootout"` is
/// active play. Unknown labels degrade to active play with a quirk — the
/// state is never guessed.
fn is_break(desc: &Exact<DESC_BYTES>, quirks: &mut impl Quirks) -> bool {
    if !desc.seen {
        return false;
    }
    if !desc.over {
        match desc.text.as_str() {
            "Halftime" | "Extra Time Halftime" | "End of Regulation" | "End of Extra Time" => {
                return true;
            }
            "First Half" | "Second Half" | "In Progress" | "Overtime" | "Shootout" => {
                return false;
            }
            _ => {}
        }
    }
    quirks.quirk(Quirk::UnknownBreakDescription);
    false
}

/// Post-state description → wire flavor byte; unknown degrades to full time
/// with a quirk.
fn final_flavor(desc: &Exact<DESC_BYTES>, quirks: &mut impl Quirks) -> FinalFlavor {
    if !desc.seen {
        return FinalFlavor::FullTime;
    }
    if !desc.over {
        match desc.text.as_str() {
            "Final Score - After Extra Time" => return FinalFlavor::AfterExtraTime,
            "Final Score - After Penalties" => return FinalFlavor::AfterPenalties,
            "Full Time" => return FinalFlavor::FullTime,
            _ => {}
        }
    }
    quirks.quirk(Quirk::UnknownFinalFlavor);
    FinalFlavor::FullTime
}

// ------------------------------------------------------------ text plumbing

/// Append with the wire's truncation semantics and report whether the whole
/// piece fit. Composing pieces with this and STOPPING at the first cut piece
/// yields exactly `truncate_utf8(concat(pieces), N)` — piece boundaries are
/// char boundaries of the concatenation, so the walk-down lands identically.
fn push_trunc<const N: usize>(dst: &mut heapless::String<N>, piece: &str) -> bool {
    let take = truncate_utf8(piece, N - dst.len());
    let _ = dst.push_str(take);
    take.len() == piece.len()
}

/// `truncate_utf8(concat(pieces), N)` without materializing the concatenation.
fn compose<const N: usize>(dst: &mut heapless::String<N>, pieces: &[&str]) {
    for piece in pieces {
        if !push_trunc(dst, piece) {
            break;
        }
    }
}

/// Bounded copy that REFUSES to truncate: parse inputs and compare keys must
/// fail loudly (matching the backend's parse failures on the same input)
/// rather than silently operate on a prefix.
#[derive(Debug, Clone, Default)]
struct Exact<const N: usize> {
    text: heapless::String<N>,
    seen: bool,
    over: bool,
}

impl<const N: usize> Exact<N> {
    fn set(&mut self, src: &str) {
        self.text.clear();
        self.seen = true;
        self.over = self.text.push_str(src).is_err();
    }

    fn valid(&self) -> Option<&str> {
        if self.seen && !self.over {
            Some(self.text.as_str())
        } else {
            None
        }
    }
}

/// Wire-bound string: copies with the shared `truncate_utf8` (ruling 2), so
/// truncate-at-copy is byte-identical to the backend's truncate-at-encode.
#[derive(Debug, Clone, Default)]
struct WireText {
    text: EText,
    seen: bool,
}

impl WireText {
    fn set(&mut self, src: &str) {
        set_text(&mut self.text, src);
        self.seen = true;
    }
}

// ------------------------------------------------------------- path tables

// Scoreboard body, relative to the document root.
const P_EVENTS: usize = 0;
const P_EVENT: usize = 1;
const P_ID: usize = 2;
const P_DATE: usize = 3;
const P_COMPS: usize = 4;
const P_COMP0: usize = 5;
const P_STATE: usize = 6;
const P_DESC: usize = 7;
const P_DCLOCK: usize = 8;
const P_NCLOCK: usize = 9;
const P_PERIOD: usize = 10;
const P_VENUE: usize = 11;
const P_VENUE_NAME: usize = 12;
const P_COMPETITOR: usize = 13;
const P_HOMEAWAY: usize = 14;
const P_SCORE: usize = 15;
const P_TEAM_ID: usize = 16;
const P_TEAM_ABBR: usize = 17;
const P_TEAM_COLOR: usize = 18;
const P_TEAM_ALT: usize = 19;
const P_DETAILS: usize = 20;
const P_DETAIL: usize = 21;
const P_DTYPE: usize = 22;
const P_DVALUE: usize = 23;
const P_DDISPLAY: usize = 24;
const P_DTEAM: usize = 25;
const P_DTEAM_ID: usize = 26;
const P_DSCORING: usize = 27;
const P_DRED: usize = 28;
const P_DATHLETES: usize = 29;
const P_DATHLETE: usize = 30;
const P_DATH_NAME: usize = 31;

/// Only `competitions[0]` is ever consumed (the backend reads exactly that);
/// the deliberately-unread corpus fields (`shootoutScore`, `attendance`,
/// `odds`, …) are skipped by construction — reproducing byte parity means
/// NOT reading them (inventory §6.1).
static SCOREBOARD_TABLE: &[Pattern] = &[
    /* P_EVENTS      */ &[Key("events")],
    /* P_EVENT       */ &[Key("events"), AnyIndex],
    /* P_ID          */ &[Key("events"), AnyIndex, Key("id")],
    /* P_DATE        */ &[Key("events"), AnyIndex, Key("date")],
    /* P_COMPS       */ &[Key("events"), AnyIndex, Key("competitions")],
    /* P_COMP0       */ &[Key("events"), AnyIndex, Key("competitions"), Index(0)],
    /* P_STATE       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("state")],
    /* P_DESC        */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("type"), Key("description")],
    /* P_DCLOCK      */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("displayClock")],
    /* P_NCLOCK      */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("clock")],
    /* P_PERIOD      */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("status"), Key("period")],
    /* P_VENUE       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("venue")],
    /* P_VENUE_NAME  */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("venue"), Key("fullName")],
    /* P_COMPETITOR  */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex],
    /* P_HOMEAWAY    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("homeAway")],
    /* P_SCORE       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("score")],
    /* P_TEAM_ID     */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("id")],
    /* P_TEAM_ABBR   */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("abbreviation")],
    /* P_TEAM_COLOR  */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("color")],
    /* P_TEAM_ALT    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("alternateColor")],
    /* P_DETAILS     */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details")],
    /* P_DETAIL      */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex],
    /* P_DTYPE       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("type"), Key("text")],
    /* P_DVALUE      */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("clock"), Key("value")],
    /* P_DDISPLAY    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("clock"), Key("displayValue")],
    /* P_DTEAM       */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("team")],
    /* P_DTEAM_ID    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("team"), Key("id")],
    /* P_DSCORING    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("scoringPlay")],
    /* P_DRED        */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("redCard")],
    /* P_DATHLETES   */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("athletesInvolved")],
    /* P_DATHLETE    */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("athletesInvolved"), AnyIndex],
    /* P_DATH_NAME   */
    &[Key("events"), AnyIndex, Key("competitions"), Index(0), Key("details"), AnyIndex, Key("athletesInvolved"), AnyIndex, Key("shortName")],
];

// Summary body: everything except `commentary[*].{sequence,text}` (boxscore,
// keyEvents, rosters, standings, article, …) is skipped by construction.
const S_COMMENTARY: usize = 0;
const S_ITEM: usize = 1;
const S_SEQ: usize = 2;
const S_TEXT: usize = 3;

static SUMMARY_TABLE: &[Pattern] = &[
    /* S_COMMENTARY */ &[Key("commentary")],
    /* S_ITEM       */ &[Key("commentary"), AnyIndex],
    /* S_SEQ        */ &[Key("commentary"), AnyIndex, Key("sequence")],
    /* S_TEXT       */ &[Key("commentary"), AnyIndex, Key("text")],
];

// ------------------------------------------------------------ event scratch

#[derive(Debug, Clone, Default)]
struct CompetitorScratch {
    home_away: Option<HomeAway>,
    score: Exact<SCORE_BYTES>,
    team_id: Exact<ID_BYTES>,
    abbreviation: WireText,
    color: Exact<COLOR_BYTES>,
    alternate: Exact<COLOR_BYTES>,
}

#[derive(Debug, Clone, Default)]
struct DetailScratch {
    active: bool,
    type_text: EText,
    type_seen: bool,
    value: f64,
    value_seen: bool,
    display: EText,
    display_seen: bool,
    team_present: bool,
    team_id: Exact<ID_BYTES>,
    scoring: bool,
    red: bool,
    /// First athlete's `shortName` (only `[0]` reaches any output).
    athlete: EText,
    athlete_elems: u16,
    cur_ath_active: bool,
    cur_ath_named: bool,
}

/// The running `max_by` winner among goals/red cards — replaced whenever the
/// incumbent does NOT strictly beat the newcomer, which is exactly
/// `Iterator::max_by`'s keep-the-LAST-of-equal-maxima fold.
#[derive(Debug, Clone)]
struct LastCandidate {
    value: f64,
    red: bool,
    team_id: Exact<ID_BYTES>,
    clock: EText,
    athlete: EText,
    type_text: EText,
}

/// One buffered scoring play for the final scorer lists. `text` is the
/// pre-composed `"{name} {displayValue}"` entry; side attribution waits for
/// finalize (competitor ids may stream after the details — ruling 4).
#[derive(Debug, Clone)]
struct ScorerRec {
    value: f64,
    team_id: Exact<ID_BYTES>,
    text: EText,
}

#[derive(Debug, Clone, Default)]
struct EventScratch {
    active: bool,
    skipped: bool,
    is_target: bool,
    /// A DU-tier violation was observed: the backend would have dropped this
    /// event in `parse_events` (and counted it failed).
    invalid: bool,
    id: WireText,
    date: Exact<DATE_BYTES>,
    competitions_present: bool,
    comp0_present: bool,
    state: Option<GameState>,
    desc: Exact<DESC_BYTES>,
    display_clock: WireText,
    numeric_clock: Option<f64>,
    period: Option<u8>,
    venue_present: bool,
    venue_name: WireText,
    competitor_count: u16,
    competitors: [CompetitorScratch; 2],
    detail: DetailScratch,
    last: Option<LastCandidate>,
    scorers: heapless::Vec<ScorerRec, SCORING_MAX>,
}

/// The DU tier: everything the backend's serde deserialize + `TryFrom` arm
/// requires before an event exists at all (ruling 1). An empty
/// `competitions` array deserializes cleanly — the event is "ok" but has
/// nothing to serve (list: filtered out; detail: `GameNotFound`).
fn du_ok(s: &EventScratch) -> bool {
    if s.invalid || !s.id.seen || !s.date.seen || !s.competitions_present {
        return false;
    }
    if !s.comp0_present {
        return true;
    }
    let Some(state) = s.state else {
        return false;
    };
    if s.competitor_count != 2 {
        return false;
    }
    for competitor in &s.competitors {
        if competitor.home_away.is_none()
            || !competitor.score.seen
            || !competitor.team_id.seen
            || !competitor.abbreviation.seen
            || !competitor.color.seen
            || !competitor.alternate.seen
        {
            return false;
        }
    }
    // A venue object without `fullName` fails deserialization in EVERY state.
    if s.venue_present && !s.venue_name.seen {
        return false;
    }
    match state {
        GameState::Pregame => s.venue_present && s.venue_name.seen,
        GameState::Live => s.display_clock.seen && s.period.is_some(),
        GameState::Final => true,
    }
}

// ---------------------------------------------------------------- transform

/// Transform-tier failure on the target event — the backend's 5xx class
/// (`AppError::EspnDeserialize` / invalid-color), distinct from the DU tier
/// that silently drops an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformError {
    /// `homeAway` markers were not exactly one home and one away.
    Sides,
    /// A team color failed the strip-`#`/exactly-6-hex-digits parse.
    Color,
    /// A competitor score failed the `u32` parse.
    Score,
    /// The pregame `event.date` failed the start-time parse.
    Date,
}

fn team_colors(competitor: &CompetitorScratch) -> Result<TeamColors, TransformError> {
    let primary = competitor
        .color
        .valid()
        .and_then(parse_hex_rgb)
        .ok_or(TransformError::Color)?;
    let alternate = competitor
        .alternate
        .valid()
        .and_then(parse_hex_rgb)
        .ok_or(TransformError::Color)?;
    Ok(TeamColors { primary, alternate })
}

fn team_score(competitor: &CompetitorScratch) -> Result<u32, TransformError> {
    competitor
        .score
        .valid()
        .and_then(|text| text.parse::<u32>().ok())
        .ok_or(TransformError::Score)
}

/// `detail_side`, order preserved: home is compared first (a pathological
/// home-id == away-id attributes home, like the backend).
fn side_of(
    team_id: &Exact<ID_BYTES>,
    home_id: Option<&str>,
    away_id: Option<&str>,
) -> Option<Side> {
    let id = team_id.valid()?;
    if Some(id) == home_id {
        Some(Side::Home)
    } else if Some(id) == away_id {
        Some(Side::Away)
    } else {
        None
    }
}

fn clock_cmp(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// One side's scorer list from the STABLE-sorted recs: filter by side, join
/// with `", "`, truncating exactly where encode would truncate the full join.
fn scorers_for(recs: &[ScorerRec], side_id: Option<&str>) -> EText {
    let mut out = EText::new();
    let side_id = match side_id {
        Some(id) => id,
        None => return out,
    };
    let mut first = true;
    for rec in recs {
        if rec.team_id.valid() != Some(side_id) {
            continue;
        }
        if !first && !push_trunc(&mut out, ", ") {
            break;
        }
        if !push_trunc(&mut out, rec.text.as_str()) {
            break;
        }
        first = false;
    }
    out
}

fn transform<Q: Quirks>(
    s: &mut EventScratch,
    quirks: &mut Q,
) -> Result<SoccerExtract, TransformError> {
    let marker_a = s.competitors[0].home_away.ok_or(TransformError::Sides)?;
    let marker_b = s.competitors[1].home_away.ok_or(TransformError::Sides)?;
    let (home_idx, away_idx) =
        order_home_away((marker_a, 0usize), (marker_b, 1usize)).ok_or(TransformError::Sides)?;
    let home = &s.competitors[home_idx];
    let away = &s.competitors[away_idx];

    let state = s.state.unwrap_or(GameState::Pregame); // DU guaranteed Some
    match state {
        GameState::Pregame => {
            let start_time = s
                .date
                .valid()
                .and_then(parse_start_time)
                .ok_or(TransformError::Date)?;
            Ok(SoccerExtract::Pregame(PregameExtract {
                game_id: s.id.text.clone(),
                start_time,
                venue: s.venue_name.text.clone(),
                away: PregameTeamExtract {
                    abbreviation: away.abbreviation.text.clone(),
                    colors: team_colors(away)?,
                },
                home: PregameTeamExtract {
                    abbreviation: home.abbreviation.text.clone(),
                    colors: team_colors(home)?,
                },
            }))
        }
        GameState::Live => {
            let home_id = home.team_id.valid();
            let away_id = away.team_id.valid();
            let last_event = s.last.as_ref().map(|cand| {
                let mut text = EText::new();
                if cand.athlete.is_empty() {
                    compose(&mut text, &[cand.type_text.as_str()]);
                } else {
                    compose(
                        &mut text,
                        &[cand.type_text.as_str(), " - ", cand.athlete.as_str()],
                    );
                }
                LastEventExtract {
                    kind: if cand.red {
                        EventKind::RedCard
                    } else {
                        EventKind::Goal
                    },
                    side: side_of(&cand.team_id, home_id, away_id),
                    clock: cand.clock.clone(),
                    athlete: cand.athlete.clone(),
                    text,
                }
            });
            Ok(SoccerExtract::Live(LiveExtract {
                game_id: s.id.text.clone(),
                clock: s.display_clock.text.clone(),
                clock_seconds: parse_display_clock(s.display_clock.text.as_str(), s.numeric_clock),
                half: s.period.unwrap_or(0), // DU guaranteed Some
                on_break: is_break(&s.desc, quirks),
                away: LiveTeamExtract {
                    abbreviation: away.abbreviation.text.clone(),
                    score: team_score(away)?,
                    colors: team_colors(away)?,
                },
                home: LiveTeamExtract {
                    abbreviation: home.abbreviation.text.clone(),
                    score: team_score(home)?,
                    colors: team_colors(home)?,
                },
                last_event,
                commentary: None,
            }))
        }
        GameState::Final => {
            let flavor = final_flavor(&s.desc, quirks);
            let mut away_team = FinalTeamExtract {
                abbreviation: away.abbreviation.text.clone(),
                score: team_score(away)?,
                colors: team_colors(away)?,
                scorers: EText::new(),
            };
            let mut home_team = FinalTeamExtract {
                abbreviation: home.abbreviation.text.clone(),
                score: team_score(home)?,
                colors: team_colors(home)?,
                scorers: EText::new(),
            };
            let home_id = home.team_id.valid();
            let away_id = away.team_id.valid();
            // STABLE insertion sort by clock value (adjacent swaps preserve
            // arrival order among equal keys) — `sort_by` parity; a
            // `sort_unstable` here breaks `final_after_penalties` bytes.
            let recs = s.scorers.as_mut_slice();
            let mut i = 1;
            while i < recs.len() {
                let mut j = i;
                while j > 0 && clock_cmp(recs[j - 1].value, recs[j].value) == Ordering::Greater {
                    recs.swap(j - 1, j);
                    j -= 1;
                }
                i += 1;
            }
            away_team.scorers = scorers_for(recs, away_id);
            home_team.scorers = scorers_for(recs, home_id);
            Ok(SoccerExtract::Final(FinalExtract {
                game_id: s.id.text.clone(),
                flavor,
                away: away_team,
                home: home_team,
            }))
        }
    }
}

// --------------------------------------------------------------- the sink

#[derive(Debug)]
// Carries the whole bounded extract; boxing is unavailable (no_std, no
// alloc) and exactly one of these exists per stream.
#[allow(clippy::large_enum_variant)]
enum DetailResult {
    Game(SoccerExtract),
    /// Target found but its `competitions` array is empty — the backend
    /// serves `GameNotFound` (404), same as an absent id on a clean parse.
    NoCompetition,
    Failed(TransformError),
}

#[derive(Debug)]
// The list vec dwarfs the detail arm; one Mode exists per stream and no_std
// forbids boxing it away.
#[allow(clippy::large_enum_variant)]
enum Mode {
    List {
        games: heapless::Vec<ListEntry, LIST_MAX>,
        overflowed: bool,
    },
    Detail {
        target: EText,
        done: bool,
        result: Option<DetailResult>,
    },
}

struct SoccerSink<Q: Quirks> {
    mode: Mode,
    scratch: EventScratch,
    quirks: Q,
    ok: u16,
    failed: u16,
    skipped: u16,
    /// The scoreboard shell itself was malformed (`events` not an array) —
    /// the backend's whole-body deserialize failure, a 502 before any event.
    body_bad: bool,
}

impl<Q: Quirks> SoccerSink<Q> {
    fn new(mode: Mode, quirks: Q) -> Self {
        Self {
            mode,
            scratch: EventScratch::default(),
            quirks,
            ok: 0,
            failed: 0,
            skipped: 0,
            body_bad: false,
        }
    }

    fn fold_detail(&mut self) {
        let s = &mut self.scratch;
        let d = &mut s.detail;
        if !d.active {
            return;
        }
        d.active = false;
        // Per-detail DU: type.text, clock.value, clock.displayValue required;
        // a `team` object requires `id`. Any violation drops the whole event.
        if !d.type_seen
            || !d.value_seen
            || !d.display_seen
            || (d.team_present && !d.team_id.seen)
        {
            s.invalid = true;
            return;
        }
        if d.scoring || d.red {
            let replace = match &s.last {
                Some(best) => clock_cmp(best.value, d.value) != Ordering::Greater,
                None => true,
            };
            if replace {
                s.last = Some(LastCandidate {
                    value: d.value,
                    red: d.red,
                    team_id: d.team_id.clone(),
                    clock: d.display.clone(),
                    athlete: d.athlete.clone(),
                    type_text: d.type_text.clone(),
                });
            }
        }
        // Scorer entries only ever come from scoring plays with a resolvable
        // team id (an id-less detail can never match a side).
        if d.scoring && d.team_id.valid().is_some() {
            let name: &str = if d.athlete_elems > 0 {
                d.athlete.as_str()
            } else {
                d.type_text.as_str()
            };
            let mut text = EText::new();
            compose(&mut text, &[name, " ", d.display.as_str()]);
            let _ = s.scorers.push(ScorerRec {
                value: d.value,
                team_id: d.team_id.clone(),
                text,
            });
        }
    }

    fn finalize_event(&mut self) {
        let scratch = &mut self.scratch;
        if !scratch.active {
            return;
        }
        scratch.active = false;
        if scratch.skipped {
            // A fast-forwarded event was not fully validated; one that had
            // already tripped a DU violation before its id is still failed.
            if scratch.invalid {
                self.failed = self.failed.saturating_add(1);
            } else {
                self.skipped = self.skipped.saturating_add(1);
            }
            return;
        }
        if !du_ok(scratch) {
            self.failed = self.failed.saturating_add(1);
            return;
        }
        self.ok = self.ok.saturating_add(1);
        match &mut self.mode {
            Mode::List { games, overflowed } => {
                if scratch.comp0_present {
                    if let Some(state) = scratch.state {
                        let entry = ListEntry {
                            state,
                            id: scratch.id.text.clone(),
                        };
                        if games.push(entry).is_err() {
                            *overflowed = true;
                        }
                    }
                }
            }
            Mode::Detail { done, result, .. } => {
                if scratch.is_target && !*done {
                    *done = true;
                    *result = Some(if scratch.comp0_present {
                        match transform(scratch, &mut self.quirks) {
                            Ok(game) => DetailResult::Game(game),
                            Err(error) => DetailResult::Failed(error),
                        }
                    } else {
                        DetailResult::NoCompetition
                    });
                }
            }
        }
    }
}

impl<Q: Quirks> Sink for SoccerSink<Q> {
    fn value(&mut self, pattern: usize, indices: &[u16], value: Value<'_>) -> Directive {
        let s = &mut self.scratch;
        match pattern {
            // `events` present but not an array (null, scalar): the backend's
            // shell deserialize fails and the whole request is upstream-error.
            P_EVENTS => self.body_bad = true,
            // A scalar events[] element parses as no event at all.
            P_EVENT => self.failed = self.failed.saturating_add(1),
            P_ID => match value {
                Value::Str(text) => {
                    s.id.set(text);
                    if let Mode::Detail { target, done, .. } = &self.mode {
                        if !*done && text == target.as_str() {
                            s.is_target = true;
                        } else {
                            s.skipped = true;
                            return Directive::SkipElement;
                        }
                    }
                }
                _ => s.invalid = true,
            },
            P_DATE => match value {
                Value::Str(text) => s.date.set(text),
                _ => s.invalid = true,
            },
            // Required containers that arrived as scalars/null.
            P_COMPS | P_COMP0 | P_DETAILS | P_DATHLETES | P_DETAIL | P_DATHLETE => {
                s.invalid = true;
            }
            P_STATE => match value {
                Value::Str("pre") => s.state = Some(GameState::Pregame),
                Value::Str("in") => s.state = Some(GameState::Live),
                Value::Str("post") => s.state = Some(GameState::Final),
                _ => s.invalid = true, // strict three-value enum
            },
            P_DESC => match value {
                Value::Str(text) => s.desc.set(text),
                Value::Null => {} // Option<String>
                _ => s.invalid = true,
            },
            P_DCLOCK => match value {
                Value::Str(text) => s.display_clock.set(text),
                Value::Null => {} // Option<String>
                _ => s.invalid = true,
            },
            P_NCLOCK => match value {
                Value::Num(text) => match text.parse::<f64>() {
                    Ok(clock) => s.numeric_clock = Some(clock),
                    Err(_) => s.invalid = true,
                },
                Value::Null => {} // Option<f64>
                _ => s.invalid = true,
            },
            P_PERIOD => match value {
                Value::Num(text) => match text.parse::<u8>() {
                    Ok(period) => s.period = Some(period),
                    // Fractional/out-of-range fails serde's u8 the same way.
                    Err(_) => s.invalid = true,
                },
                Value::Null => {} // Option<u8>
                _ => s.invalid = true,
            },
            P_VENUE => match value {
                Value::Null => {} // Option<EspnVenue>
                _ => s.invalid = true,
            },
            P_VENUE_NAME => match value {
                Value::Str(text) => s.venue_name.set(text),
                _ => s.invalid = true,
            },
            // A scalar competitor element: count it (the len==2 rule) and fail.
            P_COMPETITOR => {
                s.competitor_count = s.competitor_count.saturating_add(1);
                s.invalid = true;
            }
            P_HOMEAWAY | P_SCORE | P_TEAM_ID | P_TEAM_ABBR | P_TEAM_COLOR | P_TEAM_ALT => {
                let slot = indices[1] as usize;
                if slot >= 2 {
                    // Extra competitors already fail the len==2 rule; their
                    // fields have nowhere to go.
                    return Directive::Continue;
                }
                let competitor = &mut s.competitors[slot];
                match (pattern, value) {
                    (P_HOMEAWAY, Value::Str(text)) => match HomeAway::parse(text) {
                        Some(marker) => competitor.home_away = Some(marker),
                        None => s.invalid = true, // strict enum
                    },
                    (P_SCORE, Value::Str(text)) => competitor.score.set(text),
                    (P_TEAM_ID, Value::Str(text)) => competitor.team_id.set(text),
                    (P_TEAM_ABBR, Value::Str(text)) => competitor.abbreviation.set(text),
                    (P_TEAM_COLOR, Value::Str(text)) => competitor.color.set(text),
                    (P_TEAM_ALT, Value::Str(text)) => competitor.alternate.set(text),
                    _ => s.invalid = true,
                }
            }
            P_DTYPE => match value {
                Value::Str(text) => {
                    set_text(&mut s.detail.type_text, text);
                    s.detail.type_seen = true;
                }
                _ => s.invalid = true,
            },
            P_DVALUE => match value {
                Value::Num(text) => match text.parse::<f64>() {
                    Ok(clock) => {
                        s.detail.value = clock;
                        s.detail.value_seen = true;
                    }
                    Err(_) => s.invalid = true,
                },
                _ => s.invalid = true, // f64 required, null included
            },
            P_DDISPLAY => match value {
                Value::Str(text) => {
                    set_text(&mut s.detail.display, text);
                    s.detail.display_seen = true;
                }
                _ => s.invalid = true,
            },
            P_DTEAM => match value {
                Value::Null => {} // Option<EspnDetailTeam>
                _ => s.invalid = true,
            },
            P_DTEAM_ID => match value {
                Value::Str(text) => s.detail.team_id.set(text),
                _ => s.invalid = true,
            },
            P_DSCORING => match value {
                Value::Bool(flag) => s.detail.scoring = flag,
                // serde(default) covers ABSENT only; an explicit null fails.
                _ => s.invalid = true,
            },
            P_DRED => match value {
                Value::Bool(flag) => s.detail.red = flag,
                _ => s.invalid = true,
            },
            P_DATH_NAME => match value {
                Value::Str(text) => {
                    s.detail.cur_ath_named = true;
                    if indices[2] == 0 {
                        set_text(&mut s.detail.athlete, text);
                    }
                }
                _ => s.invalid = true,
            },
            _ => {}
        }
        Directive::Continue
    }

    fn enter(&mut self, pattern: usize, _indices: &[u16]) -> Directive {
        let s = &mut self.scratch;
        match pattern {
            P_EVENT => {
                *s = EventScratch::default();
                s.active = true;
            }
            P_COMPS => s.competitions_present = true,
            P_COMP0 => s.comp0_present = true,
            P_VENUE => s.venue_present = true,
            P_COMPETITOR => s.competitor_count = s.competitor_count.saturating_add(1),
            P_DETAIL => {
                s.detail = DetailScratch::default();
                s.detail.active = true;
            }
            P_DTEAM => s.detail.team_present = true,
            P_DATHLETE => {
                s.detail.athlete_elems = s.detail.athlete_elems.saturating_add(1);
                s.detail.cur_ath_active = true;
                s.detail.cur_ath_named = false;
            }
            _ => {}
        }
        Directive::Continue
    }

    fn leave(&mut self, pattern: usize, _indices: &[u16]) -> Directive {
        match pattern {
            P_EVENT => self.finalize_event(),
            P_DETAIL => self.fold_detail(),
            P_DATHLETE => {
                let d = &mut self.scratch.detail;
                if d.cur_ath_active {
                    d.cur_ath_active = false;
                    // Every athlete element requires `shortName`, not just [0].
                    if !d.cur_ath_named {
                        self.scratch.invalid = true;
                    }
                }
            }
            _ => {}
        }
        Directive::Continue
    }
}

// ------------------------------------------------------------- public API

/// Extraction failure past the JSON stream itself.
#[derive(Debug, PartialEq)]
pub enum ExtractError {
    /// The tokenizer/engine rejected the input.
    Stream(Error),
    /// The scoreboard shell was malformed (`events` not an array) — the
    /// backend's whole-body deserialize failure (502 before any event).
    MalformedBody,
    /// The target event was found but failed the transform tier (502).
    Transform(TransformError),
}

impl From<Error> for ExtractError {
    fn from(error: Error) -> Self {
        ExtractError::Stream(error)
    }
}

/// Outcome of a detail extraction.
#[derive(Debug, PartialEq)]
// Found carries the whole bounded extract; no_std forbids boxing it away.
#[allow(clippy::large_enum_variant)]
pub enum GameOutcome {
    Found(SoccerExtract),
    /// The target id was absent (or its event carried no competition).
    /// `failed` counts events that provably fail the backend's per-event
    /// parse — the 404-vs-502 rule (ruling 13): only `failed == 0` may map
    /// to "game ended". `skipped` events were fast-forwarded at their id
    /// without full validation (the list extractor validates everything).
    NotFound { ok: u16, failed: u16, skipped: u16 },
}

/// Streams one league scoreboard body and extracts the target event,
/// fast-forwarding every other event at its `id` (`SkipElement`).
pub struct GameExtractor<'scratch, Q: Quirks> {
    matcher: StreamMatcher<'static, 'scratch, SoccerSink<Q>>,
}

impl<'scratch, Q: Quirks> GameExtractor<'scratch, Q> {
    /// `scratch` must hold the longest contiguous string token in the body
    /// (venue names and play texts — kilobytes, not megabytes).
    pub fn new(
        target_id: &str,
        quirks: Q,
        scratch: &'scratch mut [u8],
    ) -> Result<Self, Error> {
        let mut target = EText::new();
        set_text(&mut target, target_id);
        let sink = SoccerSink::new(
            Mode::Detail {
                target,
                done: false,
                result: None,
            },
            quirks,
        );
        Ok(Self {
            matcher: StreamMatcher::new(SCOREBOARD_TABLE, sink, scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.matcher.write(chunk)
    }

    /// Finish the document; hands the quirk receiver back alongside the
    /// outcome (it is only reachable on the Ok path — transform failures are
    /// terminal 5xx, not diagnostics).
    pub fn finish(self) -> Result<(GameOutcome, Q), ExtractError> {
        let sink = self.matcher.finish()?;
        if sink.body_bad {
            return Err(ExtractError::MalformedBody);
        }
        let (ok, failed, skipped) = (sink.ok, sink.failed, sink.skipped);
        match sink.mode {
            Mode::Detail {
                result: Some(DetailResult::Game(game)),
                ..
            } => Ok((GameOutcome::Found(game), sink.quirks)),
            Mode::Detail {
                result: Some(DetailResult::Failed(error)),
                ..
            } => Err(ExtractError::Transform(error)),
            _ => Ok((GameOutcome::NotFound { ok, failed, skipped }, sink.quirks)),
        }
    }
}

/// One games-list entry: the cross-sport `(state, id)` pair.
#[derive(Debug, Clone, PartialEq)]
pub struct ListEntry {
    pub state: GameState,
    pub id: EText,
}

/// The games list plus the counts `find_event` semantics need (ruling 13).
#[derive(Debug)]
pub struct GamesList {
    /// DU-clean events with a competition, in body order.
    pub games: heapless::Vec<ListEntry, LIST_MAX>,
    /// Events that parsed clean (including competition-less ones).
    pub ok: u16,
    /// Events the backend's per-event parse would have dropped.
    pub failed: u16,
    /// More than [`LIST_MAX`] listable games arrived; the excess was dropped.
    pub overflowed: bool,
}

/// Streams one league scoreboard body into the games list. Unlike
/// [`GameExtractor`] it never skips, so `failed` is exact — this is the
/// authoritative count for the 404-vs-502 rule.
pub struct ListExtractor<'scratch> {
    matcher: StreamMatcher<'static, 'scratch, SoccerSink<IgnoreQuirks>>,
}

impl<'scratch> ListExtractor<'scratch> {
    pub fn new(scratch: &'scratch mut [u8]) -> Result<Self, Error> {
        let sink = SoccerSink::new(
            Mode::List {
                games: heapless::Vec::new(),
                overflowed: false,
            },
            IgnoreQuirks,
        );
        Ok(Self {
            matcher: StreamMatcher::new(SCOREBOARD_TABLE, sink, scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.matcher.write(chunk)
    }

    pub fn finish(self) -> Result<GamesList, ExtractError> {
        let sink = self.matcher.finish()?;
        if sink.body_bad {
            return Err(ExtractError::MalformedBody);
        }
        let (ok, failed) = (sink.ok, sink.failed);
        match sink.mode {
            Mode::List { games, overflowed } => Ok(GamesList {
                games,
                ok,
                failed,
                overflowed,
            }),
            // Unreachable by construction (the constructor sets List mode).
            Mode::Detail { .. } => Ok(GamesList {
                games: heapless::Vec::new(),
                ok,
                failed,
                overflowed: false,
            }),
        }
    }
}

// ---------------------------------------------------------------- summary

#[derive(Default)]
struct ItemScratch {
    active: bool,
    sequence: Option<u32>,
    text: EText,
    text_seen: bool,
}

#[derive(Default)]
struct SummarySink {
    /// The summary deserialize would have failed — the backend degrades the
    /// WHOLE summary to no-commentary (warn), even if other items were fine.
    poisoned: bool,
    item: ItemScratch,
    best: Option<(u32, EText)>,
}

impl SummarySink {
    fn fold_item(&mut self) {
        if !self.item.active {
            return;
        }
        self.item.active = false;
        match (self.item.sequence, self.item.text_seen) {
            (Some(sequence), true) => {
                // `max_by_key` keeps the LAST of equal maxima.
                let replace = match &self.best {
                    Some((best, _)) => sequence >= *best,
                    None => true,
                };
                if replace {
                    self.best = Some((sequence, self.item.text.clone()));
                }
            }
            _ => self.poisoned = true, // both fields required per item
        }
    }
}

impl Sink for SummarySink {
    fn value(&mut self, pattern: usize, _indices: &[u16], value: Value<'_>) -> Directive {
        match pattern {
            // `commentary` null/scalar, or a scalar item: deserialize failure.
            S_COMMENTARY | S_ITEM => self.poisoned = true,
            S_SEQ => match value {
                // A genuine JSON number, `u32`: non-integer or negative fails
                // the whole summary, exactly like serde.
                Value::Num(text) => match text.parse::<u32>() {
                    Ok(sequence) => self.item.sequence = Some(sequence),
                    Err(_) => self.poisoned = true,
                },
                _ => self.poisoned = true,
            },
            S_TEXT => match value {
                Value::Str(text) => {
                    // Raw, untrimmed; the only cap is the shared 255-byte
                    // wire truncation (which cannot turn non-empty empty).
                    set_text(&mut self.item.text, text);
                    self.item.text_seen = true;
                }
                _ => self.poisoned = true,
            },
            _ => {}
        }
        Directive::Continue
    }

    fn enter(&mut self, pattern: usize, _indices: &[u16]) -> Directive {
        if pattern == S_ITEM {
            self.item = ItemScratch {
                active: true,
                ..ItemScratch::default()
            };
        }
        Directive::Continue
    }

    fn leave(&mut self, pattern: usize, _indices: &[u16]) -> Directive {
        if pattern == S_ITEM {
            self.fold_item();
        }
        Directive::Continue
    }
}

/// What the summary pass produced. `malformed` mirrors the backend's
/// deserialize-failure warn: the caller serves the live payload without
/// commentary either way (best-effort by construction).
#[derive(Debug, PartialEq)]
pub struct SummaryOutcome {
    pub commentary: Option<CommentaryExtract>,
    pub malformed: bool,
}

/// Streams one per-event summary body (390–456 KB live) down to its single
/// wire-relevant field: the highest-sequence commentary line. Selection is
/// `max_by_key(sequence)` THEN `filter(!text.is_empty())` — an empty highest
/// collapses to `None` with no fall-through to the next non-empty line.
pub struct SummaryExtractor<'scratch> {
    matcher: StreamMatcher<'static, 'scratch, SummarySink>,
}

impl<'scratch> SummaryExtractor<'scratch> {
    pub fn new(scratch: &'scratch mut [u8]) -> Result<Self, Error> {
        Ok(Self {
            matcher: StreamMatcher::new(SUMMARY_TABLE, SummarySink::default(), scratch)?,
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.matcher.write(chunk)
    }

    pub fn finish(self) -> Result<SummaryOutcome, Error> {
        let sink = self.matcher.finish()?;
        if sink.poisoned {
            return Ok(SummaryOutcome {
                commentary: None,
                malformed: true,
            });
        }
        let commentary = sink
            .best
            .filter(|(_, text)| !text.is_empty())
            .map(|(sequence, text)| {
                let mut id = heapless::String::<SEQ_BYTES>::new();
                // u32::MAX is 10 digits; SEQ_BYTES fits every value.
                let _ = write!(id, "{sequence}");
                CommentaryExtract { id, text }
            });
        Ok(SummaryOutcome {
            commentary,
            malformed: false,
        })
    }
}

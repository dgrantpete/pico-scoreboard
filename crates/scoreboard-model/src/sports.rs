//! The per-sport commits: wire game in, snapshot view out.
//!
//! Each one is the Rust body of a `state.py` setter, minus the parts that
//! turned out to be pixels (see [`crate::snapshot`] for where the line is).

use scoreboard_wire::{FinalTeam, LivePhase, Side, football, mlb, nba, soccer};

use crate::color::Rgb888;
use crate::feed::{GameDetail, LeagueId};
use crate::snapshot::{
    Bases, CLOCK, FieldSituation, LINESCORE, Millis, Mode, Record, SHORT, Sport,
};
use crate::store::{Logos, Store};
use crate::text::{Text, push_folded, push_folded_upper, set_folded, set_plain, write_text};

/// The device's notion of local time, for the pregame first-pitch line.
///
/// `utc_offset_s` of `None` is *not* the same as `Some(0)`: a device that has
/// never synced omits the time entirely rather than show one from the wrong
/// timezone, and UTC itself is a legitimate offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalClock {
    /// Wall clock, unix seconds UTC.
    pub now_epoch_s: u32,
    pub utc_offset_s: Option<i32>,
}

/// The line-score final, as the three sports that share the screen supply it.
#[derive(Debug, Clone, Copy)]
pub struct LinescoreFinal<'a> {
    pub sport: Sport,
    pub game_id: &'a str,
    /// Innings or quarters played — the column count, and what decides whether
    /// the banner reads "FINAL", "F/10" or "F/OT".
    pub periods: u8,
    pub away: FinalTeam<'a>,
    pub home: FinalTeam<'a>,
}

/// The sport-neutral pregame screen, assembled per sport.
///
/// The MicroPython models reached this screen by duck-typing: three of the
/// four sports built a `PregameGame` whose `venue` held a league name and
/// whose `weather_condition` held a stadium, so `set_pregame` could stay one
/// function. That works but it means reading `soccer.PregameGame` to learn
/// what "venue" contains. The slots are named for their role on the panel
/// instead, and the four constructors below say plainly what each sport puts
/// where. The pixels are identical.
#[derive(Debug, Clone, Copy)]
pub struct PregameInput<'a> {
    pub sport: Sport,
    pub game_id: &'a str,
    /// Unix epoch seconds, UTC.
    pub start_time: u32,
    /// First phase of the info cycle.
    pub info_primary: &'a str,
    /// Second phase; empty drops the phase from the cycle.
    pub info_secondary: &'a str,
    /// Prefixed to `info_secondary` as "72F " when present.
    pub temperature: Option<u8>,
    pub away: PregameSideInput<'a>,
    pub home: PregameSideInput<'a>,
}

#[derive(Debug, Clone, Copy)]
pub struct PregameSideInput<'a> {
    pub abbreviation: &'a str,
    pub record: Option<Record>,
    /// The per-team line under the divider.
    pub line: &'a str,
    pub color: Rgb888,
}

fn record(record: Option<scoreboard_wire::Record>) -> Option<Record> {
    record.map(|r| Record {
        wins: r.wins,
        losses: r.losses,
    })
}

impl<'a> PregameInput<'a> {
    /// MLB: real stadium, real weather, probable starters.
    pub fn mlb(game: &mlb::Pregame<'a>) -> Self {
        let (temperature, condition) = match game.weather {
            Some(weather) => (Some(weather.temperature), weather.condition),
            None => (None, ""),
        };
        Self {
            sport: Sport::Mlb,
            game_id: game.game_id,
            start_time: game.start_time,
            info_primary: game.venue,
            info_secondary: condition,
            temperature,
            away: PregameSideInput {
                abbreviation: game.away.abbreviation,
                record: record(game.away.record),
                line: game.away.probable_pitcher.unwrap_or(""),
                color: game.away.colors.into(),
            },
            home: PregameSideInput {
                abbreviation: game.home.abbreviation,
                record: record(game.home.record),
                line: game.home.probable_pitcher.unwrap_or(""),
                color: game.home.colors.into(),
            },
        }
    }

    /// NBA: real stadium and records; no weather, no per-team line. The two
    /// empty slots are simply absent, which the screen already handles.
    pub fn nba(game: &nba::Pregame<'a>) -> Self {
        Self {
            sport: Sport::Nba,
            game_id: game.game_id,
            start_time: game.start_time,
            info_primary: game.venue,
            info_secondary: "",
            temperature: None,
            away: PregameSideInput {
                abbreviation: game.away.abbreviation,
                record: record(game.away.record),
                line: "",
                color: game.away.colors.into(),
            },
            home: PregameSideInput {
                abbreviation: game.home.abbreviation,
                record: record(game.home.record),
                line: "",
                color: game.home.colors.into(),
            },
        }
    }

    /// Football: the league leads the info cycle (the wire does not carry it —
    /// the device knows which endpoint it polled), the stadium follows, and
    /// the per-team line is the college rank ("#3 OHIO STATE") when there is
    /// one.
    pub fn football(game: &football::Pregame<'a>, league_name: &'a str) -> Self {
        Self {
            sport: Sport::Football,
            game_id: game.game_id,
            start_time: game.start_time,
            info_primary: league_name,
            info_secondary: game.venue,
            temperature: None,
            away: PregameSideInput {
                abbreviation: game.away.abbreviation,
                record: record(game.away.record),
                line: game.away.rank_line.unwrap_or(""),
                color: game.away.colors.into(),
            },
            home: PregameSideInput {
                abbreviation: game.home.abbreviation,
                record: record(game.home.record),
                line: game.home.rank_line.unwrap_or(""),
                color: game.home.colors.into(),
            },
        }
    }

    /// Soccer: league then stadium, no records, and the per-team line is the
    /// club's abbreviation — the lower half of the screen would otherwise be
    /// empty.
    pub fn soccer(game: &soccer::Pregame<'a>, league_name: &'a str) -> Self {
        Self {
            sport: Sport::Soccer,
            game_id: game.game_id,
            start_time: game.start_time,
            info_primary: league_name,
            info_secondary: game.venue,
            temperature: None,
            away: PregameSideInput {
                abbreviation: game.away.abbreviation,
                record: None,
                line: game.away.abbreviation,
                color: game.away.colors.into(),
            },
            home: PregameSideInput {
                abbreviation: game.home.abbreviation,
                record: None,
                line: game.home.abbreviation,
                color: game.home.colors.into(),
            },
        }
    }
}

impl Store {
    /// The poller's one call: commit whatever came back for the current game.
    ///
    /// Dispatches on the payload's state, the way `poller._poll_current` does,
    /// so a pregame card that flips to live mid-view goes live on the spot
    /// instead of waiting for the next rotation.
    ///
    /// The live arms also stage the shared play flash. Only MLB always carries
    /// a play; the others' is absent before the opening snap or tip, and an
    /// absent one leaves the previous line in place rather than clearing it —
    /// so a play that reappears unchanged cannot flash twice.
    pub fn commit_detail(
        &mut self,
        league: &LeagueId,
        detail: &GameDetail<'_>,
        logos: Logos,
        now_ms: Millis,
        clock: LocalClock,
    ) {
        let league_name = league.display_name.as_str();
        match detail {
            GameDetail::Mlb(mlb::Game::Live(game)) => {
                self.commit_mlb_live(game, logos, now_ms);
                self.flash_play(game.last_play.id, game.last_play.text, now_ms);
            }
            GameDetail::Mlb(mlb::Game::Pregame(game)) => {
                self.commit_pregame(&PregameInput::mlb(game), logos, now_ms, clock);
            }
            GameDetail::Mlb(mlb::Game::Final(game)) => self.commit_mlb_final(game, logos, now_ms),
            GameDetail::Nba(nba::Game::Live(game)) => {
                self.commit_nba_live(game, logos, now_ms);
                if let Some(play) = game.last_play {
                    self.flash_play(play.id, play.text, now_ms);
                }
            }
            GameDetail::Nba(nba::Game::Pregame(game)) => {
                self.commit_pregame(&PregameInput::nba(game), logos, now_ms, clock);
            }
            GameDetail::Nba(nba::Game::Final(game)) => self.commit_nba_final(game, logos, now_ms),
            GameDetail::Football(football::Game::Live(game)) => {
                self.commit_football_live(game, logos, now_ms);
                if let Some(play) = game.last_play {
                    self.flash_play(play.id, play.text, now_ms);
                }
            }
            GameDetail::Football(football::Game::Pregame(game)) => {
                self.commit_pregame(
                    &PregameInput::football(game, league_name),
                    logos,
                    now_ms,
                    clock,
                );
            }
            GameDetail::Football(football::Game::Final(game)) => {
                self.commit_football_final(game, logos, now_ms);
            }
            GameDetail::Soccer(soccer::Game::Live(game)) => {
                self.commit_soccer_live(game, logos, now_ms);
                if let Some(commentary) = game.commentary {
                    self.flash_play(commentary.id, commentary.text, now_ms);
                }
            }
            GameDetail::Soccer(soccer::Game::Pregame(game)) => {
                self.commit_pregame(
                    &PregameInput::soccer(game, league_name),
                    logos,
                    now_ms,
                    clock,
                );
            }
            GameDetail::Soccer(soccer::Game::Final(game)) => {
                self.commit_soccer_final(game, logos, now_ms);
            }
        }
    }

    // -- MLB ---------------------------------------------------------------

    pub fn commit_mlb_live(&mut self, game: &mlb::Live<'_>, logos: Logos, now_ms: Millis) {
        self.begin_game(
            Mode::MlbLive,
            game.game_id,
            game.away.abbreviation,
            game.home.abbreviation,
            logos,
            now_ms,
        );
        let away_color = Rgb888::from(game.away.colors);
        let home_color = Rgb888::from(game.home.colors);
        let view = &mut self.snapshot_mut().mlb_live;
        view.half = game.inning.half;
        set_ordinal(&mut view.inning_text, game.inning.number);
        view.away_score = game.away.score;
        view.home_score = game.home.score;
        view.balls = game.count.balls;
        view.strikes = game.count.strikes;
        view.outs = game.count.outs;
        view.bases = Bases {
            first: game.bases.first,
            second: game.bases.second,
            third: game.bases.third,
        };

        // Between halves nobody is batting: the count dots and base markers
        // render dim rather than in a stale team's color.
        let batting = match game.inning.half {
            mlb::InningHalf::Top => Some((away_color, home_color)),
            mlb::InningHalf::Bottom => Some((home_color, away_color)),
            mlb::InningHalf::Middle | mlb::InningHalf::End => None,
        };
        view.bat_color = batting.map(|(bat, _)| bat);
        view.pitch_color = batting.map(|(_, pitch)| pitch);

        view.has_at_bat = game.at_bat.is_some();
        match game.at_bat {
            Some(at_bat) => {
                set_folded(&mut view.pitcher, at_bat.pitcher);
                set_folded(&mut view.batter, at_bat.batter);
            }
            None => {
                view.pitcher.clear();
                view.batter.clear();
            }
        }
        self.finish_game();
    }

    pub fn commit_mlb_final(&mut self, game: &mlb::Final<'_>, logos: Logos, now_ms: Millis) {
        self.commit_linescore_final(
            &LinescoreFinal {
                sport: Sport::Mlb,
                game_id: game.game_id,
                periods: game.innings_played,
                away: game.away,
                home: game.home,
            },
            logos,
            now_ms,
        );
    }

    // -- NBA ---------------------------------------------------------------

    pub fn commit_nba_live(&mut self, game: &nba::Live<'_>, logos: Logos, now_ms: Millis) {
        self.begin_game(
            Mode::NbaLive,
            game.game_id,
            game.away.abbreviation,
            game.home.abbreviation,
            logos,
            now_ms,
        );
        let view = &mut self.snapshot_mut().nba_live;
        view.away_score = game.away.score;
        view.home_score = game.home.score;
        set_break_clock(
            &mut view.phase_text,
            &mut view.clock_text,
            &mut view.clock_accent,
            game.phase,
            game.period,
            game.clock,
        );
        // A stop-clock under a minute reads "53.0" — no colon. That shape is
        // the only crunch-time signal ESPN gives.
        view.clock_low = game.phase == LivePhase::InProgress && !game.clock.contains(':');
        self.finish_game();
    }

    pub fn commit_nba_final(&mut self, game: &nba::Final<'_>, logos: Logos, now_ms: Millis) {
        self.commit_linescore_final(
            &LinescoreFinal {
                sport: Sport::Nba,
                game_id: game.game_id,
                periods: game.periods_played,
                away: game.away,
                home: game.home,
            },
            logos,
            now_ms,
        );
    }

    // -- Football ----------------------------------------------------------

    pub fn commit_football_live(
        &mut self,
        game: &football::Live<'_>,
        logos: Logos,
        now_ms: Millis,
    ) {
        self.begin_game(
            Mode::FootballLive,
            game.game_id,
            game.away.abbreviation,
            game.home.abbreviation,
            logos,
            now_ms,
        );
        let away_color = Rgb888::from(game.away.colors);
        let home_color = Rgb888::from(game.home.colors);
        let view = &mut self.snapshot_mut().football_live;
        view.away_score = game.away.score;
        view.home_score = game.home.score;
        view.away_color = away_color;
        view.home_color = home_color;
        set_break_clock(
            &mut view.phase_text,
            &mut view.clock_text,
            &mut view.clock_accent,
            game.phase,
            game.period,
            game.clock,
        );
        // Crunch time only where the clock can end a half: Q2, Q4, overtime.
        // The colon-less form is NBA's sub-minute shape, kept as a belt in
        // case ESPN ever emits it for football.
        let half_end = game.period == 2 || game.period >= 4;
        let sub_minute =
            game.clock.starts_with("0:") || (!game.clock.is_empty() && !game.clock.contains(':'));
        view.clock_low = game.phase == LivePhase::InProgress && half_end && sub_minute;

        view.away_timeouts = game.timeouts.map(|timeouts| timeouts.away);
        view.home_timeouts = game.timeouts.map(|timeouts| timeouts.home);

        view.situation = game.situation.map(|situation| FieldSituation {
            down: situation.down,
            distance: situation.distance,
            yard_line: situation.yard_line,
            possession: situation.possession,
            red_zone: situation.red_zone,
        });
        view.situation_text.clear();
        if let Some(situation) = game.situation {
            set_down_and_distance(
                &mut view.situation_text,
                situation.down,
                situation.distance,
                {
                    // "& GOAL" once the first-down target is the goal line.
                    situation.yard_line as u16 + situation.distance as u16 >= 100
                },
            );
        }
        self.finish_game();
    }

    pub fn commit_football_final(
        &mut self,
        game: &football::Final<'_>,
        logos: Logos,
        now_ms: Millis,
    ) {
        self.commit_linescore_final(
            &LinescoreFinal {
                sport: Sport::Football,
                game_id: game.game_id,
                periods: game.periods_played,
                away: game.away,
                home: game.home,
            },
            logos,
            now_ms,
        );
    }

    // -- Soccer ------------------------------------------------------------

    pub fn commit_soccer_live(&mut self, game: &soccer::Live<'_>, logos: Logos, now_ms: Millis) {
        self.begin_game(
            Mode::SoccerLive,
            game.game_id,
            game.away.abbreviation,
            game.home.abbreviation,
            logos,
            now_ms,
        );
        // Stale-feed guard: if the upstream clock has not moved since the last
        // poll of this same game, stop ticking locally and hold the value.
        let stale = self
            .take_prev_soccer_clock(game.game_id, game.clock_seconds)
            .is_some_and(|previous| previous == game.clock_seconds);
        let away_color = Rgb888::from(game.away.colors);
        let home_color = Rgb888::from(game.home.colors);
        let view = &mut self.snapshot_mut().soccer_live;
        view.away_score = game.away.score;
        view.home_score = game.home.score;
        view.clock_anchor_s = game.clock_seconds;
        view.clock_anchor_ms = now_ms;
        // The clock freezes during breaks, when upstream stalls, and once a
        // shootout starts — the match clock is over and PENS carries the state.
        view.clock_running = !game.on_break && !stale && game.half != HALF_SHOOTOUT;
        view.base_min = base_minutes(game.half);
        view.on_break = game.on_break;

        view.phase_text.clear();
        view.phase_long.clear();
        if !game.on_break {
            // The break label lives in the clock region; announcing the state
            // twice is worse than leaving these empty.
            let (short, long) = soccer_phase(game.half);
            set_plain(&mut view.phase_text, short);
            set_plain(&mut view.phase_long, long);
        }

        view.has_event = game.last_event.is_some();
        match game.last_event {
            Some(event) => {
                let label = match event.kind {
                    soccer::EventKind::Goal => "GOAL",
                    soccer::EventKind::RedCard => "RED CARD",
                };
                view.event_top.clear();
                set_plain(&mut view.event_top, label);
                if !event.clock.is_empty() {
                    let _ = view.event_top.push(' ');
                    push_folded(&mut view.event_top, event.clock);
                }
                set_folded(&mut view.event_name, event.athlete);
                view.event_color = match event.side {
                    Some(Side::Home) => home_color,
                    Some(Side::Away) => away_color,
                    None => Rgb888::WHITE,
                };
            }
            None => {
                view.event_top.clear();
                view.event_name.clear();
                view.event_color = Rgb888::WHITE;
            }
        }
        self.finish_game();
    }

    pub fn commit_soccer_final(&mut self, game: &soccer::Final<'_>, logos: Logos, now_ms: Millis) {
        self.begin_game(
            Mode::SoccerFinal,
            game.game_id,
            game.away.abbreviation,
            game.home.abbreviation,
            logos,
            now_ms,
        );
        let away_color = Rgb888::from(game.away.colors);
        let home_color = Rgb888::from(game.home.colors);
        let view = &mut self.snapshot_mut().soccer_final;
        view.away_score = game.away.score;
        view.home_score = game.home.score;
        view.home_won = game.home.score > game.away.score;
        view.draw = game.home.score == game.away.score;
        view.away_color = away_color;
        view.home_color = home_color;
        set_plain(
            &mut view.ft_text,
            match game.flavor {
                soccer::FinalFlavor::AfterExtraTime => "AET",
                soccer::FinalFlavor::AfterPenalties => "PENALTIES",
                soccer::FinalFlavor::FullTime => "FULL TIME",
            },
        );
        set_folded(&mut view.scorers_away, game.away.scorers);
        set_folded(&mut view.scorers_home, game.home.scorers);
        self.finish_game();
    }

    // -- Shared screens ----------------------------------------------------

    /// The line-score final: MLB innings, NBA and football quarters.
    pub fn commit_linescore_final(
        &mut self,
        game: &LinescoreFinal<'_>,
        logos: Logos,
        now_ms: Millis,
    ) {
        let (away, home) = (&game.away, &game.home);
        self.begin_game(
            Mode::Final,
            game.game_id,
            away.abbreviation,
            home.abbreviation,
            logos,
            now_ms,
        );
        let away_color = Rgb888::from(away.colors);
        let home_color = Rgb888::from(home.colors);
        let view = &mut self.snapshot_mut().linescore_final;
        view.sport = game.sport;
        view.away_score = away.score;
        view.home_score = home.score;
        view.home_won = home.score > away.score;
        view.away_color = away_color;
        view.home_color = home_color;
        set_final_text(&mut view.final_text, game.sport, game.periods);
        build_linescore(
            &mut view.header_row,
            &mut view.away_row,
            &mut view.home_row,
            game.periods,
            away.line_score,
            home.line_score,
        );
        self.finish_game();
    }

    /// The upcoming-game screen.
    ///
    /// The date phase self-heals across midnight: this runs on every poll of
    /// the game, so a tomorrow-game becomes a today-game — and loses the date
    /// — within one poll interval of the local day rolling over.
    pub fn commit_pregame(
        &mut self,
        game: &PregameInput<'_>,
        logos: Logos,
        now_ms: Millis,
        clock: LocalClock,
    ) {
        self.begin_game(
            Mode::Pregame,
            game.game_id,
            game.away.abbreviation,
            game.home.abbreviation,
            logos,
            now_ms,
        );
        let view = &mut self.snapshot_mut().pregame;
        view.sport = game.sport;
        for (side, input) in [(&mut view.away, game.away), (&mut view.home, game.home)] {
            side.record = input.record;
            side.color = input.color;
            set_folded(&mut side.line, input.line);
        }
        set_folded(&mut view.info_primary, game.info_primary);

        view.info_secondary.clear();
        if !game.info_secondary.is_empty() {
            if let Some(temperature) = game.temperature {
                crate::text::write_args(&mut view.info_secondary, format_args!("{temperature}F "));
            }
            push_folded_upper(&mut view.info_secondary, game.info_secondary);
        }

        view.time_text.clear();
        view.date_text.clear();
        if let Some(offset) = clock.utc_offset_s {
            let local = civil_from_unix(game.start_time as i64 + offset as i64);
            let hour12 = match local.hour % 12 {
                0 => 12,
                hour => hour,
            };
            let meridiem = if local.hour < 12 { "AM" } else { "PM" };
            let minute = local.minute;
            write_text!(&mut view.time_text, "{hour12}:{minute:02} {meridiem}");

            let today = civil_from_unix(clock.now_epoch_s as i64 + offset as i64);
            if (local.year, local.month, local.day) != (today.year, today.month, today.day) {
                let weekday = WEEKDAYS[local.weekday as usize];
                let month = MONTHS[local.month as usize];
                let day = local.day;
                write_text!(&mut view.date_text, "{weekday} {month} {day}");
            }
        }
        self.finish_game();
    }
}

// -- Formatting helpers ----------------------------------------------------

/// ESPN's shootout period; the match clock is over by then.
const HALF_SHOOTOUT: u8 = 5;

/// Stoppage threshold in minutes per period: regulation halves end at 45 and
/// 90, the extra-time periods at 105 and 120.
fn base_minutes(half: u8) -> u8 {
    const BASE: [u8; 5] = [45, 45, 90, 105, 120];
    BASE[half.min(4) as usize]
}

/// Short and spelled-out period labels. Both extra-time halves read "ET".
fn soccer_phase(half: u8) -> (&'static str, &'static str) {
    const PHASES: [(&str, &str); 5] = [
        ("", ""),
        ("1ST", "1ST HALF"),
        ("2ND", "2ND HALF"),
        ("ET", "EXTRA TIME"),
        ("PENS", "SHOOTOUT"),
    ];
    let index = match half {
        0..=2 => half,
        3..=4 => 3,
        _ => 4,
    };
    PHASES[index as usize]
}

/// Period name for the quarter sports: Q1-Q4, then OT, 2OT, ...
fn set_period_name(dst: &mut Text<SHORT>, period: u8) {
    match period {
        0..=4 => write_text!(dst, "Q{period}"),
        5 => set_plain(dst, "OT"),
        _ => {
            let overtimes = period - 4;
            write_text!(dst, "{overtimes}OT");
        }
    }
}

/// The clock slot shared by NBA and football: during a break the clock string
/// is meaningless (it reads "0:00" or a reset "12:00"), so the phase is the
/// only render signal and the slot shows the break instead.
fn set_break_clock(
    phase_text: &mut Text<SHORT>,
    clock_text: &mut Text<CLOCK>,
    clock_accent: &mut bool,
    phase: LivePhase,
    period: u8,
    clock: &str,
) {
    match phase {
        LivePhase::Halftime => {
            // The clock slot renders "HT"; the period chip stays empty so the
            // state is not announced twice.
            phase_text.clear();
            set_plain(clock_text, "HT");
            *clock_accent = true;
        }
        LivePhase::EndOfPeriod => {
            set_period_name(phase_text, period);
            set_plain(clock_text, "END");
            *clock_accent = true;
        }
        LivePhase::InProgress => {
            set_period_name(phase_text, period);
            set_folded(clock_text, clock);
            *clock_accent = false;
        }
    }
}

/// The banner over a line-score final. Baseball counts innings past nine;
/// the four-period sports count overtimes. (Soccer never reaches here — its
/// full-time screen is its own shape.)
fn set_final_text(dst: &mut Text<SHORT>, sport: Sport, periods: u8) {
    match (sport, periods) {
        (Sport::Mlb, 10..) => write_text!(dst, "F/{periods}"),
        (Sport::Mlb, _) | (_, 0..=4) => set_plain(dst, "FINAL"),
        (_, 5) => set_plain(dst, "F/OT"),
        (_, _) => {
            let overtimes = periods - 4;
            write_text!(dst, "F/{overtimes}OT");
        }
    }
}

const DOWNS: [&str; 5] = ["", "1ST", "2ND", "3RD", "4TH"];

/// "3RD & 7" / "1ST & GOAL". A down outside 1..=4 is not a down anything can
/// be written about, so the line stays empty rather than reading " & 7"; the
/// field markers still draw from the yard line.
fn set_down_and_distance(dst: &mut Text<SHORT>, down: u8, distance: u8, goal_to_go: bool) {
    dst.clear();
    let Some(ordinal) = DOWNS.get(down as usize).filter(|text| !text.is_empty()) else {
        return;
    };
    if goal_to_go {
        write_text!(dst, "{ordinal} & GOAL");
    } else {
        write_text!(dst, "{ordinal} & {distance}");
    }
}

/// "1st", "2nd", "7th", "21st". `state.py` kept a 30-entry table with a bare
/// `str(n)` past the end; the rule reproduces the table exactly and keeps its
/// shape beyond it.
fn set_ordinal(dst: &mut Text<SHORT>, number: u8) {
    let suffix = match (number % 100, number % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    write_text!(dst, "{number}{suffix}");
}

/// Widest line-score the row bound holds, at three chars per column.
const MAX_PERIODS: u8 = (LINESCORE / 3) as u8;

/// Per-period runs are two digits by construction; a third would desynchronise
/// the three rows' widths, which is the whole mechanism.
const MAX_PERIOD_SCORE: u8 = 99;

/// The three line-score rows: period numbers, away, home.
///
/// Every row is the same length — three chars per column — so all three
/// measure identically in the fixed-width font and scroll in lockstep with no
/// extra mechanism. A team with fewer entries than `periods` gets `" X "` for
/// the missing trailing columns: the walk-off convention.
fn build_linescore(
    header: &mut Text<LINESCORE>,
    away_row: &mut Text<LINESCORE>,
    home_row: &mut Text<LINESCORE>,
    periods: u8,
    away_line: &[u8],
    home_line: &[u8],
) {
    header.clear();
    away_row.clear();
    home_row.clear();
    for period in 0..periods.min(MAX_PERIODS) {
        let number = period as u16 + 1;
        crate::text::write_args(header, format_args!("{number:>2} "));
        for (row, line) in [(&mut *away_row, away_line), (&mut *home_row, home_line)] {
            match line.get(period as usize) {
                Some(&score) => {
                    let score = score.min(MAX_PERIOD_SCORE);
                    crate::text::write_args(row, format_args!("{score:>2} "));
                }
                None => crate::text::write_args(row, format_args!(" X ")),
            }
        }
    }
    debug_assert_eq!(header.len(), away_row.len());
    debug_assert_eq!(header.len(), home_row.len());
}

const WEEKDAYS: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];
const MONTHS: [&str; 13] = [
    "", "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// A civil date and time-of-day, the fields `time.gmtime()` supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Civil {
    pub(crate) year: i32,
    pub(crate) month: u8,
    pub(crate) day: u8,
    /// 0 = Monday, matching `time.gmtime()`'s `tm_wday`.
    pub(crate) weekday: u8,
    pub(crate) hour: u8,
    pub(crate) minute: u8,
}

/// Unix seconds to a civil date (Howard Hinnant's `civil_from_days`, whose
/// era arithmetic is exact for the whole range and needs no leap table).
pub(crate) fn civil_from_unix(seconds: i64) -> Civil {
    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);

    // Shift the epoch to 0000-03-01 so leap day lands at the end of a year.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * march_month + 2) / 5 + 1) as u8;
    let month = (march_month + if march_month < 10 { 3 } else { -9 }) as u8;
    let year = (year_of_era + era * 400 + i64::from(month <= 2)) as i32;

    Civil {
        year,
        month,
        day,
        // 1970-01-01 was a Thursday, index 3 in a Monday-first week.
        weekday: (days + 3).rem_euclid(7) as u8,
        hour: (time_of_day / 3_600) as u8,
        minute: (time_of_day % 3_600 / 60) as u8,
    }
}

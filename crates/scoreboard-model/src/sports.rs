//! The per-sport commits: wire game in, snapshot view out.
//!
//! Each one is the Rust body of a `state.py` setter, minus the parts that
//! turned out to be pixels (see [`crate::snapshot`] for where the line is).

use scoreboard_wire::{FinalTeam, LivePhase, Side, basketball, football, hockey, mlb, soccer};

use crate::basketball::BasketballRules;
use crate::color::Rgb888;
use crate::feed::{GameDetail, LeagueId};
use crate::periods::Periods;
use crate::snapshot::{
    Banner, BannerKind, Bases, CLOCK, CourtMark, FieldSituation, GoalMark, LINESCORE, LeaderLine,
    Millis, Mode, Record, SHOOTOUT_SLOTS, SHORT, ShootoutRow, ShootoutView, ShotMark, Sport,
    StarLine,
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
    /// Innings or periods played — the column count, and what decides whether
    /// the banner reads "FINAL", "F/10" or "F/OT".
    pub periods: u8,
    /// Regulation's length in the same units: the periods past it are
    /// overtime (baseball's extra innings count their own way).
    pub regulation: u8,
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
        ot_losses: None,
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

    /// NBA: real stadium and records; no weather. The second info phase is
    /// the scoreboard's note, plus the series in the playoffs, as the wire's
    /// `ext=2` joined it ("NBA CUP - GROUP PLAY", "NBA FINALS - GAME 4 - NY
    /// LEADS SERIES 2-1"; NBA.md §3.8), empty on most nights and below
    /// `ext=2`. The per-team line is the betting line ("+3.5  +140"), which
    /// only `ext=1` carries; an absent line is an empty slot, which the screen
    /// already handles.
    pub fn basketball(game: &basketball::Pregame<'a>) -> Self {
        Self {
            sport: Sport::Basketball,
            game_id: game.game_id,
            start_time: game.start_time,
            info_primary: game.venue,
            info_secondary: game.info_secondary,
            temperature: None,
            away: PregameSideInput {
                abbreviation: game.away.abbreviation,
                record: record(game.away.record),
                line: game.away.line.unwrap_or(""),
                color: game.away.colors.into(),
            },
            home: PregameSideInput {
                abbreviation: game.home.abbreviation,
                record: record(game.home.record),
                line: game.home.line.unwrap_or(""),
                color: game.home.colors.into(),
            },
        }
    }

    /// Hockey: NBA's shape with two deltas (NHL.md §3.9) — records are W-L-OTL,
    /// and the per-team line is the probable starting goalie. The second info
    /// phase is the playoff round and series the wire already joined ("Stanley
    /// Cup Final - Game 6 - CAR leads series 3-2"), empty in the regular season.
    pub fn hockey(game: &hockey::Pregame<'a>) -> Self {
        let record = |record: Option<hockey::Record>| {
            record.map(|r| Record {
                wins: r.wins,
                losses: r.losses,
                ot_losses: Some(r.ot_losses),
            })
        };
        Self {
            sport: Sport::Hockey,
            game_id: game.game_id,
            start_time: game.start_time,
            info_primary: game.venue,
            info_secondary: game.info_secondary,
            temperature: None,
            away: PregameSideInput {
                abbreviation: game.away.abbreviation,
                record: record(game.away.record),
                line: game.away.goalie.unwrap_or(""),
                color: game.away.colors.into(),
            },
            home: PregameSideInput {
                abbreviation: game.home.abbreviation,
                record: record(game.home.record),
                line: game.home.goalie.unwrap_or(""),
                color: game.home.colors.into(),
            },
        }
    }

    /// Football: the stadium, and no second phase — no screen names the league
    /// (NCAAB.md D9: the crests already say it). The per-team line is a ranked
    /// college team's "#3 OHIO STATE", else the betting line ("+7  -305").
    pub fn football(game: &football::Pregame<'a>) -> Self {
        Self {
            sport: Sport::Football,
            game_id: game.game_id,
            start_time: game.start_time,
            info_primary: game.venue,
            info_secondary: "",
            temperature: None,
            away: PregameSideInput {
                abbreviation: game.away.abbreviation,
                record: record(game.away.record),
                line: game.away.line.unwrap_or(""),
                color: game.away.colors.into(),
            },
            home: PregameSideInput {
                abbreviation: game.home.abbreviation,
                record: record(game.home.record),
                line: game.home.line.unwrap_or(""),
                color: game.home.colors.into(),
            },
        }
    }

    /// Soccer: the stadium alone (no league name, NCAAB.md D9), no records,
    /// and the per-team line is the club's abbreviation — the lower half of the
    /// screen would otherwise be empty.
    pub fn soccer(game: &soccer::Pregame<'a>) -> Self {
        Self {
            sport: Sport::Soccer,
            game_id: game.game_id,
            start_time: game.start_time,
            info_primary: game.venue,
            info_secondary: "",
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
        match detail {
            GameDetail::Mlb(mlb::Game::Live(game)) => {
                self.commit_mlb_live(game, logos, now_ms);
                self.flash_play(game.last_play.id, game.last_play.text, now_ms);
            }
            GameDetail::Mlb(mlb::Game::Pregame(game)) => {
                self.commit_pregame(&PregameInput::mlb(game), logos, now_ms, clock);
            }
            GameDetail::Mlb(mlb::Game::Final(game)) => self.commit_mlb_final(game, logos, now_ms),
            GameDetail::Basketball(basketball::Game::Live(game)) => {
                self.commit_basketball_live(game, BasketballRules::of(league), logos, now_ms);
                if let Some(play) = game.last_play {
                    self.flash_play(play.id, play.text, now_ms);
                }
            }
            GameDetail::Basketball(basketball::Game::Pregame(game)) => {
                self.commit_pregame(&PregameInput::basketball(game), logos, now_ms, clock);
            }
            GameDetail::Basketball(basketball::Game::Final(game)) => {
                self.commit_basketball_final(game, BasketballRules::of(league), logos, now_ms)
            }
            GameDetail::Hockey(hockey::Game::Live(game)) => {
                self.commit_hockey_live(game, logos, now_ms);
                if let Some(play) = game.last_play {
                    self.flash_play(play.id, play.text, now_ms);
                }
            }
            GameDetail::Hockey(hockey::Game::Pregame(game)) => {
                self.commit_pregame(&PregameInput::hockey(game), logos, now_ms, clock);
            }
            GameDetail::Hockey(hockey::Game::Final(game)) => {
                self.commit_hockey_final(game, logos, now_ms);
            }
            GameDetail::Football(football::Game::Live(game)) => {
                self.commit_football_live(game, logos, now_ms);
                if let Some(play) = game.last_play {
                    self.flash_play(play.id, play.text, now_ms);
                }
            }
            GameDetail::Football(football::Game::Pregame(game)) => {
                self.commit_pregame(
                    &PregameInput::football(game),
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
                    &PregameInput::soccer(game),
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
        self.begin_game(Mode::MlbLive, game.game_id, logos, now_ms);
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
                regulation: 9,
                away: game.away,
                home: game.home,
            },
            logos,
            now_ms,
        );
    }

    // -- Basketball --------------------------------------------------------

    /// The court screen (NBA.md §3). The first-look rule is hockey's: the
    /// ledger hold and the news stamp compare against this device's previous
    /// commit only when that commit was this game, live. Anything else —
    /// another game, this game's pregame, an error screen — makes this a
    /// first look, which shows whatever ledger the payload carries and stamps
    /// no news: a basket already on the board when the game rotates in is old
    /// news (§3.6).
    pub fn commit_basketball_live(
        &mut self,
        game: &basketball::Live<'_>,
        rules: BasketballRules,
        logos: Logos,
        now_ms: Millis,
    ) {
        let first_look =
            self.mode() != Mode::BasketballLive || self.snapshot().game_id != game.game_id;
        self.begin_game(Mode::BasketballLive, game.game_id, logos, now_ms);
        let view = &mut self.snapshot_mut().basketball_live;
        view.away_score = game.away.score;
        view.home_score = game.home.score;
        view.away_color = Rgb888::from(game.away.colors);
        view.home_color = Rgb888::from(game.home.colors);
        view.court = rules.court;
        set_break_clock(
            &mut view.phase_text,
            &mut view.clock_text,
            &mut view.clock_accent,
            game.phase,
            game.period,
            game.clock,
            Naming {
                periods: rules.periods,
                halftime: BASKETBALL_HALFTIME,
            },
        );
        // Crunch time where the clock can end the game — the last period of
        // regulation and overtime — under a minute (NCAAB.md D7).
        view.clock_low = game.phase == LivePhase::InProgress
            && game.period >= rules.periods.regulation()
            && crunch_clock(game.clock);

        // The ledger hold (§3.3): a payload without a ledger — the summary not
        // fetched, or the one-poll glitch body the backend drops — keeps the
        // previous poll's, which outlives the glitch. A first look has no
        // previous poll to keep.
        view.ledger_tracks = rules.ledger;
        match game.ledger {
            _ if !rules.ledger => view.ledger = None,
            Some(ledger) => view.ledger = Some(ledger),
            None if first_look => view.ledger = None,
            None => {}
        }
        view.run = game.run;

        // The news stamp (§3.6). The key is the newest scoring play's sequence
        // number, so a corrected score is not news and a repeated poll is not
        // news twice. The key survives a poll whose summary was missing, so
        // the group coming back unchanged is not news either; and the first
        // key this look learns is adopted, not stamped — whether it arrives
        // with the first look or with the first summary after it, the basket
        // was on the board already.
        if first_look {
            view.score_ms = 0;
            view.score_seq = 0;
        }
        view.caption.clear();
        match game.newest_score {
            Some(score) => {
                if view.score_seq != 0 && score.seq != view.score_seq {
                    view.score_ms = now_ms;
                }
                view.score_seq = score.seq;
                view.last_score = Some(score.side);
                view.newest_marked = score.marked;
                if !score.scorer.is_empty() {
                    set_folded(&mut view.caption, score.scorer);
                    if score.scorer_points > 0 {
                        let points = score.scorer_points;
                        crate::text::write_args(&mut view.caption, format_args!(" {points}"));
                    }
                }
            }
            None => {
                view.last_score = None;
                view.newest_marked = false;
            }
        }

        view.marks.clear();
        for make in game.makes.iter() {
            // The wire carries at most the capacity's 128 makes.
            let _ = view.marks.push(CourtMark {
                side: make.side,
                x: make.x,
                y: make.y,
            });
        }
        self.finish_game();
    }

    /// The game-leaders final (D2). What every final has — scores, colors,
    /// the result label, the quarter rows — goes to the shared final view,
    /// whose rows stand in for the leaders while ESPN has not posted them
    /// (§3.9); the NBA view holds only what the NBA adds.
    pub fn commit_basketball_final(
        &mut self,
        game: &basketball::Final<'_>,
        rules: BasketballRules,
        logos: Logos,
        now_ms: Millis,
    ) {
        self.begin_game(Mode::BasketballFinal, game.game_id, logos, now_ms);
        self.fill_linescore_final(&LinescoreFinal {
            sport: Sport::Basketball,
            game_id: game.game_id,
            periods: game.periods_played,
            regulation: rules.periods.regulation(),
            away: game.away,
            home: game.home,
        });
        let view = &mut self.snapshot_mut().basketball_final;
        view.leaders = game
            .leaders
            .map(|leaders| [leaders.points, leaders.rebounds, leaders.assists].map(leader_line));
        view.series.clear();
        if let Some(series) = game.series {
            push_folded_upper(&mut view.series, series);
        }
        view.records = game.records.map(|records| {
            [records.away, records.home].map(|record| Record {
                wins: record.wins,
                losses: record.losses,
                ot_losses: None,
            })
        });
        self.finish_game();
    }

    // -- Hockey ------------------------------------------------------------

    pub fn commit_hockey_live(&mut self, game: &hockey::Live<'_>, logos: Logos, now_ms: Millis) {
        // The lamp compares against this device's previous commit only when
        // that commit was this game, live. Anything else — another game, this
        // game's pregame, an error screen — makes this the first look, and a
        // goal already on the board at a first look is old news (NHL.md §3.7).
        let first_look = self.mode() != Mode::HockeyLive || self.snapshot().game_id != game.game_id;
        self.begin_game(Mode::HockeyLive, game.game_id, logos, now_ms);
        let newest = game.goals.last();
        // A regular-season period 5 is the shootout, whose scores say nothing
        // new: the extractor holds them at the tie (§3.8).
        let shootout_period = !game.playoffs && game.period >= 5;
        let lit = if shootout_period && !first_look {
            None
        } else {
            self.lamp_watch().observe(
                first_look,
                [game.away.score, game.home.score],
                newest.map(|goal| goal.side),
                now_ms,
            )
        };

        let away_color = Rgb888::from(game.away.colors);
        let home_color = Rgb888::from(game.home.colors);
        let view = &mut self.snapshot_mut().hockey_live;
        if first_look {
            view.lamp_side = None;
            view.lamp_ms = 0;
        }
        if let Some(side) = lit {
            view.lamp_side = Some(side);
            view.lamp_ms = now_ms;
        }
        view.away_score = game.away.score;
        view.home_score = game.home.score;
        view.away_color = away_color;
        view.home_color = home_color;

        set_hockey_period(&mut view.period_text, game.period, game.playoffs);
        match game.phase {
            LivePhase::InProgress => {
                set_folded(&mut view.clock_text, game.clock);
                view.clock_accent = false;
            }
            // ESPN's end-of-period status, with the chip keeping the period
            // that just ended (§3.2). It is the intermission when another
            // period follows, and the end when none can: after the shootout,
            // or a third period or overtime that ended with the scores
            // unequal — the half minute ESPN spends there before FINAL.
            // Halftime is never sent for hockey, but the shared code set
            // decodes it, and it is a break all the same.
            LivePhase::EndOfPeriod | LivePhase::Halftime => {
                let decided = game.period >= 3 && game.away.score != game.home.score;
                let over = shootout_period || decided;
                set_plain(&mut view.clock_text, if over { "END" } else { "INT" });
                view.clock_accent = true;
            }
        }
        // Crunch time from the third period on. The colon-less form is NBA's
        // sub-minute shape, kept as a belt in case ESPN ever emits it here.
        // The shootout has no clock to run out — ESPN leaves it at 0:00.
        let sub_minute =
            game.clock.starts_with("0:") || (!game.clock.is_empty() && !game.clock.contains(':'));
        view.clock_low = game.phase == LivePhase::InProgress
            && game.period >= 3
            && !shootout_period
            && sub_minute;

        view.shots = game.shots;
        (view.away_skaters, view.home_skaters) = game.strength.map_or((0, 0), |strength| {
            (strength.away_skaters, strength.home_skaters)
        });
        view.banner = game
            .strength
            .and_then(|strength| banner(strength, game.period, game.playoffs));

        view.marks.clear();
        for goal in game.goals.iter() {
            if let Some(at) = goal.coordinate {
                // The wire carries at most the capacity's sixteen goals.
                let _ = view.marks.push(GoalMark {
                    side: goal.side,
                    x: at.x,
                    y: at.y,
                });
            }
        }
        view.newest_marked = newest.is_some_and(|goal| goal.coordinate.is_some());
        view.last_goal = newest.map(|goal| goal.side);
        view.goal_line.clear();
        if let Some(goal) = newest {
            let tag = goal_tag(goal.tag);
            let (minutes, seconds) = (goal.elapsed / 60, goal.elapsed % 60);
            write_text!(&mut view.goal_line, "{tag} {minutes}:{seconds:02}");
            if !game.last_scorer.is_empty() {
                let _ = view.goal_line.push(' ');
            }
            push_folded(&mut view.goal_line, game.last_scorer);
        }

        // The shootout layouts take over only in the shootout itself (§3.1):
        // a group sent with any other period would be a backend bug, and the
        // regular layout is the safe thing to draw for it.
        view.shootout = game.shootout.filter(|_| shootout_period).map(shootout_view);
        self.finish_game();
    }

    /// The three-stars final (D2). What every final has — scores, colors, the
    /// result label, and the line score in hockey's own columns — goes to the
    /// shared final view, which this screen draws it from; the hockey view
    /// holds only what hockey adds (NHL.md §3.10).
    pub fn commit_hockey_final(&mut self, game: &hockey::Final<'_>, logos: Logos, now_ms: Millis) {
        let (away, home) = (&game.away, &game.home);
        self.begin_game(Mode::HockeyFinal, game.game_id, logos, now_ms);
        let view = &mut self.snapshot_mut().linescore_final;
        view.sport = Sport::Hockey;
        view.away_score = away.score;
        view.home_score = home.score;
        view.home_won = home.score > away.score;
        view.away_color = Rgb888::from(away.colors);
        view.home_color = Rgb888::from(home.colors);
        set_hockey_final_text(&mut view.final_text, game.periods_played, game.playoffs);
        build_hockey_linescore(
            &mut view.header_row,
            &mut view.away_row,
            &mut view.home_row,
            game.periods_played,
            game.playoffs,
            away.line_score,
            home.line_score,
        );

        let view = &mut self.snapshot_mut().hockey_final;
        view.stars = game.stars.map(|stars| {
            stars.map(|star| {
                let mut line = StarLine {
                    name: Text::new(),
                    side: star.side,
                };
                set_folded(&mut line.name, star.name);
                line
            })
        });
        view.series.clear();
        if let Some(series) = game.series {
            push_folded_upper(&mut view.series, series);
        }
        view.shots = game.shots;
        self.finish_game();
    }

    // -- Football ----------------------------------------------------------

    pub fn commit_football_live(
        &mut self,
        game: &football::Live<'_>,
        logos: Logos,
        now_ms: Millis,
    ) {
        self.begin_game(Mode::FootballLive, game.game_id, logos, now_ms);
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
            Naming {
                periods: Periods::QUARTERS,
                halftime: FOOTBALL_HALFTIME,
            },
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
                regulation: Periods::QUARTERS.regulation(),
                away: game.away,
                home: game.home,
            },
            logos,
            now_ms,
        );
    }

    // -- Soccer ------------------------------------------------------------

    pub fn commit_soccer_live(&mut self, game: &soccer::Live<'_>, logos: Logos, now_ms: Millis) {
        self.begin_game(Mode::SoccerLive, game.game_id, logos, now_ms);
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
        if !game.on_break {
            // The break label lives in the clock region; announcing the state
            // twice is worse than leaving this empty.
            set_plain(&mut view.phase_text, soccer_phase(game.half));
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
        self.begin_game(Mode::SoccerFinal, game.game_id, logos, now_ms);
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

    /// The line-score final: MLB innings and football quarters.
    pub fn commit_linescore_final(
        &mut self,
        game: &LinescoreFinal<'_>,
        logos: Logos,
        now_ms: Millis,
    ) {
        self.begin_game(Mode::Final, game.game_id, logos, now_ms);
        self.fill_linescore_final(game);
        self.finish_game();
    }

    /// The shared final view — scores, colors, the result label and the
    /// equal-column rows — for the line-score screen and for the NBA's
    /// leaders screen, which draws them around its own rows.
    fn fill_linescore_final(&mut self, game: &LinescoreFinal<'_>) {
        let (away, home) = (&game.away, &game.home);
        let away_color = Rgb888::from(away.colors);
        let home_color = Rgb888::from(home.colors);
        let view = &mut self.snapshot_mut().linescore_final;
        view.sport = game.sport;
        view.away_score = away.score;
        view.home_score = home.score;
        view.home_won = home.score > away.score;
        view.away_color = away_color;
        view.home_color = home_color;
        set_final_text(&mut view.final_text, game.sport, game.periods, game.regulation);
        // Baseball numbers every inning it plays; the others number regulation
        // and gather every overtime into one "OT" column, hockey's rule
        // (BACKLOG 104) — a second overtime is the banner's "F/2OT".
        let overtime_from = (game.sport != Sport::Mlb).then_some(game.regulation);
        build_linescore(
            &mut view.header_row,
            &mut view.away_row,
            &mut view.home_row,
            game.periods,
            overtime_from,
            away.line_score,
            home.line_score,
        );
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
        self.begin_game(Mode::Pregame, game.game_id, logos, now_ms);
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
fn soccer_phase(half: u8) -> &'static str {
    const PHASES: [&str; 5] = ["", "1ST", "2ND", "ET", "PENS"];
    let index = match half {
        0..=2 => half,
        3..=4 => 3,
        _ => 4,
    };
    PHASES[index as usize]
}

/// How a sport's period chip and clock slot name its periods and breaks.
#[derive(Debug, Clone, Copy)]
struct Naming {
    periods: Periods,
    halftime: Halftime,
}

/// What a sport's clock slot says at halftime, and whether its period chip
/// stays up through it.
#[derive(Debug, Clone, Copy)]
struct Halftime {
    word: &'static str,
    keeps_period: bool,
}

/// Football blanks the chip and reads "HT", so the state is not announced
/// twice.
const FOOTBALL_HALFTIME: Halftime = Halftime {
    word: "HT",
    keeps_period: false,
};

/// The NBA keeps the period that ended and reads "HALF" — hockey's rule, and
/// what every mockup the owner saw read; the slot has the room (NBA.md §3.2).
const BASKETBALL_HALFTIME: Halftime = Halftime {
    word: "HALF",
    keeps_period: true,
};

/// Under a minute in either of ESPN's clock shapes — college's "0:39" or the
/// NBA's colonless "39.0" — and not all zeros: a real zero lasts a poll before
/// the period ends, while ESPN's frozen clocks ("0.0", "0:00") can sit for
/// minutes and must not wear the warning colour (NCAAB.md D7).
pub(crate) fn crunch_clock(clock: &str) -> bool {
    let under_a_minute = clock.starts_with("0:") || (!clock.is_empty() && !clock.contains(':'));
    under_a_minute && clock.bytes().any(|b| b.is_ascii_digit() && b != b'0')
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
    naming: Naming,
) {
    let Naming { periods, halftime } = naming;
    match phase {
        LivePhase::Halftime => {
            if halftime.keeps_period {
                periods.write_name(period, phase_text);
            } else {
                phase_text.clear();
            }
            set_plain(clock_text, halftime.word);
            *clock_accent = true;
        }
        LivePhase::EndOfPeriod => {
            periods.write_name(period, phase_text);
            set_plain(clock_text, "END");
            *clock_accent = true;
        }
        LivePhase::InProgress => {
            periods.write_name(period, phase_text);
            set_folded(clock_text, clock);
            *clock_accent = false;
        }
    }
}

/// The banner over a line-score final. Baseball counts innings past nine;
/// the other sports count overtimes past their regulation, and basketball
/// spells its single overtime out (NBA.md §3.1: `FINAL`, `FINAL/OT`,
/// `F/{n}OT`). (Soccer never reaches here — its full-time screen is its own
/// shape.)
fn set_final_text(dst: &mut Text<SHORT>, sport: Sport, periods: u8, regulation: u8) {
    match sport {
        Sport::Mlb if periods >= 10 => write_text!(dst, "F/{periods}"),
        Sport::Mlb => set_plain(dst, "FINAL"),
        _ if periods <= regulation => set_plain(dst, "FINAL"),
        Sport::Basketball if periods == regulation + 1 => set_plain(dst, "FINAL/OT"),
        _ if periods == regulation + 1 => set_plain(dst, "F/OT"),
        _ => {
            let overtimes = periods - regulation;
            write_text!(dst, "F/{overtimes}OT");
        }
    }
}

/// One leaders row (NBA.md §3.9): the short name and the stat as one string
/// in the player's team color — "J. Brunson 36".
fn leader_line(leader: basketball::Leader<'_>) -> LeaderLine {
    let mut line = LeaderLine {
        text: Text::new(),
        side: leader.side,
    };
    set_folded(&mut line.text, leader.name);
    let value = leader.value;
    crate::text::write_args(&mut line.text, format_args!(" {value}"));
    line
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
    overtime_from: Option<u8>,
    away_line: &[u8],
    home_line: &[u8],
) {
    header.clear();
    away_row.clear();
    home_row.clear();
    let periods = periods.min(MAX_PERIODS) as usize;
    let numbered = overtime_from.map_or(periods, |regulation| periods.min(regulation as usize));
    let regulation = (0..numbered).map(|period| (period as u16 + 1, period..period + 1));
    let overtime = (periods > numbered).then_some((0, numbered..periods));
    for (number, range) in regulation.chain(overtime) {
        if number == 0 {
            crate::text::write_args(header, format_args!("OT "));
        } else {
            crate::text::write_args(header, format_args!("{number:>2} "));
        }
        for (row, line) in [(&mut *away_row, away_line), (&mut *home_row, home_line)] {
            match line.get(range.start..range.end.min(line.len())) {
                Some(points) if !points.is_empty() => {
                    let total = points.iter().map(|&p| p as u16).sum::<u16>();
                    let total = total.min(MAX_PERIOD_SCORE as u16);
                    crate::text::write_args(row, format_args!("{total:>2} "));
                }
                _ => crate::text::write_args(row, format_args!(" X ")),
            }
        }
    }
    debug_assert_eq!(header.len(), away_row.len());
    debug_assert_eq!(header.len(), home_row.len());
}

// -- Hockey helpers ----------------------------------------------------------

/// How long a side's score must stay below its high-water mark before the lamp
/// believes the drop (NHL.md §3.7).
///
/// Measured over two weeks of real NHL polls: a score went down 33 times in 26
/// of 99 games. Sixteen were CDN flickers, every one restored within 90 s;
/// seventeen were disallowed goals that stayed down. Until a drop has stood
/// this long the side's old high stands, so a flicker's restore is not a goal
/// — DET at MTL went 0-0, 0-1, 0-0, 0-1, 0-0 in overtime, and lights once.
///
/// The cost: a real goal within 120 s of a disallowed one does not light.
/// Accepted, because a missed lamp is benign and a false one is not.
pub const LAMP_SETTLE_MS: Millis = 120_000;

/// The goal lamp's memory of the hockey game on screen: per side, the highest
/// score this look has accepted, and since when the score has sat below it.
/// Core-0 model state, kept in the [`Store`] beside the snapshot it decides,
/// as the soccer stale-clock guard is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LampWatch {
    /// Away, then home.
    high: [u16; 2],
    below_since: [Option<Millis>; 2],
}

impl LampWatch {
    pub(crate) const fn new() -> Self {
        Self {
            high: [0; 2],
            below_since: [None; 2],
        }
    }

    /// Fold in one commit's scores, away then home, and say which side's
    /// score to light, if any.
    ///
    /// A first look adopts the scores as they stand and lights nothing. After
    /// that a score above its high lights and becomes the high; one below it
    /// is believed only once it has stayed below for [`LAMP_SETTLE_MS`], when
    /// the high comes down to meet it; one back at its high was a flicker.
    /// Both sides rising in one poll lights the newest goal's side — home when
    /// the goals are missing too.
    pub(crate) fn observe(
        &mut self,
        first_look: bool,
        scores: [u16; 2],
        newest_goal: Option<Side>,
        now_ms: Millis,
    ) -> Option<Side> {
        if first_look {
            *self = Self {
                high: scores,
                below_since: [None; 2],
            };
            return None;
        }
        let mut rose = [false; 2];
        for (index, score) in scores.into_iter().enumerate() {
            let high = &mut self.high[index];
            let below_since = &mut self.below_since[index];
            if score > *high {
                *high = score;
                *below_since = None;
                rose[index] = true;
            } else if score < *high {
                let since = *below_since.get_or_insert(now_ms);
                if now_ms.saturating_sub(since) >= LAMP_SETTLE_MS {
                    *high = score;
                    *below_since = None;
                }
            } else {
                *below_since = None;
            }
        }
        match rose {
            [true, true] => Some(newest_goal.unwrap_or(Side::Home)),
            [true, false] => Some(Side::Away),
            [false, true] => Some(Side::Home),
            [false, false] => None,
        }
    }
}

/// Hockey's period ordinals; past the third, the chip reads overtime.
const HOCKEY_PERIODS: [&str; 4] = ["", "1ST", "2ND", "3RD"];

/// The period chip (NHL.md §3.1): 1ST to 3RD, then OT. ESPN's period 5 is the
/// shootout in the regular season and the second overtime in the playoffs,
/// which count on — 2OT, 3OT — and only the season type tells them apart.
fn set_hockey_period(dst: &mut Text<SHORT>, period: u8, playoffs: bool) {
    match period {
        0..=3 => set_plain(dst, HOCKEY_PERIODS[period as usize]),
        4 => set_plain(dst, "OT"),
        _ if !playoffs => set_plain(dst, "SO"),
        _ => {
            let overtimes = period - 3;
            write_text!(dst, "{overtimes}OT");
        }
    }
}

/// The result label (NHL.md §3.1): FINAL, FINAL/OT, FINAL/SO, and from the
/// playoffs' second overtime the short form, F/2OT.
fn set_hockey_final_text(dst: &mut Text<SHORT>, periods: u8, playoffs: bool) {
    match periods {
        0..=3 => set_plain(dst, "FINAL"),
        4 => set_plain(dst, "FINAL/OT"),
        _ if !playoffs => set_plain(dst, "FINAL/SO"),
        _ => {
            let overtimes = periods - 3;
            write_text!(dst, "F/{overtimes}OT");
        }
    }
}

const fn goal_tag(tag: hockey::GoalTag) -> &'static str {
    match tag {
        hockey::GoalTag::Goal => "GOAL",
        hockey::GoalTag::PowerPlay => "PPG",
        hockey::GoalTag::ShortHanded => "SHG",
        hockey::GoalTag::EmptyNet => "ENG",
    }
}

/// The banner a strength earns (NHL.md §3.4, step 5), the advantaged side's
/// skaters counted first.
fn banner(strength: hockey::Strength, period: u8, playoffs: bool) -> Option<Banner> {
    let (away, home) = (strength.away_skaters, strength.home_skaters);
    let counts = |side| match side {
        Side::Away => (away, home),
        Side::Home => (home, away),
    };
    let (kind, (attackers, defenders)) = match strength.kind {
        hockey::StrengthKind::Even => return None,
        // 3-on-3 is how regular-season overtime is played, not news.
        hockey::StrengthKind::Matched if !playoffs && period == 4 && away == 3 && home == 3 => {
            return None;
        }
        hockey::StrengthKind::Matched => (BannerKind::Matched, (away, home)),
        hockey::StrengthKind::PowerPlay(side) => (BannerKind::PowerPlay(side), counts(side)),
        hockey::StrengthKind::EmptyNet(side) => (BannerKind::EmptyNet(side), counts(side)),
    };
    Some(Banner {
        kind,
        attackers,
        defenders,
    })
}

/// A shootout row shows at least the best-of-three's three rounds.
const SHOOTOUT_ROUNDS: usize = 3;

/// The shootout as the screens draw it (NHL.md §3.8): each side's attempts in
/// order, then a marker on whoever shoots next, padded to three rounds and
/// windowed to the newest [`SHOOTOUT_SLOTS`]. One window serves both rows, so
/// a column is the same round on both sides however long sudden death runs.
///
/// The wire keeps a side's first sixteen attempts, so a shootout longer than
/// that — the record is twenty rounds — freezes its markers at round sixteen.
fn shootout_view(shootout: hockey::Shootout) -> ShootoutView {
    let away_next = shootout.next_shooter == Some(Side::Away);
    let home_next = shootout.next_shooter == Some(Side::Home);
    let rounds =
        shootout_columns(shootout.away, away_next).max(shootout_columns(shootout.home, home_next));
    let skip = rounds.saturating_sub(SHOOTOUT_SLOTS);
    ShootoutView {
        away: shootout_row(shootout.away, away_next, skip),
        home: shootout_row(shootout.home, home_next, skip),
    }
}

fn shootout_columns(attempts: hockey::ShootoutAttempts, next: bool) -> usize {
    (attempts.taken as usize + usize::from(next)).max(SHOOTOUT_ROUNDS)
}

fn shootout_row(attempts: hockey::ShootoutAttempts, next: bool, skip: usize) -> ShootoutRow {
    let mut row = ShootoutRow::new();
    // The tally is the attempts', never ESPN's running score, which flips to
    // the tally mid-shootout (§3.8).
    row.tally = attempts.scored.count_ones() as u8;
    let marks = (0..attempts.taken)
        .map(|attempt| {
            if attempts.scored >> attempt & 1 != 0 {
                ShotMark::Scored
            } else {
                ShotMark::Missed
            }
        })
        .chain(next.then_some(ShotMark::Next))
        .chain(core::iter::repeat(ShotMark::Pending))
        .take(shootout_columns(attempts, next))
        .skip(skip);
    for mark in marks {
        // `skip` leaves at most SHOOTOUT_SLOTS of the widest row.
        let _ = row.marks.push(mark);
    }
    row
}

/// The regulation columns' headers.
const HOCKEY_COLUMNS: [&str; 3] = ["1", "2", "3"];

/// The hockey final's line score (NHL.md §3.10): the three periods, one "OT"
/// column however many overtimes were played — sudden death means only the
/// last of them can hold a goal, so the column is that goal — and an "SO"
/// column for a regular-season shootout, ESPN's fifth period of 1/0. The
/// columns are [`build_linescore`]'s three chars, so the rows still measure
/// alike and scroll in lockstep.
fn build_hockey_linescore(
    header: &mut Text<LINESCORE>,
    away_row: &mut Text<LINESCORE>,
    home_row: &mut Text<LINESCORE>,
    periods: u8,
    playoffs: bool,
    away_line: &[u8],
    home_line: &[u8],
) {
    header.clear();
    away_row.clear();
    home_row.clear();
    let periods = periods as usize;
    let shootout = !playoffs && periods >= 5;
    let regulation = (0..periods.min(3)).map(|period| (HOCKEY_COLUMNS[period], period..period + 1));
    let overtime = (periods > 3).then_some(("OT", 3..if shootout { 4 } else { periods }));
    let shootout = shootout.then_some(("SO", 4..periods));
    for (label, range) in regulation.chain(overtime).chain(shootout) {
        crate::text::write_args(header, format_args!("{label:>2} "));
        for (row, line) in [(&mut *away_row, away_line), (&mut *home_row, home_line)] {
            match line.get(range.start..range.end.min(line.len())) {
                Some(goals) if !goals.is_empty() => {
                    let total = goals
                        .iter()
                        .map(|&goals| goals as u16)
                        .sum::<u16>()
                        .min(MAX_PERIOD_SCORE as u16);
                    crate::text::write_args(row, format_args!("{total:>2} "));
                }
                _ => crate::text::write_args(row, format_args!(" X ")),
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

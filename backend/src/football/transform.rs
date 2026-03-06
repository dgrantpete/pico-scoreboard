use crate::espn::types::{EspnCompetition, EspnCompetitor, EspnEvent, EspnLastPlay, EspnSituation};
use crate::shared::transform::{get_broadcast, get_competitors, parse_espn_date, parse_hex_color, parse_rank};
use crate::shared::types::Weather;
use crate::sport::{EspnLeague, FootballLeague};

use super::types::{
    Down, FootballFinal, FootballGameResponse, FootballLive, FootballPeriod, FootballPregame,
    FootballTeamScore, LastPlay, PlayType, Possession, Situation,
};

use crate::shared::types::{FinalStatus, Winner};

/// Transform an ESPN event into our football API response format
pub fn transform(event: &EspnEvent, league: FootballLeague) -> FootballGameResponse {
    let competition = &event.competitions[0];
    let state = event.status.status_type.state.as_str();
    let event_id = &event.id;

    match state {
        "pre" => FootballGameResponse::Pregame(to_pregame(event, competition, event_id, league)),
        "in" => FootballGameResponse::Live(to_live(event, competition, event_id, league)),
        "post" => FootballGameResponse::Final(to_final(event, competition, event_id, league)),
        _ => FootballGameResponse::Pregame(to_pregame(event, competition, event_id, league)),
    }
}

/// Transform to pregame response
fn to_pregame(
    event: &EspnEvent,
    competition: &EspnCompetition,
    event_id: &str,
    league: FootballLeague,
) -> FootballPregame {
    let (home_competitor, away_competitor) = get_competitors(&competition.competitors);
    let is_college = league.is_college();

    let venue = competition.venue.as_ref();
    let is_outdoor = venue.map(|v| !v.indoor.unwrap_or(false)).unwrap_or(true);

    FootballPregame {
        event_id: event_id.to_string(),
        home: crate::shared::transform::to_team(home_competitor, is_college),
        away: crate::shared::transform::to_team(away_competitor, is_college),
        start_time: parse_espn_date(&event.date),
        venue: venue.map(|v| v.full_name.clone()),
        broadcast: get_broadcast(event),
        weather: if is_outdoor {
            event.weather.as_ref().and_then(|w| {
                Some(Weather {
                    temp: w.temperature?,
                    description: w.display_value.clone()?,
                })
            })
        } else {
            None
        },
    }
}

/// Transform to live game response
fn to_live(
    event: &EspnEvent,
    competition: &EspnCompetition,
    event_id: &str,
    league: FootballLeague,
) -> FootballLive {
    let (home_competitor, away_competitor) = get_competitors(&competition.competitors);
    let is_college = league.is_college();
    let situation = competition.situation.as_ref();
    let last_play = situation.and_then(|s| s.last_play.as_ref()).map(to_last_play);

    // Compute clock_running based on game status and last play
    let clock_running = compute_clock_running(event, last_play.as_ref());

    // Weather is available for outdoor venues during live games
    let venue = competition.venue.as_ref();
    let is_outdoor = venue.map(|v| !v.indoor.unwrap_or(false)).unwrap_or(true);
    let weather = if is_outdoor {
        event.weather.as_ref().and_then(|w| {
            Some(Weather {
                temp: w.temperature?,
                description: w.display_value.clone()?,
            })
        })
    } else {
        None
    };

    FootballLive {
        event_id: event_id.to_string(),
        home: to_team_with_score(home_competitor, situation.and_then(|s| s.home_timeouts), is_college),
        away: to_team_with_score(away_competitor, situation.and_then(|s| s.away_timeouts), is_college),
        period: parse_period(event.status.period, &event.status.status_type.id),
        clock: event.status.display_clock.clone(),
        clock_running,
        situation: situation.and_then(|s| to_situation(s, home_competitor, away_competitor)),
        last_play,
        weather,
    }
}

/// Transform to final game response
fn to_final(
    event: &EspnEvent,
    competition: &EspnCompetition,
    event_id: &str,
    league: FootballLeague,
) -> FootballFinal {
    let (home_competitor, away_competitor) = get_competitors(&competition.competitors);
    let is_college = league.is_college();

    let home_score = parse_score(&home_competitor.score);
    let away_score = parse_score(&away_competitor.score);

    let situation = competition.situation.as_ref();

    FootballFinal {
        event_id: event_id.to_string(),
        home: to_team_with_score(home_competitor, situation.and_then(|s| s.home_timeouts), is_college),
        away: to_team_with_score(away_competitor, situation.and_then(|s| s.away_timeouts), is_college),
        status: if event.status.period > 4 {
            FinalStatus::FinalOvertime
        } else {
            FinalStatus::Final
        },
        winner: determine_winner(home_score, away_score),
    }
}

/// Transform ESPN competitor to our FootballTeamScore type
fn to_team_with_score(
    competitor: &EspnCompetitor,
    timeouts: Option<u8>,
    is_college: bool,
) -> FootballTeamScore {
    FootballTeamScore {
        abbreviation: competitor.team.abbreviation.clone(),
        color: parse_hex_color(competitor.team.color.as_deref().unwrap_or("000000")),
        record: competitor.records.first().map(|r| r.summary.clone()),
        rank: parse_rank(competitor, is_college),
        score: parse_score(&competitor.score),
        timeouts: timeouts.unwrap_or(0),
    }
}

/// Transform ESPN situation to our Situation type
fn to_situation(
    situation: &EspnSituation,
    home: &EspnCompetitor,
    away: &EspnCompetitor,
) -> Option<Situation> {
    // Convert i8 to u8, treating negative values (ESPN's sentinel for "not applicable") as None
    let down = situation.down.filter(|&v| v >= 0).map(|v| v as u8)?;
    let distance = situation.distance.filter(|&v| v >= 0).map(|v| v as u8)?;
    let yard_line = situation.yard_line.filter(|&v| v >= 0).map(|v| v as u8)?;
    let possession_id = situation.possession.as_ref()?;

    Some(Situation {
        down: parse_down(down)?,
        distance,
        yard_line,
        possession: determine_possession(possession_id, &home.team.id, &away.team.id),
        red_zone: situation.is_red_zone.unwrap_or(false),
    })
}

/// Parse ESPN period number to our FootballPeriod enum.
/// Status ID "23" = halftime.
fn parse_period(period: u8, status_id: &str) -> FootballPeriod {
    if status_id == "23" {
        return FootballPeriod::Halftime;
    }
    match period {
        1 => FootballPeriod::Q1,
        2 => FootballPeriod::Q2,
        3 => FootballPeriod::Q3,
        4 => FootballPeriod::Q4,
        5 => FootballPeriod::OT,
        6 => FootballPeriod::OT2,
        7 => FootballPeriod::OT3,
        8 => FootballPeriod::OT4,
        _ => FootballPeriod::OT4,
    }
}

/// Parse ESPN down number to our Down enum
fn parse_down(down: u8) -> Option<Down> {
    match down {
        1 => Some(Down::First),
        2 => Some(Down::Second),
        3 => Some(Down::Third),
        4 => Some(Down::Fourth),
        _ => None,
    }
}

/// Determine possession based on team IDs
fn determine_possession(possession_id: &str, home_id: &str, away_id: &str) -> Possession {
    if possession_id == home_id {
        Possession::Home
    } else if possession_id == away_id {
        Possession::Away
    } else {
        // Default to home if we can't determine
        Possession::Home
    }
}

/// Determine winner based on scores
fn determine_winner(home_score: u8, away_score: u8) -> Winner {
    if home_score > away_score {
        Winner::Home
    } else if away_score > home_score {
        Winner::Away
    } else {
        Winner::Tie
    }
}

/// Parse score string to u8
fn parse_score(score: &Option<String>) -> u8 {
    score
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Transform ESPN last play to our LastPlay type
fn to_last_play(last_play: &EspnLastPlay) -> LastPlay {
    LastPlay {
        play_type: PlayType::from_espn_id_with_context(
            &last_play.play_type.id,
            last_play.text.as_deref(),
        ),
        text: last_play.text.clone(),
    }
}

/// Compute whether the game clock is running based on NFL rules.
///
/// Uses a two-layer approach:
/// 1. Check game status (halftime, end of period, etc. = clock stopped)
/// 2. Check last play type and details (incomplete pass, timeout, out of bounds, etc.)
fn compute_clock_running(event: &EspnEvent, last_play: Option<&LastPlay>) -> bool {
    // Status IDs that indicate clock is definitely stopped
    // 1 = scheduled, 3 = final, 22 = end of period, 23 = halftime
    let status_id = &event.status.status_type.id;
    if status_id != "2" {
        // Not STATUS_IN_PROGRESS - clock is stopped
        return false;
    }

    // Check last play type
    if let Some(play) = last_play {
        // If play type always stops the clock, return false
        if play.play_type.stops_clock() {
            return false;
        }

        // For plays where clock depends on details (rush, reception, sack),
        // check if the play went out of bounds
        if play.play_type.clock_depends_on_details() {
            if let Some(text) = &play.text {
                let text_lower = text.to_lowercase();
                if text_lower.contains("out of bounds")
                    || text_lower.contains("pushed out")
                    || text_lower.contains("ran out")
                    || text_lower.contains("stepped out")
                {
                    return false;
                }
            }
            // In bounds - clock runs
            return true;
        }
    }

    // Default: assume clock is running during in-progress status
    true
}

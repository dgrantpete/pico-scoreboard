use crate::espn::types::{EspnCompetition, EspnCompetitor, EspnEvent, EspnLastPlay, EspnSituation};

use super::types::{
    Color, Down, FinalGame, FinalStatus, GameResponse, LastPlay, LiveGame, PlayType, Possession,
    PregameGame, Quarter, Situation, Team, TeamWithScore, Weather, Winner,
};

/// Transform an ESPN event into our API response format
pub fn transform(event: &EspnEvent) -> GameResponse {
    let competition = &event.competitions[0];
    let state = event.status.status_type.state.as_str();
    let event_id = &event.id;

    match state {
        "pre" => GameResponse::Pregame(to_pregame(event, competition, event_id)),
        "in" => GameResponse::Live(to_live(event, competition, event_id)),
        "post" => GameResponse::Final(to_final(event, competition, event_id)),
        _ => GameResponse::Pregame(to_pregame(event, competition, event_id)), // Default to pregame for unknown states
    }
}

/// Transform to pregame response
fn to_pregame(event: &EspnEvent, competition: &EspnCompetition, event_id: &str) -> PregameGame {
    let (home_competitor, away_competitor) = get_competitors(competition);

    let venue = competition.venue.as_ref();
    let is_outdoor = venue.map(|v| !v.indoor.unwrap_or(false)).unwrap_or(true);

    PregameGame {
        event_id: event_id.to_string(),
        home: to_team(home_competitor),
        away: to_team(away_competitor),
        start_time: event.date.clone(),  // ISO datetime for firmware to parse
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
fn to_live(event: &EspnEvent, competition: &EspnCompetition, event_id: &str) -> LiveGame {
    let (home_competitor, away_competitor) = get_competitors(competition);
    let situation = competition.situation.as_ref();
    let last_play = situation.and_then(|s| s.last_play.as_ref()).map(to_last_play);

    // Compute clock_running based on game status and last play
    let clock_running = compute_clock_running(event, last_play.as_ref());

    LiveGame {
        event_id: event_id.to_string(),
        home: to_team_with_score(home_competitor, situation.and_then(|s| s.home_timeouts)),
        away: to_team_with_score(away_competitor, situation.and_then(|s| s.away_timeouts)),
        quarter: parse_quarter(event.status.period),
        clock: event.status.display_clock.clone(),
        clock_running,
        situation: situation.and_then(|s| to_situation(s, home_competitor, away_competitor)),
        last_play,
    }
}

/// Transform to final game response
fn to_final(event: &EspnEvent, competition: &EspnCompetition, event_id: &str) -> FinalGame {
    let (home_competitor, away_competitor) = get_competitors(competition);

    let home_score = parse_score(&home_competitor.score);
    let away_score = parse_score(&away_competitor.score);

    // Timeouts don't really matter in final, but we include them for consistency
    let situation = competition.situation.as_ref();

    FinalGame {
        event_id: event_id.to_string(),
        home: to_team_with_score(home_competitor, situation.and_then(|s| s.home_timeouts)),
        away: to_team_with_score(away_competitor, situation.and_then(|s| s.away_timeouts)),
        status: if event.status.period > 4 {
            FinalStatus::FinalOvertime
        } else {
            FinalStatus::Final
        },
        winner: determine_winner(home_score, away_score),
    }
}

/// Extract home and away competitors from competition
fn get_competitors(competition: &EspnCompetition) -> (&EspnCompetitor, &EspnCompetitor) {
    let home = competition
        .competitors
        .iter()
        .find(|c| c.home_away == "home")
        .expect("No home competitor found");

    let away = competition
        .competitors
        .iter()
        .find(|c| c.home_away == "away")
        .expect("No away competitor found");

    (home, away)
}

/// Transform ESPN competitor to our Team type
fn to_team(competitor: &EspnCompetitor) -> Team {
    Team {
        abbreviation: competitor.team.abbreviation.clone(),
        color: parse_hex_color(competitor.team.color.as_deref().unwrap_or("000000")),
        record: competitor.records.first().map(|r| r.summary.clone()),
    }
}

/// Transform ESPN competitor to our TeamWithScore type
fn to_team_with_score(competitor: &EspnCompetitor, timeouts: Option<u8>) -> TeamWithScore {
    TeamWithScore {
        abbreviation: competitor.team.abbreviation.clone(),
        color: parse_hex_color(competitor.team.color.as_deref().unwrap_or("000000")),
        record: competitor.records.first().map(|r| r.summary.clone()),
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

/// Get broadcast network from event
fn get_broadcast(event: &EspnEvent) -> Option<String> {
    event
        .geo_broadcasts
        .first()
        .and_then(|b| b.media.as_ref())
        .map(|m| m.short_name.clone())
}

/// Parse a hex color string (without #) to RGB
fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');

    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);

    Color { r, g, b }
}

/// Parse ESPN period number to our Quarter enum
fn parse_quarter(period: u8) -> Quarter {
    match period {
        1 => Quarter::First,
        2 => Quarter::Second,
        3 => Quarter::Third,
        4 => Quarter::Fourth,
        5 => Quarter::Overtime,
        _ => Quarter::DoubleOvertime,
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

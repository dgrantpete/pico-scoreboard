use crate::espn::types::{EspnCompetition, EspnCompetitor, EspnEvent, EspnSituation};

use super::types::{
    Color, Down, FinalGame, FinalStatus, GameResponse, LiveGame, Possession, PregameGame, Quarter,
    Situation, Team, TeamWithScore, Weather, Winner,
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
        start_time: event.status.status_type.short_detail.clone(),
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

    LiveGame {
        event_id: event_id.to_string(),
        home: to_team_with_score(home_competitor, situation.and_then(|s| s.home_timeouts)),
        away: to_team_with_score(away_competitor, situation.and_then(|s| s.away_timeouts)),
        quarter: parse_quarter(event.status.period),
        clock: event.status.display_clock.clone(),
        situation: situation.and_then(|s| to_situation(s, home_competitor, away_competitor)),
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
    // All fields must be present for a valid situation
    let down = situation.down?;
    let distance = situation.distance?;
    let yard_line = situation.yard_line?;
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

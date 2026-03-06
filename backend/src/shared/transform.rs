use chrono::DateTime;

use crate::espn::types::{EspnCompetitor, EspnEvent};

use super::types::{Color, Team, Winner};

/// Parse an ESPN ISO 8601 date string to a Unix timestamp (seconds).
/// Returns 0 if the date can't be parsed.
pub fn parse_espn_date(date: &str) -> i64 {
    DateTime::parse_from_rfc3339(date)
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|_| {
            tracing::warn!(date = date, "Failed to parse ESPN date as RFC 3339");
            0
        })
}

/// Parse a hex color string (without #) to RGB
pub fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');

    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);

    Color { r, g, b }
}

/// Get broadcast network from event
pub fn get_broadcast(event: &EspnEvent) -> Option<String> {
    event
        .geo_broadcasts
        .first()
        .and_then(|b| b.media.as_ref())
        .map(|m| m.short_name.clone())
}

/// Determine winner based on scores (u16 to support basketball)
pub fn determine_winner(home_score: u16, away_score: u16) -> Winner {
    if home_score > away_score {
        Winner::Home
    } else if away_score > home_score {
        Winner::Away
    } else {
        Winner::Tie
    }
}

/// Parse college ranking from ESPN competitor.
/// Returns None for pro leagues or unranked teams (ESPN uses 99 for unranked).
pub fn parse_rank(competitor: &EspnCompetitor, is_college: bool) -> Option<u8> {
    if !is_college {
        return None;
    }
    competitor
        .curated_rank
        .as_ref()
        .and_then(|r| r.current)
        .filter(|&rank| rank < 99)
}

/// Transform ESPN competitor to our shared Team type (for pregame, no score)
pub fn to_team(competitor: &EspnCompetitor, is_college: bool) -> Team {
    Team {
        abbreviation: competitor.team.abbreviation.clone(),
        color: parse_hex_color(competitor.team.color.as_deref().unwrap_or("000000")),
        record: competitor.records.first().map(|r| r.summary.clone()),
        rank: parse_rank(competitor, is_college),
    }
}

/// Extract home and away competitors from competition
pub fn get_competitors(
    competitors: &[EspnCompetitor],
) -> (&EspnCompetitor, &EspnCompetitor) {
    let home = competitors
        .iter()
        .find(|c| c.home_away == "home")
        .expect("No home competitor found");

    let away = competitors
        .iter()
        .find(|c| c.home_away == "away")
        .expect("No away competitor found");

    (home, away)
}

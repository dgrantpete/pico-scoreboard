//! Cross-sport transform helpers that turn ESPN competitors into the shared
//! outbound primitives. The three sports' inbound `EspnCompetitor` structs
//! differ (MLB carries probables + line scores, NBA line scores, soccer
//! neither), so these helpers work through the small [`Competitor`] accessor
//! trait rather than one shared struct.

use crate::error::AppError;
use crate::espn::types::{EspnLinescore, EspnRecord, EspnTeam, HomeAway};
use crate::shared::game::Record;
use crate::shared::team::{TeamColors, TeamState, order_home_away, parse_hex_rgb};

/// The slice of an ESPN competitor the shared builders need. Each sport's
/// inbound `EspnCompetitor` implements it next to its declaration.
pub(crate) trait Competitor {
    fn home_away(&self) -> HomeAway;
    fn team(&self) -> &EspnTeam;
    fn score(&self) -> &str;
}

/// Parse a competitor's primary and alternate colors. No log here: the
/// returned `InvalidTeamColor` carries team + raw value, and every 5xx response
/// is logged centrally (see `AppError::into_response`).
pub(crate) fn competitor_colors(c: &impl Competitor) -> Result<TeamColors, AppError> {
    let team = c.team();
    let primary = parse_hex_rgb(&team.color, &team.abbreviation)?;
    let alternate = parse_hex_rgb(&team.alternate_color, &team.abbreviation)?;
    Ok(TeamColors { primary, alternate })
}

/// Parse a competitor's score string into a `u32`.
pub(crate) fn parse_score(c: &impl Competitor) -> Result<u32, AppError> {
    let abbreviation = &c.team().abbreviation;
    c.score().parse::<u32>().map_err(|e| {
        let json_path =
            format!("events[?].competitions[0].competitors[{abbreviation}].score");
        tracing::error!(
            json_path = %json_path,
            team = %abbreviation,
            raw_score = %c.score(),
            error = %e,
            "ESPN competitor score failed to parse as u32"
        );
        AppError::EspnDeserialize {
            url: String::new(),
            json_path,
            message: format!("invalid score '{}': {}", c.score(), e),
        }
    })
}

/// Build a live `TeamState` from an ESPN competitor (score + colors).
pub(crate) fn competitor_to_team_state(c: &impl Competitor) -> Result<TeamState, AppError> {
    Ok(TeamState {
        abbreviation: c.team().abbreviation.clone(),
        score: parse_score(c)?,
        colors: competitor_colors(c)?,
    })
}

/// Order two competitors into (home, away) by their `homeAway` markers.
pub(crate) fn order_competitors<C: Competitor>(
    event_id: &str,
    competitors: [C; 2],
) -> Result<(C, C), AppError> {
    order_home_away(
        event_id,
        competitors,
        |c| c.home_away(),
        |c| c.team().abbreviation.as_str(),
    )
}

/// Parse the overall win-loss record from a competitor's records list.
///
/// The `type=="total"` entry carries the season record as "47-42"
/// (`abbreviation` varies — "Game", "Total" — so it is not matched on). A
/// missing or malformed entry yields `None` (the field is decorative).
pub(crate) fn parse_record(records: &[EspnRecord]) -> Option<Record> {
    let summary = records.iter().find(|r| r.r#type == "total")?;
    let parsed = summary
        .summary
        .split_once('-')
        .and_then(|(w, l)| Some((w.parse::<u16>().ok()?, l.parse::<u16>().ok()?)));
    match parsed {
        Some((wins, losses)) => Some(Record { wins, losses }),
        None => {
            tracing::warn!(
                summary = %summary.summary,
                "ESPN total record not in 'W-L' form — dropping record"
            );
            None
        }
    }
}

/// Per-period scores for one team, ordered by period and clamped to `u8`.
///
/// ESPN sends floats (`value`); a single period never scores past 255, so the
/// clamp only guards against corrupt data. A short line (a walk-off leaves the
/// home side short) or a long one (extras / overtime) simply changes the
/// vector length.
pub(crate) fn linescore_bytes(linescores: &[EspnLinescore]) -> Vec<u8> {
    let mut ordered: Vec<&EspnLinescore> = linescores.iter().collect();
    ordered.sort_by_key(|l| l.period);
    ordered
        .into_iter()
        .map(|l| l.value.clamp(0.0, u8::MAX as f64) as u8)
        .collect()
}

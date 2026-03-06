use crate::espn::types::{EspnCompetitor, EspnEvent, EspnSummary};
use crate::shared::transform::{
    determine_winner, get_broadcast, get_competitors, parse_espn_date, parse_hex_color, parse_rank,
    to_team,
};
use crate::sport::{BasketballLeague, EspnLeague};

use super::types::{
    BasketballFinal, BasketballFinalDetail, BasketballGameDetail, BasketballGameResponse,
    BasketballLive, BasketballLiveDetail, BasketballPeriod, BasketballPregame,
    BasketballTeamScore, BasketballTeamScoreDetail,
};
use crate::shared::types::FinalStatus;

// ── Scoreboard transform (list endpoints, no fouls) ──

/// Transform an ESPN scoreboard event into a basketball game response.
pub fn transform_from_scoreboard(
    event: &EspnEvent,
    league: BasketballLeague,
) -> BasketballGameResponse {
    let competition = &event.competitions[0];
    let (home, away) = get_competitors(&competition.competitors);
    let state = event.status.status_type.state.as_str();

    match state {
        "pre" => BasketballGameResponse::Pregame(to_pregame(event, home, away, league)),
        "in" => BasketballGameResponse::Live(to_live(event, home, away, league)),
        "post" => BasketballGameResponse::Final(to_final(event, home, away, league)),
        _ => BasketballGameResponse::Pregame(to_pregame(event, home, away, league)),
    }
}

fn to_pregame(
    event: &EspnEvent,
    home: &EspnCompetitor,
    away: &EspnCompetitor,
    league: BasketballLeague,
) -> BasketballPregame {
    let is_college = league.is_college();
    let venue = event.competitions[0].venue.as_ref();

    BasketballPregame {
        event_id: event.id.clone(),
        home: to_team(home, is_college),
        away: to_team(away, is_college),
        start_time: parse_espn_date(&event.date),
        venue: venue.map(|v| v.full_name.clone()),
        broadcast: get_broadcast(event),
    }
}

fn to_live(
    event: &EspnEvent,
    home: &EspnCompetitor,
    away: &EspnCompetitor,
    league: BasketballLeague,
) -> BasketballLive {
    let is_college = league.is_college();

    BasketballLive {
        event_id: event.id.clone(),
        home: to_team_score(home, is_college),
        away: to_team_score(away, is_college),
        period: parse_period(event.status.period, league, &event.status.status_type.id),
        clock: event.status.display_clock.clone(),
    }
}

fn to_final(
    event: &EspnEvent,
    home: &EspnCompetitor,
    away: &EspnCompetitor,
    league: BasketballLeague,
) -> BasketballFinal {
    let is_college = league.is_college();
    let home_score = parse_score_u16(&home.score);
    let away_score = parse_score_u16(&away.score);

    let regulation_periods = if league.is_college() { 2 } else { 4 };

    BasketballFinal {
        event_id: event.id.clone(),
        home: to_team_score(home, is_college),
        away: to_team_score(away, is_college),
        status: if event.status.period > regulation_periods {
            FinalStatus::FinalOvertime
        } else {
            FinalStatus::Final
        },
        winner: determine_winner(home_score, away_score),
    }
}

fn to_team_score(competitor: &EspnCompetitor, is_college: bool) -> BasketballTeamScore {
    BasketballTeamScore {
        abbreviation: competitor.team.abbreviation.clone(),
        color: parse_hex_color(competitor.team.color.as_deref().unwrap_or("000000")),
        record: competitor.records.first().map(|r| r.summary.clone()),
        rank: parse_rank(competitor, is_college),
        score: parse_score_u16(&competitor.score),
    }
}

// ── Summary transform (detail endpoints, with fouls) ──

/// Transform an ESPN summary response into a basketball game detail.
pub fn transform_from_summary(
    summary: &EspnSummary,
    league: BasketballLeague,
) -> BasketballGameDetail {
    let competition = &summary.header.competitions[0];
    let (home, away) = get_competitors(&competition.competitors);
    let state = competition.status.status_type.state.as_str();
    let is_college = league.is_college();

    match state {
        "pre" => {
            let venue = competition.venue.as_ref();
            BasketballGameDetail::Pregame(BasketballPregame {
                event_id: summary.header.id.clone(),
                home: to_team(home, is_college),
                away: to_team(away, is_college),
                start_time: 0, // summary endpoint doesn't carry event date
                venue: venue.map(|v| v.full_name.clone()),
                broadcast: None, // summary doesn't carry broadcast info the same way
            })
        }
        "in" => {
            let home_fouls = extract_fouls(summary, &home.team.id);
            let away_fouls = extract_fouls(summary, &away.team.id);

            BasketballGameDetail::Live(BasketballLiveDetail {
                event_id: summary.header.id.clone(),
                home: to_team_score_detail(home, is_college, home_fouls),
                away: to_team_score_detail(away, is_college, away_fouls),
                period: parse_period(competition.status.period, league, &competition.status.status_type.id),
                clock: competition.status.display_clock.clone(),
            })
        }
        "post" => {
            let home_fouls = extract_fouls(summary, &home.team.id);
            let away_fouls = extract_fouls(summary, &away.team.id);
            let home_score = parse_score_u16(&home.score);
            let away_score = parse_score_u16(&away.score);

            let regulation_periods = if league.is_college() { 2 } else { 4 };

            BasketballGameDetail::Final(BasketballFinalDetail {
                event_id: summary.header.id.clone(),
                home: to_team_score_detail(home, is_college, home_fouls),
                away: to_team_score_detail(away, is_college, away_fouls),
                status: if competition.status.period > regulation_periods {
                    FinalStatus::FinalOvertime
                } else {
                    FinalStatus::Final
                },
                winner: determine_winner(home_score, away_score),
            })
        }
        _ => {
            let venue = competition.venue.as_ref();
            BasketballGameDetail::Pregame(BasketballPregame {
                event_id: summary.header.id.clone(),
                home: to_team(home, is_college),
                away: to_team(away, is_college),
                start_time: 0, // summary endpoint doesn't carry event date
                venue: venue.map(|v| v.full_name.clone()),
                broadcast: None,
            })
        }
    }
}

fn to_team_score_detail(
    competitor: &EspnCompetitor,
    is_college: bool,
    fouls: u8,
) -> BasketballTeamScoreDetail {
    BasketballTeamScoreDetail {
        abbreviation: competitor.team.abbreviation.clone(),
        color: parse_hex_color(competitor.team.color.as_deref().unwrap_or("000000")),
        record: competitor.records.first().map(|r| r.summary.clone()),
        rank: parse_rank(competitor, is_college),
        score: parse_score_u16(&competitor.score),
        fouls,
    }
}

/// Extract fouls for a team from the boxscore.
/// Searches boxscore.teams[] for a matching team ID, then finds the "fouls" stat.
fn extract_fouls(summary: &EspnSummary, team_id: &str) -> u8 {
    summary
        .boxscore
        .as_ref()
        .and_then(|bs| {
            bs.teams
                .iter()
                .find(|t| t.team.id == team_id)
                .and_then(|t| {
                    t.statistics
                        .iter()
                        .find(|s| s.name == "fouls")
                        .and_then(|s| s.display_value.parse().ok())
                })
        })
        .unwrap_or(0)
}

// ── Shared helpers ──

/// Parse ESPN period number to BasketballPeriod.
/// NBA: 1=Q1, 2=Q2, 3=Q3, 4=Q4, 5+=OT.
/// NCAAB: 1=H1, 2=H2, 3+=OT.
/// Status ID "23" = halftime.
fn parse_period(period: u8, league: BasketballLeague, status_id: &str) -> BasketballPeriod {
    if status_id == "23" {
        return BasketballPeriod::Halftime;
    }
    if league.is_college() {
        match period {
            1 => BasketballPeriod::H1,
            2 => BasketballPeriod::H2,
            3 => BasketballPeriod::OT,
            4 => BasketballPeriod::OT2,
            5 => BasketballPeriod::OT3,
            6 => BasketballPeriod::OT4,
            _ => BasketballPeriod::OT4,
        }
    } else {
        match period {
            1 => BasketballPeriod::Q1,
            2 => BasketballPeriod::Q2,
            3 => BasketballPeriod::Q3,
            4 => BasketballPeriod::Q4,
            5 => BasketballPeriod::OT,
            6 => BasketballPeriod::OT2,
            7 => BasketballPeriod::OT3,
            8 => BasketballPeriod::OT4,
            _ => BasketballPeriod::OT4,
        }
    }
}

/// Parse score string to u16 (basketball scores routinely exceed 100).
fn parse_score_u16(score: &Option<String>) -> u16 {
    score
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

//! Hockey lane corpus + NHL.md §3 semantics tests.
//!
//! Fixtures are single EVENT objects (`backend/testdata/hockey/nhl/*.json`),
//! wrapped here as `{"events":[…]}` scoreboard bodies, each paired with the
//! raw summary body of the same moment (`*.summary.json`; pregame has none).
//! The expected values are `PROVENANCE.md`'s evidence; every wire value the
//! lane produces is encoded and decoded back through `scoreboard_wire::hockey`.

use std::fs;
use std::path::PathBuf;

use scoreboard_espn::common::{IgnoreQuirks, Quirk, Quirks};
use scoreboard_espn::hockey::{
    DetailReport, Extract, ExtractError, GameExtractor, GameOutcome, Kind, ListExtractor, Summary,
    SummaryContext, SummaryExtractor, SummaryReport, TransformError,
};
use scoreboard_wire::hockey::{
    self as wire, Coordinate, Game, GoalTag, Shootout, ShootoutAttempts, Shots, Strength,
    StrengthKind,
};
use scoreboard_wire::{GameState, LivePhase, Side, SliceSink};

/// Every committed pair, in `PROVENANCE.md` order.
const FIXTURES: &[&str] = &[
    "pregame",
    "puck_drop",
    "in_progress",
    "end_of_period",
    "power_play",
    "empty_net",
    "four_on_four",
    "overtime",
    "shootout",
    "onice_glitch",
    "final",
    "final_ot",
    "final_so",
    "final_2ot",
    "playoff_clincher",
    "delayed_start",
    "penalty_placeholder",
    "shootout_teamless_attempt",
    "shootout_failed_attempt",
    "final_so_no_stars",
];

fn testdata(rel: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../backend/testdata/hockey/nhl"
    ))
    .join(rel)
}

fn fixture(name: &str) -> String {
    let path = testdata(&format!("{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

fn summary_text(name: &str) -> Option<String> {
    let path = testdata(&format!("{name}.summary.json"));
    path.exists()
        .then(|| fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}")))
}

fn wrap(events: &[&str]) -> String {
    format!("{{\"events\":[{}]}}", events.join(","))
}

fn parse(json: &str) -> serde_json::Value {
    serde_json::from_str(json).expect("fixture parses")
}

fn id_of(event_json: &str) -> String {
    parse(event_json)["id"]
        .as_str()
        .expect("string id")
        .to_string()
}

fn mutate(json: &str, f: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut value = parse(json);
    f(&mut value);
    value.to_string()
}

/// Records quirks for assertions.
#[derive(Debug, Default)]
struct RecQuirks(Vec<Quirk>);

impl Quirks for RecQuirks {
    fn quirk(&mut self, quirk: Quirk) {
        self.0.push(quirk);
    }
}

fn chunks(len: usize, sizes: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let (mut pos, mut i) = (0, 0);
    while pos < len {
        let end = (pos + sizes[i % sizes.len()].max(1)).min(len);
        out.push((pos, end));
        pos = end;
        i += 1;
    }
    out
}

fn extract_sized(body: &str, target: &str, sizes: &[usize]) -> DetailReport<RecQuirks> {
    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = GameExtractor::new(target, RecQuirks::default(), &mut scratch).unwrap();
    for (start, end) in chunks(body.len(), sizes) {
        extractor.write(&body.as_bytes()[start..end]).unwrap();
    }
    extractor.finish().unwrap()
}

fn detail(body: &str, target: &str) -> DetailReport<RecQuirks> {
    extract_sized(body, target, &[usize::MAX])
}

fn found(body: &str, target: &str) -> Extract {
    match detail(body, target).outcome {
        GameOutcome::Found(extract) => extract,
        other => panic!("expected Found for {target}, got {other:?}"),
    }
}

fn summarize_sized(
    text: &str,
    context: SummaryContext,
    sizes: &[usize],
) -> SummaryReport<RecQuirks> {
    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = SummaryExtractor::new(context, RecQuirks::default(), &mut scratch).unwrap();
    for (start, end) in chunks(text.len(), sizes) {
        extractor.write(&text.as_bytes()[start..end]).unwrap();
    }
    extractor.finish().unwrap()
}

fn summarize(text: &str, context: SummaryContext) -> SummaryReport<RecQuirks> {
    summarize_sized(text, context, &[usize::MAX])
}

/// The scoreboard extract of a fixture and its summary, whole-buffer.
fn pair(name: &str) -> (Extract, Option<Summary>) {
    let event = fixture(name);
    let extract = found(&wrap(&[&event]), &id_of(&event));
    let summary =
        summary_text(name).map(|text| summarize(&text, extract.summary_context()).summary);
    (extract, summary)
}

/// The summary report of a fixture, quirks included.
fn summary_report(name: &str) -> (Extract, SummaryReport<RecQuirks>) {
    let event = fixture(name);
    let extract = found(&wrap(&[&event]), &id_of(&event));
    let report = summarize(
        &summary_text(name).expect("pair"),
        extract.summary_context(),
    );
    (extract, report)
}

fn encode(game: &Game<'_>) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    let mut sink = SliceSink::new(&mut buf);
    wire::encode(game, &mut sink).expect("encode fits");
    sink.written().to_vec()
}

/// Every value the lane emits must survive the wire: encode, decode, equal.
fn round_trip(game: &Game<'_>) -> Vec<u8> {
    let bytes = encode(game);
    let decoded = wire::decode(&bytes).expect("decodes");
    assert_eq!(&decoded, game, "round trip");
    bytes
}

fn live<'a>(game: &'a Game<'a>) -> &'a wire::Live<'a> {
    match game {
        Game::Live(live) => live,
        other => panic!("expected live, got {other:?}"),
    }
}

fn final_<'a>(game: &'a Game<'a>) -> &'a wire::Final<'a> {
    match game {
        Game::Final(game) => game,
        other => panic!("expected final, got {other:?}"),
    }
}

fn pregame<'a>(game: &'a Game<'a>) -> &'a wire::Pregame<'a> {
    match game {
        Game::Pregame(game) => game,
        other => panic!("expected pregame, got {other:?}"),
    }
}

fn shots(away: u8, home: u8) -> Option<Shots> {
    Some(Shots { away, home })
}

fn strength(kind: StrengthKind, away_skaters: u8, home_skaters: u8) -> Option<Strength> {
    Some(Strength {
        kind,
        away_skaters,
        home_skaters,
    })
}

fn attempts(taken: u8, scored: u16) -> ShootoutAttempts {
    ShootoutAttempts { taken, scored }
}

fn star_names<'a>(game: &'a wire::Final<'a>) -> Option<Vec<(&'a str, Option<Side>)>> {
    game.stars
        .as_ref()
        .map(|stars| stars.iter().map(|star| (star.name, star.side)).collect())
}

fn last_play<'a>(game: &'a wire::Live<'a>) -> Option<(&'a str, &'a str)> {
    game.last_play.map(|play| (play.id, play.text))
}

// ----------------------------------------------------------------- corpus

/// Everything `PROVENANCE.md` says about a pair, checked on the decoded wire
/// values: the scoreboard alone (`without`) and combined with its summary
/// (`with`, `None` for the pregame).
fn check_fixture(name: &str, without: &Game<'_>, with: Option<&Game<'_>>) {
    match name {
        "pregame" => {
            let game = pregame(without);
            assert_eq!(game.game_id, "401892505");
            assert_eq!(game.start_time, 1_792_024_200, "2026-10-15T00:30Z");
            assert_eq!(game.venue, "Ball Arena");
            assert_eq!(game.info_secondary, "");
            assert!(!game.playoffs);
            assert_eq!(game.away.abbreviation, "DAL");
            assert_eq!(
                (game.away.colors.primary, game.away.colors.alternate),
                (0x20864c, 0x000000)
            );
            assert_eq!(
                game.away.record,
                Some(wire::Record {
                    wins: 2,
                    losses: 3,
                    ot_losses: 1
                })
            );
            assert_eq!(game.away.goalie, Some("C. DeSmith"));
            assert_eq!(game.home.abbreviation, "COL");
            assert_eq!(
                (game.home.colors.primary, game.home.colors.alternate),
                (0x860038, 0x005ea3)
            );
            assert_eq!(
                game.home.record,
                Some(wire::Record {
                    wins: 4,
                    losses: 2,
                    ot_losses: 0
                })
            );
            assert_eq!(game.home.goalie, Some("S. Wedgewood"));
            assert!(with.is_none());
        }
        "delayed_start" => {
            // STATUS_DELAYED at period 0 is a pregame; the summary adds nothing.
            for game in [Some(without), with].into_iter().flatten() {
                let game = pregame(game);
                assert_eq!(game.game_id, "401891782");
                assert_eq!(game.start_time, 1_790_989_200, "2026-10-03T01:00Z");
                assert_eq!(game.venue, "American Airlines Center");
                assert_eq!(
                    (game.away.abbreviation, game.home.abbreviation),
                    ("STL", "DAL")
                );
                // No alternateColor on either team: the primary stands in.
                assert_eq!(
                    (game.away.colors.primary, game.away.colors.alternate),
                    (0x0070b9, 0x0070b9)
                );
                assert_eq!(
                    (game.home.colors.primary, game.home.colors.alternate),
                    (0x20864c, 0x20864c)
                );
                let zero = Some(wire::Record {
                    wins: 0,
                    losses: 0,
                    ot_losses: 0,
                });
                assert_eq!((game.away.record, game.home.record), (zero, zero));
                assert_eq!(
                    (game.away.goalie, game.home.goalie),
                    (Some("J. Hofer"), Some("J. Oettinger"))
                );
            }
        }
        "puck_drop" => {
            let (a, b) = (live(without), live(with.unwrap()));
            for game in [a, b] {
                assert_eq!(
                    (game.period, game.clock, game.phase),
                    (1, "19:26", LivePhase::InProgress)
                );
                assert_eq!((game.away.score, game.home.score), (0, 0));
                assert_eq!(game.shots, None, "statistics [] and no boxscore shotsTotal");
                assert!(game.goals.is_empty());
                assert_eq!(game.shootout, None);
                assert_eq!(game.last_play, None);
            }
            assert_eq!(a.strength, None);
            // Empty goalie groups: the quiet fallback, the face-off's Even.
            assert_eq!(b.strength, strength(StrengthKind::Even, 0, 0));
        }
        "in_progress" => {
            let (a, b) = (live(without), live(with.unwrap()));
            for game in [a, b] {
                assert_eq!((game.period, game.clock), (2, "9:32"));
                assert_eq!(
                    (game.away.abbreviation, game.home.abbreviation),
                    ("BOS", "SJ")
                );
                assert_eq!((game.away.score, game.home.score), (3, 2));
                assert_eq!(game.shots, shots(10, 19), "identity equals the boxscore");
            }
            assert_eq!(a.strength, None);
            assert_eq!(a.last_play, None, "a blocked shot never flashes");
            assert!(a.goals.is_empty());
            assert_eq!(b.strength, strength(StrengthKind::Even, 5, 5));
            assert_eq!(b.goals.len(), 5);
            let last = b.goals.last().unwrap();
            assert_eq!(
                (last.side, last.tag, last.period, last.elapsed),
                (Side::Away, GoalTag::Goal, 2, 522)
            );
            assert_eq!(last.coordinate, Some(Coordinate { x: 83, y: 2 }));
            assert_eq!(b.last_scorer, "M. Poitras");
            assert_eq!(
                last_play(b),
                Some((
                    "401892503000004560",
                    "GOAL! Matthew Poitras Goal (2) Tip-In, assists: Mark Kastelic (4), Morgan Geekie (4)"
                ))
            );
            assert_eq!(b.shootout, None);
        }
        "end_of_period" => {
            let (a, b) = (live(without), live(with.unwrap()));
            for game in [a, b] {
                assert_eq!(
                    (game.period, game.clock, game.phase),
                    (2, "0:00", LivePhase::EndOfPeriod)
                );
                assert_eq!((game.away.score, game.home.score), (4, 3));
                assert_eq!(game.shots, shots(30, 14));
            }
            assert_eq!(a.last_play, None, "a period end never flashes");
            assert_eq!(b.strength, strength(StrengthKind::Even, 5, 5));
            assert_eq!(b.goals.len(), 7);
            assert_eq!(b.last_scorer, "J. Bratt");
            assert_eq!(
                last_play(b),
                Some((
                    "401892488000000430",
                    "Amadeus Lombardi Hooking against Emmitt Finnie"
                ))
            );
        }
        "power_play" => {
            let (a, b) = (live(without), live(with.unwrap()));
            assert_eq!((a.period, a.clock), (2, "5:23"));
            assert_eq!((a.away.abbreviation, a.home.abbreviation), ("UTA", "BUF"));
            assert_eq!((a.away.score, a.home.score), (1, 1));
            assert_eq!(a.shots, shots(17, 10), "the identity, one shot off");
            assert_eq!(a.last_play, None);
            assert_eq!(b.shots, shots(16, 10), "the boxscore wins");
            assert_eq!(
                b.strength,
                strength(StrengthKind::PowerPlay(Side::Away), 5, 4)
            );
            assert_eq!(b.goals.len(), 2);
            let ppg = b.goals.get(0).unwrap();
            assert_eq!(
                (ppg.side, ppg.tag, ppg.period, ppg.elapsed),
                (Side::Home, GoalTag::PowerPlay, 1, 480)
            );
            assert_eq!(
                ppg.coordinate,
                Some(Coordinate { x: -84, y: 21 }),
                "home rotated"
            );
            let tying = b.goals.get(1).unwrap();
            assert_eq!((tying.side, tying.tag), (Side::Away, GoalTag::Goal));
            assert_eq!(tying.coordinate, Some(Coordinate { x: 87, y: 0 }));
            assert_eq!(b.last_scorer, "K. Yamamoto");
            assert_eq!(
                last_play(b),
                Some((
                    "401892472000007210",
                    "Owen Power Delaying Game - Puck over glass"
                ))
            );
        }
        "empty_net" => {
            let (a, b) = (live(without), live(with.unwrap()));
            assert_eq!((a.period, a.clock), (3, "1:04"));
            assert_eq!((a.away.score, a.home.score), (3, 4));
            assert_eq!((a.shots, b.shots), (shots(25, 19), shots(25, 19)));
            assert_eq!(
                b.strength,
                strength(StrengthKind::EmptyNet(Side::Away), 6, 5)
            );
            assert_eq!(b.goals.len(), 7);
            assert_eq!(b.goals.get(2).unwrap().tag, GoalTag::PowerPlay);
            assert_eq!(b.goals.last().unwrap().tag, GoalTag::Goal);
            assert_eq!(b.last_scorer, "T. Teravainen");
            assert_eq!(
                last_play(b),
                Some((
                    "401892496000009680",
                    "GOAL! Teuvo Teravainen Goal (2) Wrist Shot, assists: Ryan Greene (2), Cole Smith (1)"
                ))
            );
        }
        "four_on_four" => {
            let (a, b) = (live(without), live(with.unwrap()));
            assert_eq!((a.period, a.clock), (1, "5:10"));
            assert_eq!((a.away.score, a.home.score), (1, 0));
            assert_eq!((a.shots, b.shots), (shots(7, 7), shots(7, 7)));
            assert_eq!(b.strength, strength(StrengthKind::Matched, 4, 4));
            assert_eq!(b.goals.len(), 1);
            let goal = b.goals.get(0).unwrap();
            assert_eq!(
                (goal.side, goal.elapsed, goal.coordinate),
                (Side::Away, 692, Some(Coordinate { x: 69, y: -5 }))
            );
            assert_eq!(b.last_scorer, "A. Lehkonen");
            assert_eq!(
                last_play(b),
                Some((
                    "401892498000003420",
                    "Sam Malinski Roughing against Colin Blackwell"
                ))
            );
        }
        "overtime" => {
            let (a, b) = (live(without), live(with.unwrap()));
            assert_eq!((a.period, a.clock), (4, "0:39"));
            assert_eq!((a.away.score, a.home.score), (3, 3));
            assert_eq!(a.shots, shots(23, 41));
            assert_eq!(b.shots, shots(22, 41));
            assert_eq!(
                b.strength,
                strength(StrengthKind::Matched, 3, 3),
                "3-on-3 overtime"
            );
            assert_eq!(b.goals.len(), 6);
            let penalty_shot = b.goals.last().unwrap();
            assert_eq!(
                (penalty_shot.side, penalty_shot.tag),
                (Side::Home, GoalTag::Goal),
                "902 is a plain GOAL"
            );
            assert_eq!(b.last_scorer, "J. Eichel");
            assert_eq!(b.shootout, None, "period 4 is not the shootout");
            assert_eq!(
                last_play(b),
                Some((
                    "401892464000009690",
                    "Bo Groulx Holding against Ivan Barbashev"
                ))
            );
        }
        "shootout" => {
            let (a, b) = (live(without), live(with.unwrap()));
            for game in [a, b] {
                assert_eq!((game.period, game.clock), (5, "0:00"));
                assert!(!game.playoffs);
                assert_eq!(
                    (game.away.score, game.home.score),
                    (2, 2),
                    "the tied game score"
                );
                assert_eq!(game.shots, shots(18, 20));
                assert_eq!(game.strength, None, "no strength in a shootout");
            }
            assert_eq!(a.shootout, None, "no summary, no tracker");
            assert_eq!(a.last_play, None);
            assert_eq!(b.goals.len(), 4);
            assert_eq!(b.last_scorer, "D. Batherson");
            assert_eq!(
                b.shootout,
                Some(Shootout {
                    away: attempts(2, 0b01),
                    home: attempts(2, 0b10),
                    next_shooter: Some(Side::Home),
                })
            );
            assert_eq!(
                last_play(b),
                Some(("401892474000005810", "GOAL! Tim Stützle Goal Wrist Shot"))
            );
        }
        "shootout_teamless_attempt" => {
            let (a, b) = (live(without), live(with.unwrap()));
            assert_eq!((a.away.abbreviation, a.home.abbreviation), ("SEA", "PHI"));
            assert_eq!((a.away.score, a.home.score), (2, 2));
            assert_eq!(a.last_play, None);
            assert_eq!(b.shots, shots(31, 26));
            assert_eq!(b.goals.len(), 4);
            assert_eq!(b.last_scorer, "T. Zegras");
            // The team-less fourth attempt was saved by PHI's goalie: SEA shot.
            assert_eq!(
                b.shootout,
                Some(Shootout {
                    away: attempts(2, 0b01),
                    home: attempts(3, 0b010),
                    next_shooter: Some(Side::Away),
                })
            );
            assert_eq!(
                last_play(b),
                Some(("401892492000005240", "GOAL! Matvei Michkov Goal Wrist Shot"))
            );
        }
        "shootout_failed_attempt" => {
            let (a, b) = (live(without), live(with.unwrap()));
            assert_eq!((a.away.abbreviation, a.home.abbreviation), ("BUF", "MTL"));
            assert_eq!((a.away.score, a.home.score), (3, 3));
            // The scoreboard poll already has Bolduc's attempt as a goal.
            assert_eq!(
                last_play(a),
                Some(("401892490000008060", "GOAL! Zachary Bolduc Goal Backhand"))
            );
            assert_eq!(b.shots, shots(17, 38));
            assert_eq!(b.goals.len(), 6);
            assert_eq!(b.last_scorer, "O. Zellweger");
            // Type 2010 is a miss; the summary still has Bolduc's as a shot.
            assert_eq!(
                b.shootout,
                Some(Shootout {
                    away: attempts(2, 0b01),
                    home: attempts(3, 0b011),
                    next_shooter: Some(Side::Away),
                })
            );
            assert_eq!(
                last_play(b),
                Some((
                    "401892490000008050",
                    "GOAL! Alexandre Texier Goal Snap Shot"
                ))
            );
        }
        "onice_glitch" => {
            let (a, b) = (live(without), live(with.unwrap()));
            assert_eq!((a.period, a.clock), (3, "10:39"));
            assert_eq!((a.away.score, a.home.score), (5, 1));
            assert_eq!((a.shots, b.shots), (shots(27, 22), shots(27, 22)));
            let fight = Some((
                "401892454000004710",
                "Mark Stone Fighting against Brandon Montour",
            ));
            assert_eq!(last_play(a), fight, "a major flashes from the scoreboard");
            assert_eq!(last_play(b), fight);
            // SEA's 3-entry side is insane: the latest team-bearing
            // non-penalty play (a VGK face-off on the power play) decides.
            assert_eq!(
                b.strength,
                strength(StrengthKind::PowerPlay(Side::Away), 0, 0)
            );
            assert_eq!(b.goals.len(), 6);
            assert_eq!(b.goals.get(2).unwrap().tag, GoalTag::ShortHanded);
            assert_eq!(b.last_scorer, "S. Wright");
        }
        "penalty_placeholder" => {
            let (a, b) = (live(without), live(with.unwrap()));
            assert_eq!((a.period, a.clock), (1, "6:53"));
            assert_eq!(a.last_play, None, "the 143 placeholder never flashes");
            assert_eq!(b.shots, shots(5, 7));
            assert_eq!(b.strength, strength(StrengthKind::Even, 5, 5));
            assert_eq!(b.goals.len(), 1);
            assert_eq!(
                last_play(b),
                Some((
                    "401892498000003150",
                    "GOAL! Artturi Lehkonen Goal (4) Backhand, assists: Nathan MacKinnon (6), Josh Manson (3)"
                ))
            );
        }
        "final" => {
            let (a, b) = (final_(without), final_(with.unwrap()));
            for game in [a, b] {
                assert_eq!(game.game_id, "401892491");
                assert_eq!(
                    (game.away.abbreviation, game.home.abbreviation),
                    ("WSH", "CAR")
                );
                assert_eq!((game.away.score, game.home.score), (2, 4));
                assert_eq!(game.periods_played, 3);
                assert!(!game.playoffs);
                assert_eq!(game.shots, shots(22, 35));
                assert_eq!(game.away.line_score, &[1, 1, 0]);
                assert_eq!(game.home.line_score, &[0, 2, 2]);
                assert_eq!(
                    star_names(game),
                    Some(vec![
                        ("A. Svechnikov", Some(Side::Home)),
                        ("S. Walker", Some(Side::Home)),
                        ("P. Kochetkov", Some(Side::Home)),
                    ])
                );
                assert_eq!(game.series, None);
            }
        }
        "final_ot" => {
            let (a, b) = (final_(without), final_(with.unwrap()));
            for game in [a, b] {
                assert_eq!((game.away.score, game.home.score), (2, 3));
                assert_eq!(game.periods_played, 4);
                assert_eq!(game.shots, shots(38, 23));
                assert_eq!(game.away.line_score, &[1, 1, 0, 0]);
                assert_eq!(game.home.line_score, &[1, 1, 0, 1]);
                assert_eq!(
                    star_names(game),
                    Some(vec![
                        ("Y. Askarov", Some(Side::Home)),
                        ("M. Celebrini", Some(Side::Home)),
                        ("P. Kurashev", Some(Side::Home)),
                    ])
                );
            }
        }
        "final_so" => {
            let (a, b) = (final_(without), final_(with.unwrap()));
            for game in [a, b] {
                assert_eq!(
                    (game.away.score, game.home.score),
                    (3, 4),
                    "the +1 stays on a final"
                );
                assert_eq!(game.periods_played, 5);
                assert_eq!(game.shots, shots(22, 28));
                assert_eq!(game.away.line_score, &[0, 2, 1, 0, 0]);
                assert_eq!(game.home.line_score, &[0, 2, 1, 0, 1]);
                assert_eq!(
                    star_names(game),
                    Some(vec![
                        ("B. Marchand", Some(Side::Home)),
                        ("S. Bobrovsky", Some(Side::Home)),
                        ("S. Reinhart", Some(Side::Home)),
                    ])
                );
            }
        }
        "final_2ot" => {
            let (a, b) = (final_(without), final_(with.unwrap()));
            for game in [a, b] {
                assert!(game.playoffs);
                assert_eq!((game.away.score, game.home.score), (4, 5));
                assert_eq!(game.periods_played, 5);
                assert_eq!(game.shots, shots(33, 35));
                assert_eq!(game.away.line_score, &[0, 0, 4, 0, 0]);
                assert_eq!(game.home.line_score, &[0, 4, 0, 0, 1]);
                assert_eq!(
                    star_names(game),
                    Some(vec![
                        ("S. Theodore", Some(Side::Home)),
                        ("M. Marner", Some(Side::Home)),
                        ("B. McNabb", Some(Side::Home)),
                    ])
                );
                assert_eq!(game.series, Some("VGK leads series 2-1"));
            }
        }
        "playoff_clincher" => {
            let (a, b) = (final_(without), final_(with.unwrap()));
            for game in [a, b] {
                assert!(game.playoffs);
                assert_eq!((game.away.score, game.home.score), (3, 0));
                assert_eq!(game.periods_played, 3);
                assert_eq!(game.shots, shots(23, 22));
                assert_eq!(game.away.line_score, &[1, 1, 1]);
                assert_eq!(game.home.line_score, &[0, 0, 0]);
                assert_eq!(
                    star_names(game),
                    Some(vec![
                        ("B. Bussi", Some(Side::Away)),
                        ("T. Hall", Some(Side::Away)),
                        ("J. Blake", Some(Side::Away)),
                    ])
                );
                assert_eq!(game.series, Some("CAR wins series 4-2"));
            }
        }
        "final_so_no_stars" => {
            let (a, b) = (final_(without), final_(with.unwrap()));
            for game in [a, b] {
                assert_eq!((game.away.score, game.home.score), (2, 3));
                assert_eq!(game.periods_played, 5);
                assert_eq!(game.shots, shots(18, 20), "identity and boxscore agree");
                assert_eq!(game.away.line_score, &[2, 0, 0, 0, 0]);
                assert_eq!(game.home.line_score, &[0, 1, 1, 0, 1]);
                assert_eq!(game.stars, None, "the usual just-final shape");
                assert_eq!(game.series, None);
            }
        }
        other => panic!("no expectations for {other}"),
    }
}

#[test]
fn every_pair_extracts_and_round_trips_with_and_without_its_summary() {
    for name in FIXTURES {
        let (extract, summary) = pair(name);
        let without = extract.as_game(None);
        round_trip(&without);
        let with = summary
            .as_ref()
            .map(|summary| extract.as_game(Some(summary)));
        if let Some(with) = &with {
            round_trip(with);
        }
        check_fixture(name, &without, with.as_ref());
    }
}

#[test]
fn the_corpus_pairs_raise_only_the_quirks_provenance_predicts() {
    for name in FIXTURES {
        let event = fixture(name);
        let report = detail(&wrap(&[&event]), &id_of(&event));
        assert_eq!(report.quirks.0, vec![], "{name}: scoreboard quirks");
        let GameOutcome::Found(extract) = report.outcome else {
            panic!("{name}");
        };
        if let Some(text) = summary_text(name) {
            let report = summarize(&text, extract.summary_context());
            let expected = match *name {
                "onice_glitch" => vec![Quirk::OnIceInsane],
                _ => vec![],
            };
            assert_eq!(report.quirks.0, expected, "{name}: summary quirks");
        }
    }
}

#[test]
fn chunk_split_invariance_over_every_pair() {
    let seeded: Vec<usize> = (0..64)
        .map(|i| 1 + (17usize.wrapping_mul(31).wrapping_add(i * 7)) % 97)
        .collect();
    for name in FIXTURES {
        let event = fixture(name);
        let target = id_of(&event);
        let body = wrap(&[&event]);
        let whole = detail(&body, &target);
        let GameOutcome::Found(reference) = whole.outcome else {
            panic!("{name}");
        };
        for sizes in [&[1][..], &[7], &seeded] {
            let GameOutcome::Found(split) = extract_sized(&body, &target, sizes).outcome else {
                panic!("{name} at {sizes:?}");
            };
            assert_eq!(split, reference, "{name}: scoreboard diverged at {sizes:?}");
        }

        let Some(text) = summary_text(name) else {
            continue;
        };
        let context = reference.summary_context();
        let whole = summarize(&text, context);
        // The summaries run to 630 KB; the byte-at-a-time feed covers the
        // largest, a shootout and a puck drop, the rest the two others.
        let one_byte = matches!(
            *name,
            "final_2ot" | "shootout_teamless_attempt" | "puck_drop"
        );
        for sizes in [&[7][..], &seeded]
            .into_iter()
            .chain(one_byte.then_some(&[1][..]))
        {
            let split = summarize_sized(&text, context, sizes);
            assert_eq!(
                split.summary, whole.summary,
                "{name}: summary diverged at {sizes:?}"
            );
            assert_eq!(
                split.quirks.0, whole.quirks.0,
                "{name}: quirks diverged at {sizes:?}"
            );
        }
        let with = reference.as_game(Some(&whole.summary));
        assert_eq!(
            encode(&with),
            encode(&reference.as_game(Some(&whole.summary)))
        );
    }
}

/// Re-serialize with every object's keys in sorted or reverse-sorted order —
/// which moves `header` behind `plays`, a play's `team` behind its
/// `participants`, a group's `name` behind its `athletes`, and so on.
fn reordered(value: &serde_json::Value, reverse: bool, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            if reverse {
                keys.reverse();
            }
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap());
                out.push(':');
                reordered(&map[*key], reverse, out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                reordered(item, reverse, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&serde_json::to_string(scalar).unwrap()),
    }
}

#[test]
fn field_order_independence_over_every_pair() {
    for name in FIXTURES {
        let event = fixture(name);
        let target = id_of(&event);
        let reference = found(&wrap(&[&event]), &target);
        let summary = summary_text(name);
        let reference_summary = summary
            .as_ref()
            .map(|text| summarize(text, reference.summary_context()));
        for reverse in [false, true] {
            let mut shuffled = String::new();
            reordered(&parse(&event), reverse, &mut shuffled);
            assert_ne!(
                shuffled, event,
                "{name}: the reordering must change the text"
            );
            let extract = found(&wrap(&[&shuffled]), &target);
            assert_eq!(
                extract, reference,
                "{name}: scoreboard order-dependent (reverse={reverse})"
            );

            let Some(text) = &summary else {
                continue;
            };
            let mut shuffled = String::new();
            reordered(&parse(text), reverse, &mut shuffled);
            let report = summarize(&shuffled, extract.summary_context());
            let reference_summary = reference_summary.as_ref().unwrap();
            assert_eq!(
                report.summary, reference_summary.summary,
                "{name}: summary order-dependent (reverse={reverse})"
            );
            assert_eq!(
                report.quirks.0, reference_summary.quirks.0,
                "{name}: quirks order-dependent"
            );
        }
    }
}

// ------------------------------------------------------- §3.1 states

#[test]
fn delayed_start_at_period_zero_is_a_pregame_and_lists_as_one() {
    let event = fixture("delayed_start");
    let target = id_of(&event);
    assert!(matches!(
        found(&wrap(&[&event]), &target).kind,
        Kind::Pregame(_)
    ));

    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = ListExtractor::new(&mut scratch).unwrap();
    extractor.write(wrap(&[&event]).as_bytes()).unwrap();
    let list = extractor.finish().unwrap();
    assert_eq!(list.games[0].state, GameState::Pregame);

    // DELAYED after the puck dropped is live, in play.
    let later = mutate(&event, |v| {
        v["competitions"][0]["status"]["period"] = serde_json::json!(1);
        v["competitions"][0]["status"]["displayClock"] = serde_json::json!("18:55");
    });
    let extract = found(&wrap(&[&later]), &target);
    let game = extract.as_game(None);
    assert_eq!(
        (live(&game).period, live(&game).phase, live(&game).clock),
        (1, LivePhase::InProgress, "18:55")
    );
}

#[test]
fn unknown_status_name_follows_its_state_with_a_quirk() {
    let event = fixture("in_progress");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["name"] = serde_json::json!("STATUS_RAIN_DELAY");
    })]);
    let report = detail(&body, &id_of(&event));
    let GameOutcome::Found(extract) = report.outcome else {
        panic!("found");
    };
    assert!(matches!(extract.kind, Kind::Live(_)));
    assert_eq!(report.quirks.0, vec![Quirk::UnknownStatusName]);
    // An absent name is nothing to remark on.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]
            .as_object_mut()
            .unwrap()
            .remove("name");
    })]);
    let report = detail(&body, &id_of(&event));
    assert!(matches!(report.outcome, GameOutcome::Found(_)));
    assert_eq!(report.quirks.0, vec![]);
}

#[test]
fn period_five_is_a_second_overtime_in_the_playoffs() {
    let (extract, summary) = pair("final_2ot");
    assert!(extract.playoffs);
    let summary = summary.unwrap();
    // The winning goal is logged in period 5: a goal, not a shootout attempt.
    assert_eq!(summary.goals.len(), 9);
    let winner = summary.goals.last().unwrap();
    assert_eq!(
        (winner.period, winner.side, winner.elapsed),
        (5, Side::Home, 338)
    );
    assert_eq!(summary.shootout, None);
    assert_eq!(summary.last_scorer, "S. Theodore");

    // The same period in the regular season is the shootout: the scores tie
    // and the strength group is absent.
    let event = fixture("overtime");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["period"] = serde_json::json!(5);
        v["competitions"][0]["competitors"][0]["score"] = serde_json::json!("4");
    })]);
    let extract = found(&body, &id_of(&event));
    let game = extract.as_game(None);
    assert_eq!((live(&game).away.score, live(&game).home.score), (3, 3));
}

#[test]
fn final_alt_detail_disagreement_is_a_quirk_not_a_rejection() {
    let event = fixture("final_ot");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["altDetail"] = serde_json::json!("SO");
    })]);
    let report = detail(&body, &id_of(&event));
    let GameOutcome::Found(extract) = report.outcome else {
        panic!("found");
    };
    assert_eq!(
        final_(&extract.as_game(None)).periods_played,
        4,
        "the period is served"
    );
    assert_eq!(report.quirks.0, vec![Quirk::FinalDetailMismatch]);

    // Regulation finals carry no altDetail; one that does disagrees.
    let event = fixture("final");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["altDetail"] = serde_json::json!("OT");
    })]);
    assert_eq!(
        detail(&body, &id_of(&event)).quirks.0,
        vec![Quirk::FinalDetailMismatch]
    );

    // A playoff fourth overtime is "4OT".
    let event = fixture("final_2ot");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["period"] = serde_json::json!(7);
        v["competitions"][0]["status"]["type"]["altDetail"] = serde_json::json!("4OT");
    })]);
    assert_eq!(detail(&body, &id_of(&event)).quirks.0, vec![]);
}

#[test]
fn tied_final_is_a_quirk() {
    let event = fixture("final");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][0]["score"] = serde_json::json!("2");
    })]);
    let report = detail(&body, &id_of(&event));
    assert!(matches!(report.outcome, GameOutcome::Found(_)));
    assert_eq!(report.quirks.0, vec![Quirk::TiedFinal]);
}

// ------------------------------------------------------- §3.3 shots

#[test]
fn shots_prefer_the_boxscore_then_the_identity_then_absent() {
    let (extract, summary) = pair("power_play");
    assert_eq!(
        live(&extract.as_game(summary.as_ref())).shots,
        shots(16, 10)
    );
    assert_eq!(live(&extract.as_game(None)).shots, shots(17, 10));

    // A summary without shotsTotal yields to the identity.
    let event = fixture("power_play");
    let text = mutate(&summary_text("power_play").unwrap(), |v| {
        v["boxscore"]["teams"][0]["statistics"] = serde_json::json!([]);
    });
    let report = summarize(&text, extract.summary_context());
    assert_eq!(report.summary.shots, None, "both teams or neither");
    assert_eq!(
        live(&extract.as_game(Some(&report.summary))).shots,
        shots(17, 10)
    );

    // And one team's statistics missing leaves no shots at all.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][1]["statistics"] = serde_json::json!([]);
    })]);
    let extract = found(&body, &id_of(&event));
    assert_eq!(live(&extract.as_game(Some(&report.summary))).shots, None);

    let (extract, summary) = pair("puck_drop");
    assert_eq!(live(&extract.as_game(summary.as_ref())).shots, None);
}

#[test]
fn post_final_saves_of_zero_break_the_identity() {
    let event = fixture("final");
    let zero_saves = |v: &mut serde_json::Value| {
        for competitor in v["competitions"][0]["competitors"].as_array_mut().unwrap() {
            for stat in competitor["statistics"].as_array_mut().unwrap() {
                if stat["name"] == "saves" {
                    stat["displayValue"] = serde_json::json!("0");
                }
            }
        }
    };
    let extract = found(&wrap(&[&mutate(&event, zero_saves)]), &id_of(&event));
    assert_eq!(final_(&extract.as_game(None)).shots, None);
    let summary = summarize(&summary_text("final").unwrap(), extract.summary_context()).summary;
    assert_eq!(
        final_(&extract.as_game(Some(&summary))).shots,
        shots(22, 35),
        "the boxscore still knows"
    );

    // One side at zero is a shutout's goalie facing nothing — kept.
    let one_side = mutate(&event, |v| {
        v["competitions"][0]["competitors"][0]["statistics"][0]["displayValue"] =
            serde_json::json!("0");
    });
    let extract = found(&wrap(&[&one_side]), &id_of(&event));
    assert_eq!(final_(&extract.as_game(None)).shots, shots(2, 35));

    // Live, both at zero is the first minute, not the quirk.
    let event = fixture("in_progress");
    let extract = found(&wrap(&[&mutate(&event, zero_saves)]), &id_of(&event));
    assert_eq!(live(&extract.as_game(None)).shots, shots(3, 2));
}

// ---------------------------------------------------- §3.4 strength

#[test]
fn strength_kinds_come_from_on_ice_against_the_goalie_groups() {
    for (name, expected) in [
        ("in_progress", strength(StrengthKind::Even, 5, 5)),
        (
            "power_play",
            strength(StrengthKind::PowerPlay(Side::Away), 5, 4),
        ),
        ("four_on_four", strength(StrengthKind::Matched, 4, 4)),
        ("overtime", strength(StrengthKind::Matched, 3, 3)),
        (
            "empty_net",
            strength(StrengthKind::EmptyNet(Side::Away), 6, 5),
        ),
        ("end_of_period", strength(StrengthKind::Even, 5, 5)),
    ] {
        let (_, report) = summary_report(name);
        assert_eq!(report.summary.strength, expected, "{name}");
        assert_eq!(report.quirks.0, vec![], "{name}");
    }
}

#[test]
fn strength_falls_back_to_the_latest_team_bearing_non_penalty_play_with_a_quirk() {
    let (_, report) = summary_report("onice_glitch");
    assert_eq!(
        report.summary.strength,
        strength(StrengthKind::PowerPlay(Side::Away), 0, 0)
    );
    assert_eq!(report.quirks.0, vec![Quirk::OnIceInsane]);

    // Shorthanded on the play's team is the other side's power play.
    let (extract, _) = pair("power_play");
    let text = mutate(&summary_text("power_play").unwrap(), |v| {
        v["onIce"][0]["entries"] = serde_json::json!([]);
        // The newest play is a UTA shot on the power play; make it a BUF
        // play tagged Shorthanded instead.
        let plays = v["plays"].as_array_mut().unwrap();
        let last = plays.last_mut().unwrap();
        last["team"]["id"] = serde_json::json!("2");
        last["strength"]["id"] = serde_json::json!("703");
    });
    let report = summarize(&text, extract.summary_context());
    assert_eq!(
        report.summary.strength,
        strength(StrengthKind::PowerPlay(Side::Away), 0, 0)
    );
    assert_eq!(report.quirks.0, vec![Quirk::OnIceInsane]);

    // Anything but Power Play / Shorthanded on the fallback play is Even.
    let text = mutate(&summary_text("power_play").unwrap(), |v| {
        v["onIce"][0]["entries"] = serde_json::json!([]);
        v["plays"].as_array_mut().unwrap().last_mut().unwrap()["strength"]["id"] =
            serde_json::json!("701");
    });
    assert_eq!(
        summarize(&text, extract.summary_context()).summary.strength,
        strength(StrengthKind::Even, 0, 0)
    );
}

#[test]
fn puck_drops_empty_goalie_groups_fall_back_quietly() {
    let (_, report) = summary_report("puck_drop");
    assert_eq!(report.summary.strength, strength(StrengthKind::Even, 0, 0));
    assert_eq!(report.quirks.0, vec![]);
}

#[test]
fn on_ice_missing_at_a_period_start_falls_back_quietly() {
    let (extract, _) = pair("in_progress");
    for empty in [serde_json::json!([]), serde_json::Value::Null] {
        let text = mutate(&summary_text("in_progress").unwrap(), |v| {
            v["onIce"] = empty.clone();
        });
        let report = summarize(&text, extract.summary_context());
        assert_eq!(report.summary.strength, strength(StrengthKind::Even, 0, 0));
        assert_eq!(report.quirks.0, vec![]);
    }
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        v.as_object_mut().unwrap().remove("onIce");
    });
    assert_eq!(summarize(&text, extract.summary_context()).quirks.0, vec![]);
}

#[test]
fn team_less_plays_are_skipped_by_the_strength_fallback() {
    // Make every onIce side empty and the newest play team-less and tagged
    // Power Play: the fallback must reach past it.
    let (extract, _) = pair("in_progress");
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        v["onIce"] = serde_json::json!([]);
        let plays = v["plays"].as_array_mut().unwrap();
        plays.push(serde_json::json!({
            "id": "401892503000099990", "type": {"id": "516", "text": "Stoppage"},
            "text": "Goalie Stopped", "period": {"number": 2, "displayValue": "2nd"},
            "clock": {"displayValue": "10:04"}, "scoringPlay": false,
            "strength": {"id": "702", "text": "Power Play"}
        }));
    });
    let report = summarize(&text, extract.summary_context());
    // The play before it is BOS's blocked shot at even strength.
    assert_eq!(report.summary.strength, strength(StrengthKind::Even, 0, 0));
}

#[test]
fn a_pulled_goalie_wins_over_the_power_play_and_three_on_three_is_matched() {
    let (extract, _) = pair("empty_net");
    // PIT (away, 6 skaters, no goalie) against CHI with 4 skaters and a
    // goalie: the banner says EMPTY NET, the dots say 6 on 4.
    let text = mutate(&summary_text("empty_net").unwrap(), |v| {
        let blocks = v["onIce"].as_array_mut().unwrap();
        let chi = blocks.iter_mut().find(|b| b["teamId"] == "4").unwrap();
        chi["entries"].as_array_mut().unwrap().pop();
    });
    let report = summarize(&text, extract.summary_context());
    assert_eq!(
        report.summary.strength,
        strength(StrengthKind::EmptyNet(Side::Away), 6, 4)
    );
    assert_eq!(report.quirks.0, vec![]);
}

#[test]
fn no_strength_during_a_live_shootout() {
    for name in [
        "shootout",
        "shootout_teamless_attempt",
        "shootout_failed_attempt",
    ] {
        let (extract, report) = summary_report(name);
        assert_eq!(report.summary.strength, None, "{name}");
        assert_eq!(
            report.quirks.0,
            vec![],
            "{name}: the goalie-only units are not a quirk"
        );
        assert_eq!(live(&extract.as_game(Some(&report.summary))).strength, None);
    }
    // A final derives none either, whatever its stale onIce says.
    let (_, report) = summary_report("final");
    assert_eq!(report.summary.strength, None);
}

// ------------------------------------------------------- §3.5 goals

fn goal_play(
    id: u32,
    team: &str,
    period: u8,
    clock: &str,
    x: i32,
    y: i32,
    scorer: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": format!("4018925030000{id:05}"),
        "type": {"id": "505", "text": "Goal"},
        "text": format!("{scorer} Goal"),
        "period": {"number": period, "displayValue": "2nd"},
        "clock": {"displayValue": clock},
        "team": {"id": team},
        "scoringPlay": true,
        "participants": [{"type": "scorer", "athlete": {"id": "1", "shortName": scorer}}],
        "coordinate": {"x": x, "y": y},
        "strength": {"id": "701", "text": "Even Strength"},
        "shotInfo": {"id": "901"}
    })
}

#[test]
fn goals_exclude_the_shootout_and_keep_the_newest_sixteen() {
    let (extract, _) = pair("in_progress");
    // Five real goals plus twenty synthesized: the ring keeps the newest 16,
    // oldest first, and the last scorer is the newest's.
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        let plays = v["plays"].as_array_mut().unwrap();
        for n in 0..20u32 {
            plays.push(goal_play(
                n,
                if n % 2 == 0 { "1" } else { "18" },
                3,
                &format!("{}:00", n),
                50,
                10,
                &format!("P{n}"),
            ));
        }
    });
    let report = summarize(&text, extract.summary_context());
    let goals = &report.summary.goals;
    assert_eq!(goals.len(), 16);
    assert_eq!(
        goals[0].elapsed,
        4 * 60,
        "goal #4 of the synthesized is the oldest kept"
    );
    assert_eq!(goals[15].elapsed, 19 * 60);
    assert_eq!(report.summary.last_scorer, "P19");
    let game = extract.as_game(Some(&report.summary));
    assert_eq!(live(&game).goals.len(), 16);
    round_trip(&game);

    // Shootout goals are attempts, never goals (§3.5 excludes the shootout).
    let (_, summary) = pair("shootout");
    let summary = summary.unwrap();
    assert_eq!(summary.goals.len(), 4);
    assert!(summary.goals.iter().all(|goal| goal.period < 5));
}

#[test]
fn goal_tags_eng_ppg_shg_and_the_penalty_shot_goal() {
    let (_, summary) = pair("final");
    assert_eq!(
        summary.unwrap().goals.last().unwrap().tag,
        GoalTag::EmptyNet,
        "shotInfo 903 and strength 903"
    );
    let (_, summary) = pair("playoff_clincher");
    assert_eq!(
        summary.unwrap().goals.last().unwrap().tag,
        GoalTag::EmptyNet
    );
    let (_, summary) = pair("power_play");
    assert_eq!(summary.unwrap().goals[0].tag, GoalTag::PowerPlay);
    let (_, summary) = pair("onice_glitch");
    assert_eq!(summary.unwrap().goals[2].tag, GoalTag::ShortHanded);
    let (_, summary) = pair("overtime");
    assert_eq!(
        summary.unwrap().goals.last().unwrap().tag,
        GoalTag::Goal,
        "902 penalty shot"
    );

    // Either empty-net marker alone is enough, and ENG wins over PPG.
    let (extract, _) = pair("final");
    let text = mutate(&summary_text("final").unwrap(), |v| {
        let goal = v["plays"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .rev()
            .find(|p| p["scoringPlay"] == true)
            .unwrap();
        goal["strength"]["id"] = serde_json::json!("702");
    });
    assert_eq!(
        summarize(&text, extract.summary_context())
            .summary
            .goals
            .last()
            .unwrap()
            .tag,
        GoalTag::EmptyNet
    );
    let text = mutate(&summary_text("final").unwrap(), |v| {
        let goal = v["plays"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .rev()
            .find(|p| p["scoringPlay"] == true)
            .unwrap();
        goal["shotInfo"]["id"] = serde_json::json!("901");
    });
    assert_eq!(
        summarize(&text, extract.summary_context())
            .summary
            .goals
            .last()
            .unwrap()
            .tag,
        GoalTag::EmptyNet
    );
}

#[test]
fn coordinates_normalize_so_away_attacks_plus_x() {
    let (extract, summary) = pair("in_progress");
    let summary = summary.unwrap();
    // BOS (away) scored from (-44, 14): rotated to (44, -14).
    assert_eq!(
        (summary.goals[0].side, summary.goals[0].coordinate),
        (Side::Away, Some(Coordinate { x: 44, y: -14 }))
    );
    // SJ (home) scored from (27, -4): rotated to (-27, 4).
    assert_eq!(
        (summary.goals[1].side, summary.goals[1].coordinate),
        (Side::Home, Some(Coordinate { x: -27, y: 4 }))
    );
    // BOS from (83, 2): already attacking +x, untouched.
    assert_eq!(
        summary.goals[4].coordinate,
        Some(Coordinate { x: 83, y: 2 })
    );

    // x = 0 has no end to infer from: sign(0) is +1, y keeps its sign; and a
    // coordinate past the boards is pulled back to them.
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        let plays = v["plays"].as_array_mut().unwrap();
        plays.push(goal_play(1, "1", 2, "9:00", 0, -7, "A"));
        plays.push(goal_play(2, "18", 2, "9:01", 0, -7, "B"));
        plays.push(goal_play(3, "1", 2, "9:02", -150, 60, "C"));
    });
    let goals = summarize(&text, extract.summary_context()).summary.goals;
    let tail = &goals[goals.len() - 3..];
    assert_eq!(tail[0].coordinate, Some(Coordinate { x: 0, y: -7 }));
    assert_eq!(tail[1].coordinate, Some(Coordinate { x: 0, y: 7 }));
    assert_eq!(tail[2].coordinate, Some(Coordinate { x: 100, y: -43 }));
}

fn newest_goal(v: &mut serde_json::Value) -> &mut serde_json::Value {
    v["plays"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .rev()
        .find(|p| p["scoringPlay"] == true)
        .unwrap()
}

#[test]
fn scorer_role_falls_back_to_the_first_participant() {
    let (extract, _) = pair("in_progress");
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        for participant in newest_goal(v)["participants"].as_array_mut().unwrap() {
            participant.as_object_mut().unwrap().remove("type");
        }
    });
    assert_eq!(
        summarize(&text, extract.summary_context())
            .summary
            .last_scorer,
        "M. Poitras"
    );
    // The scorer role wins wherever it is listed.
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        let participants = newest_goal(v)["participants"].as_array_mut().unwrap();
        participants.reverse();
    });
    assert_eq!(
        summarize(&text, extract.summary_context())
            .summary
            .last_scorer,
        "M. Poitras"
    );
    // No participants at all: an empty scorer, the goal still counts.
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        newest_goal(v)["participants"] = serde_json::json!([]);
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(summary.last_scorer, "");
    assert_eq!(summary.goals.len(), 5);
    round_trip(&extract.as_game(Some(&summary)));
}

#[test]
fn a_goal_without_a_coordinate_keeps_its_slot() {
    let (extract, _) = pair("in_progress");
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        let goal = v["plays"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .rev()
            .find(|p| p["scoringPlay"] == true)
            .unwrap();
        goal.as_object_mut().unwrap().remove("coordinate");
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(summary.goals.len(), 5);
    assert_eq!(summary.goals.last().unwrap().coordinate, None);
    assert_eq!(summary.last_scorer, "M. Poitras");
    let game = extract.as_game(Some(&summary));
    assert_eq!(live(&game).goals.last().unwrap().coordinate, None);
    round_trip(&game);
}

#[test]
fn a_goal_whose_team_matches_neither_side_is_dropped_with_a_quirk() {
    let (extract, _) = pair("in_progress");
    let text = mutate(&summary_text("in_progress").unwrap(), |v| {
        let goal = v["plays"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .rev()
            .find(|p| p["scoringPlay"] == true)
            .unwrap();
        goal["team"]["id"] = serde_json::json!("999");
    });
    let report = summarize(&text, extract.summary_context());
    assert_eq!(report.summary.goals.len(), 4);
    assert_eq!(
        report.summary.last_scorer, "",
        "the dropped goal was the newest"
    );
    assert_eq!(report.quirks.0, vec![Quirk::UnplaceableGoal]);
}

// ------------------------------------------------------- §3.6 flash

#[test]
fn the_placeholder_penalty_never_flashes() {
    let (extract, summary) = pair("penalty_placeholder");
    assert_eq!(
        live(&extract.as_game(None)).last_play,
        None,
        "the scoreboard's lastPlay is the 143"
    );
    let with = extract.as_game(summary.as_ref());
    assert_eq!(
        last_play(live(&with)).map(|(id, _)| id),
        Some("401892498000003150"),
        "the goal before it"
    );

    // The rewrite under the same id flashes, once it is the real infraction.
    let (extract, summary) = pair("four_on_four");
    let with = extract.as_game(summary.as_ref());
    assert_eq!(
        last_play(live(&with)).map(|(id, _)| id),
        Some("401892498000003420")
    );
}

#[test]
fn penalty_shot_calls_without_minutes_do_not_flash() {
    let (extract, _) = pair("four_on_four");
    let text = mutate(&summary_text("four_on_four").unwrap(), |v| {
        let play = v["plays"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["id"] == "401892498000003420")
            .unwrap();
        play["type"] =
            serde_json::json!({"id": "137", "text": "Slashing on Breakaway", "penaltyType": "PS"});
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(
        summary.last_play.as_ref().map(|p| p.id.as_str()),
        Some("401892498000003380"),
        "falls back to the coincidental minor"
    );

    // The 2025-26 type 509 penalties carried no minutes either: never eligible.
    let (extract, _) = pair("final_ot");
    let text = mutate(&summary_text("final_ot").unwrap(), |v| {
        // Drop every goal so only penalties could flash.
        v["plays"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["scoringPlay"] != true);
    });
    assert_eq!(
        summarize(&text, extract.summary_context())
            .summary
            .last_play,
        None
    );
}

#[test]
fn the_flash_falls_back_to_the_scoreboard_without_a_summary() {
    let (extract, summary) = pair("onice_glitch");
    let fight = Some((
        "401892454000004710",
        "Mark Stone Fighting against Brandon Montour",
    ));
    assert_eq!(last_play(live(&extract.as_game(None))), fight);
    assert_eq!(last_play(live(&extract.as_game(summary.as_ref()))), fight);

    // With a summary the summary decides, even when it has nothing yet.
    let (extract, _) = pair("shootout_failed_attempt");
    assert!(live(&extract.as_game(None)).last_play.is_some());
    let text = mutate(&summary_text("shootout_failed_attempt").unwrap(), |v| {
        v["plays"] = serde_json::json!([]);
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(live(&extract.as_game(Some(&summary))).last_play, None);

    // A scoreboard lastPlay with no id, or a scalar one, flashes nothing.
    let event = fixture("onice_glitch");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["situation"]["lastPlay"]
            .as_object_mut()
            .unwrap()
            .remove("id");
    })]);
    assert_eq!(
        live(&found(&body, &id_of(&event)).as_game(None)).last_play,
        None
    );
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["situation"]["lastPlay"] = serde_json::Value::Null;
    })]);
    assert_eq!(
        live(&found(&body, &id_of(&event)).as_game(None)).last_play,
        None
    );
}

// --------------------------------------------------- §3.8 shootout

fn so_attempt(
    id: u32,
    type_id: &str,
    team: Option<&str>,
    participants: &[(Option<&str>, &str)],
) -> serde_json::Value {
    let mut play = serde_json::json!({
        "id": format!("4018924920000{id:05}"),
        "type": {"id": type_id, "text": "Shot"},
        "text": "Shot",
        "period": {"number": 5, "displayValue": "SO"},
        "clock": {"displayValue": "0:00"},
        "scoringPlay": type_id == "505",
        "participants": participants.iter().map(|(role, athlete)| {
            let mut p = serde_json::json!({"athlete": {"id": athlete, "shortName": "X"}});
            if let Some(role) = role {
                p["type"] = serde_json::json!(role);
            }
            p
        }).collect::<Vec<_>>(),
        "strength": {"id": "701"}
    });
    if let Some(team) = team {
        play["team"] = serde_json::json!({"id": team});
    }
    play
}

/// `shootout_teamless_attempt` with its shootout plays replaced.
fn teamless_with_attempts(
    extract: &Extract,
    attempts: Vec<serde_json::Value>,
) -> SummaryReport<RecQuirks> {
    let text = mutate(&summary_text("shootout_teamless_attempt").unwrap(), |v| {
        let plays = v["plays"].as_array_mut().unwrap();
        plays.retain(|p| p["period"]["number"] != 5);
        plays.extend(attempts);
    });
    summarize(&text, extract.summary_context())
}

#[test]
fn shootout_attempt_types_and_the_failed_attempt() {
    let (extract, _) = pair("shootout_teamless_attempt");
    // SEA = 124292 (away), PHI = 15 (home). Goal, Shot, Missed and the 2010
    // failed attempt all count; only 505 scores. A stoppage does not count.
    let report = teamless_with_attempts(
        &extract,
        vec![
            so_attempt(1, "505", Some("15"), &[]),
            so_attempt(2, "506", Some("124292"), &[]),
            so_attempt(3, "507", Some("15"), &[]),
            so_attempt(4, "2010", Some("124292"), &[]),
            so_attempt(5, "516", Some("15"), &[]),
            so_attempt(6, "505", Some("15"), &[]),
        ],
    );
    assert_eq!(
        report.summary.shootout,
        Some(Shootout {
            away: attempts(2, 0b00),
            home: attempts(3, 0b101),
            next_shooter: Some(Side::Away),
        })
    );
    assert_eq!(report.quirks.0, vec![]);
}

#[test]
fn shootout_sides_fall_back_to_the_goalie_then_alternation() {
    let (extract, summary) = pair("shootout_teamless_attempt");
    let summary = summary.unwrap();
    assert_eq!(
        summary.shootout.unwrap().away,
        attempts(2, 0b01),
        "placed through PHI's goalie"
    );

    // No goalie among the participants: the side opposite the previous
    // attempt's (PHI's) — SEA again.
    let text = mutate(&summary_text("shootout_teamless_attempt").unwrap(), |v| {
        let play = v["plays"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["id"] == "401892492000005250")
            .unwrap();
        play["participants"] = serde_json::json!([]);
    });
    let report = summarize(&text, extract.summary_context());
    assert_eq!(report.summary.shootout, summary.shootout);
    assert_eq!(report.quirks.0, vec![]);

    // The first attempt team-less: the side opposite the next attempt's
    // (SEA's), so home scored first; after SEA's attempt home shoots again.
    let report = teamless_with_attempts(
        &extract,
        vec![
            so_attempt(1, "505", None, &[]),
            so_attempt(2, "506", Some("124292"), &[]),
        ],
    );
    assert_eq!(
        report.summary.shootout,
        Some(Shootout {
            away: attempts(1, 0b0),
            home: attempts(1, 0b1),
            next_shooter: Some(Side::Home),
        })
    );
    assert_eq!(report.quirks.0, vec![]);

    // The goalie fallback reads any participant, typed or not, and the
    // goalie's own team defended.
    let report = teamless_with_attempts(
        &extract,
        vec![
            so_attempt(1, "506", Some("15"), &[]),
            so_attempt(2, "505", None, &[(None, "5657")]), // Grubauer, SEA: PHI shot
        ],
    );
    assert_eq!(report.summary.shootout.unwrap().home, attempts(2, 0b10));
}

#[test]
fn an_unplaceable_attempt_is_dropped_with_a_quirk() {
    let (extract, _) = pair("shootout_teamless_attempt");
    let report = teamless_with_attempts(&extract, vec![so_attempt(1, "505", None, &[])]);
    assert_eq!(
        report.summary.shootout,
        Some(Shootout {
            away: attempts(0, 0),
            home: attempts(0, 0),
            next_shooter: None,
        })
    );
    assert_eq!(report.quirks.0, vec![Quirk::UnplaceableAttempt]);

    // Two unplaceable in a row drop both; a later placed one still counts and
    // sets the next shooter; the quirk fires once.
    let report = teamless_with_attempts(
        &extract,
        vec![
            so_attempt(1, "505", None, &[]),
            so_attempt(2, "506", None, &[]),
            so_attempt(3, "505", Some("15"), &[]),
        ],
    );
    assert_eq!(
        report.summary.shootout,
        Some(Shootout {
            away: attempts(0, 0),
            home: attempts(1, 0b1),
            next_shooter: Some(Side::Away),
        })
    );
    assert_eq!(report.quirks.0, vec![Quirk::UnplaceableAttempt]);
}

#[test]
fn shootout_keeps_the_first_sixteen_per_side() {
    let (extract, _) = pair("shootout_teamless_attempt");
    // Eighteen rounds, home first; home scores on odd attempts, away on the
    // 1st and 17th: the 17th is past the mask and frozen out.
    let mut plays = Vec::new();
    for round in 0..18u32 {
        plays.push(so_attempt(
            round * 2 + 1,
            if round % 2 == 0 { "505" } else { "506" },
            Some("15"),
            &[],
        ));
        plays.push(so_attempt(
            round * 2 + 2,
            if round == 0 || round == 16 {
                "505"
            } else {
                "507"
            },
            Some("124292"),
            &[],
        ));
    }
    let report = teamless_with_attempts(&extract, plays);
    assert_eq!(
        report.summary.shootout,
        Some(Shootout {
            away: attempts(16, 0b0000_0000_0000_0001),
            home: attempts(16, 0b0101_0101_0101_0101),
            next_shooter: Some(Side::Home),
        })
    );
    assert_eq!(
        report.quirks.0,
        vec![],
        "36 attempts fit the sequence bound"
    );
    round_trip(&extract.as_game(Some(&report.summary)));

    // Past the sequence bound the excess is dropped with one quirk.
    let mut plays = Vec::new();
    for n in 0..50u32 {
        plays.push(so_attempt(
            n,
            "506",
            Some(if n % 2 == 0 { "15" } else { "124292" }),
            &[],
        ));
    }
    let report = teamless_with_attempts(&extract, plays);
    assert_eq!(
        report.summary.shootout.unwrap().next_shooter,
        Some(Side::Home),
        "the 40th attempt was away's"
    );
    assert_eq!(report.quirks.0, vec![Quirk::BoundedOverflow]);
}

#[test]
fn next_shooter_alternates_and_ends_with_shootout_end_while_the_score_stays_tied() {
    let (extract, summary) = pair("shootout");
    let summary = summary.unwrap();
    assert_eq!(summary.shootout.unwrap().next_shooter, Some(Side::Home));

    // ESPN's live score already carries the +1: both sides read the tie.
    let event = fixture("shootout");
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][0]["score"] = serde_json::json!("3");
    })]);
    let extract_plus_one = found(&body, &id_of(&event));
    let game = extract_plus_one.as_game(Some(&summary));
    assert_eq!((live(&game).away.score, live(&game).home.score), (2, 2));
    round_trip(&game);

    // A Shootout End play: nobody shoots next, the tally stays.
    let text = mutate(&summary_text("shootout").unwrap(), |v| {
        v["plays"].as_array_mut().unwrap().push(serde_json::json!({
            "id": "401892474000005850", "type": {"id": "521", "text": "Shootout End"},
            "text": "End of Shootout", "period": {"number": 5, "displayValue": "SO"},
            "clock": {"displayValue": "0:00"}, "scoringPlay": false
        }));
    });
    let ended = summarize(&text, extract.summary_context())
        .summary
        .shootout
        .unwrap();
    assert_eq!(ended.next_shooter, None);
    assert_eq!(
        (ended.away, ended.home),
        (attempts(2, 0b01), attempts(2, 0b10))
    );

    // The scoreboard says In SO before the summary has a shootout play: the
    // block is present and empty, so the clock slot can show 0-0.
    let text = mutate(&summary_text("shootout").unwrap(), |v| {
        v["plays"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["period"]["number"] != 5);
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(summary.shootout, None);
    let game = extract.as_game(Some(&summary));
    assert_eq!(
        live(&game).shootout,
        Some(Shootout {
            away: attempts(0, 0),
            home: attempts(0, 0),
            next_shooter: None,
        })
    );
    round_trip(&game);

    // The intermission after the shootout keeps the block and the tie.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["type"]["name"] = serde_json::json!("STATUS_END_PERIOD");
        v["competitions"][0]["competitors"][0]["score"] = serde_json::json!("3");
    })]);
    let extract = found(&body, &id_of(&event));
    let (_, summary) = pair("shootout");
    let game = extract.as_game(summary.as_ref());
    assert_eq!(live(&game).phase, LivePhase::EndOfPeriod);
    assert_eq!((live(&game).away.score, live(&game).home.score), (2, 2));
    assert!(live(&game).shootout.is_some());
}

// ------------------------------------------------- §3.9 / §3.10

#[test]
fn stars_come_from_the_scoreboard_all_three_or_none() {
    let event = fixture("final");
    let target = id_of(&event);
    // The summary's featuredAthletes are never read: stripping them changes
    // nothing, stripping the scoreboard's third star drops all three.
    let (extract, summary) = pair("final");
    assert!(final_(&extract.as_game(summary.as_ref())).stars.is_some());
    let text = mutate(&summary_text("final").unwrap(), |v| {
        v["header"]["competitions"][0]["status"]
            .as_object_mut()
            .unwrap()
            .remove("featuredAthletes");
    });
    let summary = summarize(&text, extract.summary_context()).summary;
    assert_eq!(
        star_names(final_(&extract.as_game(Some(&summary)))).map(|s| s.len()),
        Some(3)
    );

    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["featuredAthletes"]
            .as_array_mut()
            .unwrap()
            .retain(|f| f["name"] != "thirdStar");
    })]);
    assert_eq!(final_(&found(&body, &target).as_game(None)).stars, None);

    // A star whose team is neither side has no side; the winning goalie is
    // not a star.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["status"]["featuredAthletes"][2]["team"]["id"] =
            serde_json::json!("999");
    })]);
    let extract = found(&body, &target);
    let game = extract.as_game(None);
    assert_eq!(
        star_names(final_(&game)).unwrap()[0],
        ("A. Svechnikov", None)
    );
    round_trip(&game);
}

#[test]
fn pregame_records_are_w_l_otl_and_goalies_come_from_probables() {
    let event = fixture("pregame");
    let target = id_of(&event);
    // `ytd` is accepted like `total`; the home/road entries never are.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][0]["records"][0]["type"] = serde_json::json!("ytd");
    })]);
    let extract = found(&body, &target);
    let game = extract.as_game(None);
    assert_eq!(
        pregame(&game).home.record,
        Some(wire::Record {
            wins: 4,
            losses: 2,
            ot_losses: 0
        })
    );
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][0]["records"][0]["type"] = serde_json::json!("home");
    })]);
    assert_eq!(
        pregame(&found(&body, &target).as_game(None)).home.record,
        None
    );

    // W-L is not a hockey record: a quirk and no record.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][0]["records"][0]["summary"] = serde_json::json!("4-2");
    })]);
    let report = detail(&body, &target);
    let GameOutcome::Found(extract) = report.outcome else {
        panic!("found");
    };
    assert_eq!(pregame(&extract.as_game(None)).home.record, None);
    assert_eq!(report.quirks.0, vec![Quirk::MalformedRecord]);

    // No probables: no goalie line, the event stands.
    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][1]
            .as_object_mut()
            .unwrap()
            .remove("probables");
    })]);
    let extract = found(&body, &target);
    let game = extract.as_game(None);
    assert_eq!(
        (pregame(&game).away.goalie, pregame(&game).home.goalie),
        (None, Some("S. Wedgewood"))
    );
    round_trip(&game);
}

#[test]
fn playoff_pregame_info_secondary_joins_headline_and_series() {
    let event = fixture("pregame");
    let target = id_of(&event);
    let playoff = |v: &mut serde_json::Value| {
        v["season"]["type"] = serde_json::json!(3);
        v["competitions"][0]["notes"] =
            serde_json::json!([{"type": "event", "headline": "Stanley Cup Final - Game 6"}]);
        v["competitions"][0]["series"] = serde_json::json!({"summary": "CAR leads series 3-2"});
    };
    let extract = found(&wrap(&[&mutate(&event, playoff)]), &target);
    let game = extract.as_game(None);
    assert!(pregame(&game).playoffs);
    assert_eq!(
        pregame(&game).info_secondary,
        "Stanley Cup Final - Game 6 - CAR leads series 3-2"
    );
    round_trip(&game);

    // Only a headline, only a series: whichever half there is.
    let body = wrap(&[&mutate(&event, |v| {
        playoff(v);
        v["competitions"][0]
            .as_object_mut()
            .unwrap()
            .remove("series");
    })]);
    assert_eq!(
        pregame(&found(&body, &target).as_game(None)).info_secondary,
        "Stanley Cup Final - Game 6"
    );
    let body = wrap(&[&mutate(&event, |v| {
        playoff(v);
        v["competitions"][0]["notes"] = serde_json::json!([]);
    })]);
    assert_eq!(
        pregame(&found(&body, &target).as_game(None)).info_secondary,
        "CAR leads series 3-2"
    );

    // The regular season shows nothing, series or not.
    let body = wrap(&[&mutate(&event, |v| {
        playoff(v);
        v["season"]["type"] = serde_json::json!(2);
    })]);
    let extract = found(&body, &target);
    let game = extract.as_game(None);
    assert!(!pregame(&game).playoffs);
    assert_eq!(pregame(&game).info_secondary, "");
}

#[test]
fn alternate_color_falls_back_to_the_primary_when_absent_and_rejects_when_malformed() {
    let event = fixture("in_progress"); // no alternateColor on either team
    let target = id_of(&event);
    let extract = found(&wrap(&[&event]), &target);
    let game = extract.as_game(None);
    assert_eq!(
        (
            live(&game).away.colors.primary,
            live(&game).away.colors.alternate
        ),
        (0x231f20, 0x231f20)
    );

    let body = wrap(&[&mutate(&event, |v| {
        v["competitions"][0]["competitors"][0]["team"]["alternateColor"] =
            serde_json::json!("ffffff");
    })]);
    let extract = found(&body, &target);
    let game = extract.as_game(None);
    assert_eq!(live(&game).home.colors.alternate, 0xffffff);

    for junk in [serde_json::json!("xyz"), serde_json::json!(12)] {
        let body = wrap(&[&mutate(&event, |v| {
            v["competitions"][0]["competitors"][0]["team"]["alternateColor"] = junk.clone();
        })]);
        let mut scratch = vec![0u8; 64 * 1024];
        let mut extractor = GameExtractor::new(&target, IgnoreQuirks, &mut scratch).unwrap();
        extractor.write(body.as_bytes()).unwrap();
        assert!(
            matches!(
                extractor.finish(),
                Err(ExtractError::Transform(TransformError::Color))
            ),
            "{junk}"
        );
    }
}

#[test]
fn final_line_scores_stay_raw_per_period() {
    // Every overtime its own entry, the shootout a fifth of 1/0; the model
    // collapses them (§3.10), the extractor does not.
    let (extract, _) = pair("final_2ot");
    let game = extract.as_game(None);
    assert_eq!(final_(&game).away.line_score.len(), 5);
    assert_eq!(final_(&game).home.line_score, &[0, 4, 0, 0, 1]);
    let (extract, _) = pair("final_so");
    let game = extract.as_game(None);
    assert_eq!(final_(&game).home.line_score, &[0, 2, 1, 0, 1]);
    assert_eq!(final_(&game).away.line_score, &[0, 2, 1, 0, 0]);
}

// ------------------------------------------------ list + finding

#[test]
fn games_list_states_ids_and_exact_failed_counts() {
    let pregame = fixture("pregame");
    let delayed = fixture("delayed_start");
    let shootout = fixture("shootout");
    let final_ = fixture("final");
    let no_comp = r#"{"id":"555","date":"2026-10-14T00:00Z","competitions":[]}"#;
    let body = wrap(&[&pregame, "{}", &delayed, &shootout, no_comp, &final_]);

    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = ListExtractor::new(&mut scratch).unwrap();
    extractor.write(body.as_bytes()).unwrap();
    let list = extractor.finish().unwrap();

    assert_eq!((list.ok, list.failed), (5, 1));
    assert!(!list.overflowed);
    let entries: Vec<(GameState, &str)> = list
        .games
        .iter()
        .map(|e| (e.state, e.id.as_str()))
        .collect();
    assert_eq!(
        entries,
        vec![
            (GameState::Pregame, "401892505"),
            (GameState::Pregame, "401891782"),
            (GameState::Live, "401892474"),
            (GameState::Final, "401892491"),
        ]
    );
}

#[test]
fn events_after_found_target_are_skipped() {
    let event = fixture("in_progress");
    let id = id_of(&event);
    let garbage = r#"{"id":42,"date":false,"competitions":"nope"}"#;
    let reference = encode(&found(&wrap(&[&event]), &id).as_game(None));

    let report = detail(&wrap(&[&event, garbage]), &id);
    let GameOutcome::Found(extract) = report.outcome else {
        panic!("{:?}", report.outcome);
    };
    assert_eq!(encode(&extract.as_game(None)), reference);
    assert_eq!((report.ok, report.failed), (1, 0), "post-target skipped");

    let report = detail(&wrap(&[garbage, &event]), &id);
    let GameOutcome::Found(extract) = report.outcome else {
        panic!("{:?}", report.outcome);
    };
    assert_eq!(encode(&extract.as_game(None)), reference);
    assert_eq!((report.ok, report.failed), (1, 1), "pre-target validated");

    let report = detail(&wrap(&[garbage, &event]), "999");
    assert_eq!(report.outcome, GameOutcome::Absent);
    assert_eq!((report.ok, report.failed), (1, 1));
}

#[test]
fn absent_target_distinguishes_clean_from_glitched_scoreboards() {
    let pregame = fixture("pregame");
    let report = detail(&wrap(&[&pregame]), "999");
    assert_eq!(report.outcome, GameOutcome::Absent);
    assert_eq!((report.ok, report.failed), (1, 0));
    let report = detail(&wrap(&[&pregame, "{}"]), "999");
    assert_eq!(report.outcome, GameOutcome::Absent);
    assert_eq!((report.ok, report.failed), (1, 1));
}

#[test]
fn events_shell_must_be_an_array() {
    for body in [
        r#"{"events":42}"#,
        r#"{"events":null}"#,
        r#"{"events":"x"}"#,
        r#"{"events":{"x":1}}"#,
        r#"{"events":{}}"#,
    ] {
        let mut scratch = vec![0u8; 4096];
        let mut extractor = GameExtractor::new("77", IgnoreQuirks, &mut scratch).unwrap();
        extractor.write(body.as_bytes()).unwrap();
        assert!(
            matches!(extractor.finish(), Err(ExtractError::MalformedBody)),
            "{body}"
        );
        let mut extractor = ListExtractor::new(&mut scratch).unwrap();
        extractor.write(body.as_bytes()).unwrap();
        assert!(
            matches!(extractor.finish(), Err(ExtractError::MalformedBody)),
            "{body}"
        );
    }
    assert_eq!(
        detail(r#"{"events":[]}"#, "77").outcome,
        GameOutcome::Absent
    );
}

#[test]
fn target_with_no_competition_is_not_found() {
    let no_comp = r#"{"id":"555","date":"2026-10-14T00:00Z","competitions":[]}"#;
    let report = detail(&wrap(&[no_comp]), "555");
    assert_eq!(report.outcome, GameOutcome::NoCompetition);
    assert_eq!((report.ok, report.failed), (1, 0));
    let report = detail(&wrap(&["{}", no_comp]), "555");
    assert_eq!(report.outcome, GameOutcome::NoCompetition);
    assert_eq!((report.ok, report.failed), (1, 1));
}

#[test]
fn required_fields_drop_the_event_and_optional_ones_degrade() {
    let live_event = fixture("in_progress");
    let live_id = id_of(&live_event);
    let dropped = |body: &str, target: &str| {
        let report = detail(body, target);
        assert_eq!(report.outcome, GameOutcome::Absent, "{body:.120}");
        assert_eq!((report.ok, report.failed), (0, 1));
    };
    dropped(
        &wrap(&[&mutate(&live_event, |v| {
            v["competitions"][0]["status"]
                .as_object_mut()
                .unwrap()
                .remove("displayClock");
        })]),
        &live_id,
    );
    dropped(
        &wrap(&[&mutate(&live_event, |v| {
            v["competitions"][0]["status"]["period"] = serde_json::json!("2");
        })]),
        &live_id,
    );
    dropped(
        &wrap(&[&mutate(&live_event, |v| {
            v["competitions"][0]["competitors"][0]["team"]
                .as_object_mut()
                .unwrap()
                .remove("color");
        })]),
        &live_id,
    );
    dropped(
        &wrap(&[&mutate(&live_event, |v| {
            v["competitions"][0]["competitors"][1]
                .as_object_mut()
                .unwrap()
                .remove("score");
        })]),
        &live_id,
    );
    dropped(
        &wrap(&[&mutate(&live_event, |v| {
            v["competitions"][0]["status"]["type"]["state"] = serde_json::json!("live");
        })]),
        &live_id,
    );
    dropped(
        &wrap(&[&mutate(&live_event, |v| {
            v["competitions"][0]["competitors"]
                .as_array_mut()
                .unwrap()
                .pop();
        })]),
        &live_id,
    );

    // A pregame needs neither clock nor period nor scores; nobody needs a
    // venue, a season or statistics.
    let pregame_event = fixture("pregame");
    let pregame_id = id_of(&pregame_event);
    let body = wrap(&[&mutate(&pregame_event, |v| {
        let status = v["competitions"][0]["status"].as_object_mut().unwrap();
        status.remove("displayClock");
        status.remove("period");
        for competitor in v["competitions"][0]["competitors"].as_array_mut().unwrap() {
            competitor.as_object_mut().unwrap().remove("score");
            competitor.as_object_mut().unwrap().remove("statistics");
        }
        v["competitions"][0]
            .as_object_mut()
            .unwrap()
            .remove("venue");
        v.as_object_mut().unwrap().remove("season");
    })]);
    let extract = found(&body, &pregame_id);
    let game = extract.as_game(None);
    assert_eq!(pregame(&game).venue, "");
    assert!(!pregame(&game).playoffs);
    let body = wrap(&[&mutate(&live_event, |v| {
        v["competitions"][0]["competitors"][0]
            .as_object_mut()
            .unwrap()
            .remove("statistics");
        v["competitions"][0]
            .as_object_mut()
            .unwrap()
            .remove("situation");
    })]);
    let extract = found(&body, &live_id);
    let game = extract.as_game(None);
    assert_eq!((live(&game).shots, live(&game).last_play), (None, None));
}

#[test]
fn transform_failures_reject_the_target() {
    let event = fixture("in_progress");
    let target = id_of(&event);
    let rejected = |body: String| {
        let mut scratch = vec![0u8; 64 * 1024];
        let mut extractor =
            GameExtractor::new(&target, RecQuirks::default(), &mut scratch).unwrap();
        extractor.write(body.as_bytes()).unwrap();
        match extractor.finish() {
            Err(ExtractError::Transform(error)) => error,
            other => panic!("expected a rejection, got {other:?}"),
        }
    };
    assert_eq!(
        rejected(wrap(&[&mutate(&event, |v| {
            v["competitions"][0]["competitors"][0]["team"]["color"] = serde_json::json!("12345");
        })])),
        TransformError::Color
    );
    assert_eq!(
        rejected(wrap(&[&mutate(&event, |v| {
            v["competitions"][0]["competitors"][0]["score"] = serde_json::json!("two");
        })])),
        TransformError::Score
    );
    assert_eq!(
        rejected(wrap(&[&mutate(&event, |v| {
            v["competitions"][0]["competitors"][1]["homeAway"] = serde_json::json!("home");
        })])),
        TransformError::Sides
    );
    let pregame_event = fixture("pregame");
    let pregame_id = id_of(&pregame_event);
    let body = wrap(&[&mutate(&pregame_event, |v| {
        v["date"] = serde_json::json!("tonight");
    })]);
    let mut scratch = vec![0u8; 64 * 1024];
    let mut extractor = GameExtractor::new(&pregame_id, IgnoreQuirks, &mut scratch).unwrap();
    extractor.write(body.as_bytes()).unwrap();
    assert!(matches!(
        extractor.finish(),
        Err(ExtractError::Transform(TransformError::Date))
    ));
    // The same bad date on a live game is nothing: only the pregame reads it.
    let body = wrap(&[&mutate(&event, |v| {
        v["date"] = serde_json::json!("tonight");
    })]);
    assert!(matches!(
        detail(&body, &target).outcome,
        GameOutcome::Found(_)
    ));
}

// ------------------------------------------------------- bounds

#[test]
fn summary_bounds_degrade_without_panicking() {
    let (extract, _) = pair("in_progress");
    let context = extract.summary_context();
    let base = summary_text("in_progress").unwrap();

    // A fifth goalie overflows the group with a quirk; the first four decide.
    let text = mutate(&base, |v| {
        for team in v["boxscore"]["players"].as_array_mut().unwrap() {
            let group = team["statistics"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|g| g["name"] == "goalies")
                .unwrap();
            let extra: Vec<serde_json::Value> = (0..4)
                .map(|n| serde_json::json!({"athlete": {"id": format!("9{n}")}}))
                .collect();
            let athletes = group["athletes"].as_array_mut().unwrap();
            let real = athletes.remove(0);
            athletes.extend(extra);
            athletes.push(real);
        }
    });
    let report = summarize(&text, context);
    assert_eq!(
        report.quirks.0,
        vec![Quirk::BoundedOverflow, Quirk::BoundedOverflow]
    );
    // The bound's failure mode, and why it quirks: the real goalies fell
    // past it, so neither unit finds its goalie and §3.4 reads two pulled
    // but sane sides. Nothing panicked, nothing was lost silently.
    assert_eq!(
        report.summary.strength,
        strength(StrengthKind::EmptyNet(Side::Away), 6, 6)
    );

    // A third onIce block, boxscore team, player group and header
    // competitor each quirk once and are ignored.
    let text = mutate(&base, |v| {
        let extra = v["onIce"][0].clone();
        v["onIce"].as_array_mut().unwrap().push(extra);
        let extra = v["boxscore"]["teams"][0].clone();
        v["boxscore"]["teams"].as_array_mut().unwrap().push(extra);
        let extra = v["boxscore"]["players"][0].clone();
        v["boxscore"]["players"].as_array_mut().unwrap().push(extra);
        let extra = v["header"]["competitions"][0]["competitors"][0].clone();
        v["header"]["competitions"][0]["competitors"]
            .as_array_mut()
            .unwrap()
            .push(extra);
    });
    let report = summarize(&text, context);
    assert_eq!(
        report
            .quirks
            .0
            .iter()
            .filter(|q| **q == Quirk::BoundedOverflow)
            .count(),
        4
    );
    assert_eq!(report.summary.strength, strength(StrengthKind::Even, 5, 5));
    assert_eq!(report.summary.shots, shots(10, 19));

    // A unit of nine on one side is insane however the goalie check goes.
    let text = mutate(&base, |v| {
        let entries = v["onIce"][0]["entries"].as_array_mut().unwrap();
        for n in 0..3 {
            entries.push(serde_json::json!({"athleteid": format!("8{n}")}));
        }
    });
    let report = summarize(&text, context);
    assert_eq!(report.summary.strength, strength(StrengthKind::Even, 0, 0));
    assert_eq!(report.quirks.0, vec![Quirk::OnIceInsane]);

    // Five participants on a play are read; the two past the bound only
    // matter for a team-less shootout attempt.
    let text = mutate(&base, |v| {
        let goal = v["plays"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .rev()
            .find(|p| p["scoringPlay"] == true)
            .unwrap();
        let participants = goal["participants"].as_array_mut().unwrap();
        for n in 0..3 {
            participants.push(
                serde_json::json!({"athlete": {"id": format!("7{n}"), "shortName": "Extra"}}),
            );
        }
    });
    let report = summarize(&text, context);
    assert_eq!(report.summary.last_scorer, "M. Poitras");
    assert_eq!(report.quirks.0, vec![]);
}

#[test]
fn a_malformed_summary_region_degrades_only_what_depended_on_it() {
    let (extract, _) = pair("power_play");
    let context = extract.summary_context();
    let base = summary_text("power_play").unwrap();
    // No header: nothing resolves to a side, every side-bound field is gone,
    // the extractor still finishes.
    let text = mutate(&base, |v| {
        v.as_object_mut().unwrap().remove("header");
    });
    let report = summarize(&text, context);
    assert_eq!(report.summary.shots, None);
    // No goalie group maps to a side either, which is §3.4's quiet
    // fallback — and the fallback play has no side to map to.
    assert_eq!(report.summary.strength, None);
    assert!(report.summary.goals.is_empty());
    assert!(
        report.summary.last_play.is_some(),
        "the flash needs no side"
    );
    assert_eq!(report.quirks.0, vec![Quirk::UnplaceableGoal]);

    // Scalars where objects belong are ignored, not fatal.
    let text = mutate(&base, |v| {
        v["plays"][3] = serde_json::json!(42);
        v["onIce"] = serde_json::json!("none");
        v["boxscore"]["players"] = serde_json::Value::Null;
    });
    let report = summarize(&text, context);
    assert_eq!(report.summary.shots, shots(16, 10));
    assert_eq!(
        report.quirks.0,
        vec![],
        "no goalie groups is the quiet fallback"
    );

    // Not JSON at all is the stream error, the caller's degrade-to-None.
    let mut scratch = vec![0u8; 4096];
    let mut extractor = SummaryExtractor::new(context, IgnoreQuirks, &mut scratch).unwrap();
    assert!(extractor
        .write(b"not json")
        .and_then(|()| extractor.finish().map(drop))
        .is_err());
}

// --------------------------------------------------------- crests

#[test]
fn every_fixture_exposes_both_crests_ordered_by_home_away() {
    for name in FIXTURES {
        let event = fixture(name);
        let value = parse(&event);
        let extract = found(&wrap(&[&event]), &id_of(&event));
        for competitor in value["competitions"][0]["competitors"].as_array().unwrap() {
            let href = competitor["team"]["logo"].as_str().unwrap();
            let path = href.strip_prefix("https://a.espncdn.com").unwrap();
            let crest = match competitor["homeAway"].as_str().unwrap() {
                "away" => &extract.crests().away,
                _ => &extract.crests().home,
            };
            assert_eq!(crest.as_ref().map(|c| c.as_str()), Some(path), "{name}");
            assert!(
                path.starts_with("/i/teamlogos/nhl/500/scoreboard/"),
                "{name}: {path}"
            );
        }
    }
}

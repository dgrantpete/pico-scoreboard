//! Wire-format corpus harness (tests only).
//!
//! Every fixture in `testdata/` is run through its sport's real transform,
//! encoded, and pinned byte-for-byte to a committed golden under
//! `testdata/wire/`. The goldens are the format's identity contract: they were
//! captured from the encoder that shipped to the deployed firmware, so any diff
//! here is a wire break, not a refactor artifact. They are also the input the
//! future Rust firmware's render-parity harness replays, which is why they are
//! committed as bytes rather than recomputed.
//!
//! Regenerate deliberately (and only alongside a `WIRE_VERSION` bump):
//! `UPDATE_WIRE_GOLDENS=1 cargo test -p backend`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::football::FootballGame;
use crate::mlb::MlbGame;
use crate::nba::NbaGame;
use crate::soccer::SoccerGame;

/// One corpus entry: the fixture's path relative to its sport directory
/// (without `.json`) and the bytes its game encodes to.
pub(crate) struct Encoded {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
}

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

/// Fixture stems under `testdata/{sport}`, recursing into the per-league
/// subdirectories football and soccer use. Sorted, so goldens and failures come
/// out in a stable order.
fn fixture_names(sport: &str) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}"));
        for entry in entries {
            let entry = entry.expect("readable dir entry");
            let name = entry.file_name().into_string().expect("utf-8 filename");
            if entry.file_type().expect("file type").is_dir() {
                walk(&entry.path(), &format!("{prefix}{name}/"), out);
            } else if let Some(stem) = name.strip_suffix(".json") {
                out.push(format!("{prefix}{stem}"));
            }
        }
    }
    let mut names = Vec::new();
    walk(&testdata().join(sport), "", &mut names);
    names.sort();
    names
}

fn read_fixture<E: serde::de::DeserializeOwned>(sport: &str, name: &str) -> E {
    let path = testdata().join(sport).join(format!("{name}.json"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path:?}: {e}"))
}

/// MLB: one entry per fixture, except the rain-delay fixture — a live game in a
/// non-inning state has no displayable payload and never reaches the encoder
/// (see `mlb::transform::parse_inning_half`).
pub(crate) fn mlb_games() -> Vec<(String, MlbGame)> {
    use crate::mlb::transform::{
        final_competition_to_game, live_competition_to_game, pregame_competition_to_game,
    };
    use crate::mlb::types::{EspnCompetition, EspnEvent};

    fixture_names("mlb")
        .into_iter()
        .filter_map(|name| {
            let event: EspnEvent = read_fixture("mlb", &name);
            let id = event.id;
            let competition = event.competitions.into_iter().next().expect("competition");
            let game = match competition {
                EspnCompetition::PreGame {
                    competitors,
                    venue_name,
                } => MlbGame::Pregame(
                    pregame_competition_to_game(
                        id,
                        &event.date,
                        event.weather.as_ref(),
                        venue_name,
                        competitors,
                    )
                    .expect("pregame transforms"),
                ),
                EspnCompetition::Live {
                    competitors,
                    situation,
                    period,
                    short_detail,
                } => {
                    match live_competition_to_game(id, competitors, situation, period, short_detail)
                    {
                        Ok(live) => MlbGame::Live(live),
                        Err(_) => return None,
                    }
                }
                EspnCompetition::Final {
                    competitors,
                    period,
                } => MlbGame::Final(
                    final_competition_to_game(id, competitors, period).expect("final transforms"),
                ),
            };
            Some((name, game))
        })
        .collect()
}

pub(crate) fn nba_games() -> Vec<(String, NbaGame)> {
    use crate::nba::transform::{
        final_competition_to_game, live_competition_to_game, pregame_competition_to_game,
    };
    use crate::nba::types::{EspnCompetition, EspnEvent};

    fixture_names("nba")
        .into_iter()
        .map(|name| {
            let event: EspnEvent = read_fixture("nba", &name);
            let id = event.id;
            let competition = event.competitions.into_iter().next().expect("competition");
            let game = match competition {
                EspnCompetition::PreGame {
                    competitors,
                    venue_name,
                } => NbaGame::Pregame(
                    pregame_competition_to_game(id, &event.date, venue_name, competitors)
                        .expect("pregame transforms"),
                ),
                EspnCompetition::Live {
                    competitors,
                    period,
                    display_clock,
                    phase,
                    situation,
                } => NbaGame::Live(
                    live_competition_to_game(
                        id,
                        competitors,
                        period,
                        display_clock,
                        phase,
                        situation,
                    )
                    .expect("live transforms"),
                ),
                EspnCompetition::Final {
                    competitors,
                    period,
                } => NbaGame::Final(
                    final_competition_to_game(id, competitors, period).expect("final transforms"),
                ),
            };
            (name, game)
        })
        .collect()
}

/// Football fixtures nest under their ESPN league slug; `college-football/`
/// drives `is_college` (the only per-league input to the transform).
pub(crate) fn football_games() -> Vec<(String, FootballGame)> {
    use crate::football::transform::{
        final_competition_to_game, live_competition_to_game, pregame_competition_to_game,
    };
    use crate::football::types::{EspnCompetition, EspnEvent};

    fixture_names("football")
        .into_iter()
        .map(|name| {
            let is_college = name.starts_with("college-football/");
            let event: EspnEvent = read_fixture("football", &name);
            let id = event.id;
            let competition = event.competitions.into_iter().next().expect("competition");
            let game = match competition {
                EspnCompetition::PreGame {
                    competitors,
                    venue_name,
                } => FootballGame::Pregame(
                    pregame_competition_to_game(
                        id,
                        &event.date,
                        venue_name,
                        competitors,
                        is_college,
                    )
                    .expect("pregame transforms"),
                ),
                EspnCompetition::Live {
                    competitors,
                    period,
                    display_clock,
                    phase,
                    situation,
                } => FootballGame::Live(
                    live_competition_to_game(
                        id,
                        competitors,
                        period,
                        display_clock,
                        phase,
                        situation,
                    )
                    .expect("live transforms"),
                ),
                EspnCompetition::Final {
                    competitors,
                    period,
                } => FootballGame::Final(
                    final_competition_to_game(id, competitors, period).expect("final transforms"),
                ),
            };
            (name, game)
        })
        .collect()
}

/// Soccer live games encode with `commentary: None` — commentary comes from a
/// separate summary endpoint that the scoreboard fixtures don't carry. The
/// commentary-bearing layout is covered by the crate's own goldens.
pub(crate) fn soccer_games() -> Vec<(String, SoccerGame)> {
    use crate::soccer::transform::{
        final_competition_to_game, live_competition_to_game, pregame_competition_to_game,
    };
    use crate::soccer::types::{EspnCompetition, EspnEvent};

    fixture_names("soccer")
        .into_iter()
        .map(|name| {
            let event: EspnEvent = read_fixture("soccer", &name);
            let id = event.id;
            let competition = event.competitions.into_iter().next().expect("competition");
            let game = match competition {
                EspnCompetition::PreGame {
                    competitors,
                    venue_name,
                } => SoccerGame::Pregame(
                    pregame_competition_to_game(id, &event.date, venue_name, competitors)
                        .expect("pregame transforms"),
                ),
                EspnCompetition::Live {
                    competitors,
                    display_clock,
                    clock_seconds,
                    period,
                    on_break,
                    details,
                } => SoccerGame::Live(
                    live_competition_to_game(
                        id,
                        competitors,
                        display_clock,
                        clock_seconds,
                        period,
                        on_break,
                        details,
                        None,
                    )
                    .expect("live transforms"),
                ),
                EspnCompetition::Final {
                    competitors,
                    details,
                    flavor,
                } => SoccerGame::Final(
                    final_competition_to_game(id, competitors, details, flavor)
                        .expect("final transforms"),
                ),
            };
            (name, game)
        })
        .collect()
}

/// Every corpus fixture, encoded, keyed by `{sport}/{fixture}`.
pub(crate) fn encoded_corpus() -> Vec<Encoded> {
    let mut out = Vec::new();
    let mut push = |sport: &str, name: String, bytes: Vec<u8>| {
        out.push(Encoded {
            name: format!("{sport}/{name}"),
            bytes,
        })
    };
    for (name, game) in mlb_games() {
        push("mlb", name, crate::mlb::wire::encode_game(&game));
    }
    for (name, game) in nba_games() {
        push("nba", name, crate::nba::wire::encode_game(&game));
    }
    for (name, game) in football_games() {
        push("football", name, crate::football::wire::encode_game(&game));
    }
    for (name, game) in soccer_games() {
        push("soccer", name, crate::soccer::wire::encode_game(&game));
    }
    out
}

/// Running per-class maximum. A struct rather than closures so the borrow
/// checker lets the sport walks interleave string and team measurements.
struct Maxima(BTreeMap<&'static str, usize>);

impl Maxima {
    fn note(&mut self, class: &'static str, text: &str) {
        let entry = self.0.entry(class).or_default();
        *entry = (*entry).max(text.len());
    }

    fn team(&mut self, team: &scoreboard_wire::TeamState<'_>) {
        self.note("abbreviation", team.abbreviation);
    }

    fn final_team(&mut self, team: &scoreboard_wire::FinalTeam<'_>) {
        self.note("abbreviation", team.abbreviation);
        let entry = self.0.entry("line score").or_default();
        *entry = (*entry).max(team.line_score.len());
    }
}

/// The longest string of each class the corpus produces, keyed by class. The
/// firmware's bounded snapshot fields (Phase 2) size themselves from these, so
/// the harness measures rather than guesses.
fn corpus_string_maxima() -> Vec<(&'static str, usize)> {
    use scoreboard_wire as wire;

    let mut max = Maxima(BTreeMap::new());

    for (name, game) in mlb_games() {
        let bytes = crate::mlb::wire::encode_game(&game);
        match wire::mlb::decode(&bytes).unwrap_or_else(|e| panic!("mlb/{name}: {e}")) {
            wire::mlb::Game::Live(live) => {
                max.note("game id", live.game_id);
                max.team(&live.away);
                max.team(&live.home);
                if let Some(at_bat) = live.at_bat {
                    max.note("player name", at_bat.pitcher);
                    max.note("player name", at_bat.batter);
                }
                max.note("play id", live.last_play.id);
                max.note("play text", live.last_play.text);
            }
            wire::mlb::Game::Pregame(pregame) => {
                max.note("game id", pregame.game_id);
                max.note("venue", pregame.venue);
                max.note("abbreviation", pregame.away.abbreviation);
                max.note("abbreviation", pregame.home.abbreviation);
                if let Some(weather) = pregame.weather {
                    max.note("weather condition", weather.condition);
                }
                for pitcher in [pregame.away.probable_pitcher, pregame.home.probable_pitcher]
                    .into_iter()
                    .flatten()
                {
                    max.note("player name", pitcher);
                }
            }
            wire::mlb::Game::Final(game) => {
                max.note("game id", game.game_id);
                max.final_team(&game.away);
                max.final_team(&game.home);
            }
        }
    }

    for (name, game) in nba_games() {
        let bytes = crate::nba::wire::encode_game(&game);
        match wire::nba::decode(&bytes).unwrap_or_else(|e| panic!("nba/{name}: {e}")) {
            wire::nba::Game::Live(live) => {
                max.note("game id", live.game_id);
                max.note("clock", live.clock);
                max.team(&live.away);
                max.team(&live.home);
                if let Some(play) = live.last_play {
                    max.note("play id", play.id);
                    max.note("play text", play.text);
                }
            }
            wire::nba::Game::Pregame(pregame) => {
                max.note("game id", pregame.game_id);
                max.note("venue", pregame.venue);
                max.note("abbreviation", pregame.away.abbreviation);
                max.note("abbreviation", pregame.home.abbreviation);
            }
            wire::nba::Game::Final(game) => {
                max.note("game id", game.game_id);
                max.final_team(&game.away);
                max.final_team(&game.home);
            }
        }
    }

    for (name, game) in football_games() {
        let bytes = crate::football::wire::encode_game(&game);
        match wire::football::decode(&bytes).unwrap_or_else(|e| panic!("football/{name}: {e}")) {
            wire::football::Game::Live(live) => {
                max.note("game id", live.game_id);
                max.note("clock", live.clock);
                max.team(&live.away);
                max.team(&live.home);
                if let Some(play) = live.last_play {
                    max.note("play id", play.id);
                    max.note("play text", play.text);
                }
            }
            wire::football::Game::Pregame(pregame) => {
                max.note("game id", pregame.game_id);
                max.note("venue", pregame.venue);
                max.note("abbreviation", pregame.away.abbreviation);
                max.note("abbreviation", pregame.home.abbreviation);
                for rank in [pregame.away.rank_line, pregame.home.rank_line]
                    .into_iter()
                    .flatten()
                {
                    max.note("rank line", rank);
                }
            }
            wire::football::Game::Final(game) => {
                max.note("game id", game.game_id);
                max.final_team(&game.away);
                max.final_team(&game.home);
            }
        }
    }

    for (name, game) in soccer_games() {
        let bytes = crate::soccer::wire::encode_game(&game);
        match wire::soccer::decode(&bytes).unwrap_or_else(|e| panic!("soccer/{name}: {e}")) {
            wire::soccer::Game::Live(live) => {
                max.note("game id", live.game_id);
                max.team(&live.away);
                max.team(&live.home);
                if let Some(event) = live.last_event {
                    max.note("clock", event.clock);
                    max.note("player name", event.athlete);
                }
                if let Some(commentary) = live.commentary {
                    max.note("play id", commentary.id);
                    max.note("play text", commentary.text);
                }
            }
            wire::soccer::Game::Pregame(pregame) => {
                max.note("game id", pregame.game_id);
                max.note("venue", pregame.venue);
                max.note("abbreviation", pregame.away.abbreviation);
                max.note("abbreviation", pregame.home.abbreviation);
            }
            wire::soccer::Game::Final(game) => {
                max.note("game id", game.game_id);
                max.note("abbreviation", game.away.abbreviation);
                max.note("abbreviation", game.home.abbreviation);
                max.note("scorers", game.away.scorers);
                max.note("scorers", game.home.scorers);
            }
        }
    }

    max.0.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn golden_path(name: &str) -> PathBuf {
        testdata().join("wire").join(format!("{name}.bin"))
    }

    /// Byte-identity gate. The corpus is fixed, so a new fixture or a dropped
    /// one shows up as a missing/orphaned golden rather than passing silently.
    #[test]
    fn corpus_encodes_to_the_committed_golden_bytes() {
        let corpus = encoded_corpus();
        assert!(!corpus.is_empty(), "corpus must not be empty");
        let blessing = std::env::var_os("UPDATE_WIRE_GOLDENS").is_some();

        for entry in &corpus {
            let path = golden_path(&entry.name);
            if blessing {
                std::fs::create_dir_all(path.parent().unwrap()).expect("golden dir");
                std::fs::write(&path, &entry.bytes).expect("write golden");
                continue;
            }
            let golden = std::fs::read(&path).unwrap_or_else(|e| {
                panic!("read golden {path:?}: {e} (bless with UPDATE_WIRE_GOLDENS=1)")
            });
            assert_eq!(
                hex::encode(&entry.bytes),
                hex::encode(&golden),
                "{} encodes differently than its golden",
                entry.name
            );
        }

        let mut on_disk = Vec::new();
        fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("golden dir readable") {
                let entry = entry.expect("readable dir entry");
                let name = entry.file_name().into_string().expect("utf-8 filename");
                if entry.file_type().expect("file type").is_dir() {
                    walk(&entry.path(), &format!("{prefix}{name}/"), out);
                } else if let Some(stem) = name.strip_suffix(".bin") {
                    out.push(format!("{prefix}{stem}"));
                }
            }
        }
        walk(&testdata().join("wire"), "", &mut on_disk);
        on_disk.sort();
        let mut expected: Vec<String> = corpus.iter().map(|e| e.name.clone()).collect();
        expected.sort();
        assert_eq!(
            on_disk, expected,
            "golden files and corpus entries disagree"
        );
    }

    /// Decode is only trustworthy if it is exact, so every corpus payload is
    /// decoded and re-encoded: any field the decoder drops, mis-offsets or
    /// rounds shows up as a byte diff.
    #[test]
    fn corpus_round_trips_through_decode() {
        use scoreboard_wire as wire;

        for entry in encoded_corpus() {
            let bytes = &entry.bytes;
            let mut out = Vec::with_capacity(bytes.len());
            let sport = entry.name.split('/').next().expect("sport-prefixed name");
            let decoded = match sport {
                "mlb" => wire::mlb::decode(bytes).map(|g| wire::mlb::encode(&g, &mut out)),
                "nba" => wire::nba::decode(bytes).map(|g| wire::nba::encode(&g, &mut out)),
                "football" => {
                    wire::football::decode(bytes).map(|g| wire::football::encode(&g, &mut out))
                }
                "soccer" => wire::soccer::decode(bytes).map(|g| wire::soccer::encode(&g, &mut out)),
                other => panic!("unknown sport {other}"),
            };
            decoded
                .unwrap_or_else(|e| panic!("{}: {e}", entry.name))
                .expect("a Vec sink never fills");
            assert_eq!(
                hex::encode(&out),
                hex::encode(bytes),
                "{} does not survive a decode/encode round trip",
                entry.name
            );
        }
    }

    /// What the corpus actually produces per string class, with the headroom the
    /// firmware's bounded snapshot fields (Phase 2) will be sized from. The wire
    /// itself caps strings at 255 bytes and decode borrows rather than copies,
    /// so nothing here constrains the format — this is a budget tripwire: a
    /// fixture that outgrows a line means the snapshot bound needs revisiting,
    /// not that the payload is invalid.
    #[test]
    fn corpus_strings_fit_the_snapshot_budget() {
        const BUDGET: &[(&str, usize)] = &[
            ("abbreviation", 8),
            ("clock", 12),
            ("game id", 16),
            ("line score", 16),
            ("play id", 24),
            ("play text", 128),
            ("player name", 32),
            ("rank line", 32),
            ("scorers", 128),
            ("venue", 48),
            ("weather condition", 24),
        ];

        let observed = corpus_string_maxima();
        for (class, longest) in &observed {
            let (_, budget) = BUDGET
                .iter()
                .find(|(name, _)| name == class)
                .unwrap_or_else(|| panic!("no budget line for {class}"));
            assert!(
                longest <= budget,
                "{class}: corpus reaches {longest} bytes, budget is {budget}"
            );
        }
        let measured: Vec<&str> = observed.iter().map(|(class, _)| *class).collect();
        let budgeted: Vec<&str> = BUDGET.iter().map(|(class, _)| *class).collect();
        assert_eq!(
            measured, budgeted,
            "every budget line must be exercised by the corpus"
        );
    }
}

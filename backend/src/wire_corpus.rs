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
                } => match live_competition_to_game(
                    id,
                    competitors,
                    situation,
                    period,
                    short_detail,
                ) {
                    Ok(live) => MlbGame::Live(live),
                    Err(_) => return None,
                },
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
        push("mlb", name, crate::wire::encode_mlb_game(&game));
    }
    for (name, game) in nba_games() {
        push("nba", name, crate::wire::encode_nba_game(&game));
    }
    for (name, game) in football_games() {
        push("football", name, crate::wire::encode_football_game(&game));
    }
    for (name, game) in soccer_games() {
        push("soccer", name, crate::wire::encode_soccer_game(&game));
    }
    out
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
            let golden = std::fs::read(&path)
                .unwrap_or_else(|e| panic!("read golden {path:?}: {e} (bless with UPDATE_WIRE_GOLDENS=1)"));
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
        assert_eq!(on_disk, expected, "golden files and corpus entries disagree");
    }
}

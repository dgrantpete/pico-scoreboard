//! Which crest the idle poller should fetch next, and the memory that keeps a
//! game from being asked about twice.
//!
//! The app owns the crest pool and the network; what belongs here is the
//! *order* — the same split [`crate::slate`] makes, for the same reason.
//!
//! # Why this needs a memory at all
//!
//! A crest is keyed by `{league key}/{abbreviation}`, and **the rotation does
//! not know either team's abbreviation**. The games list carries a state and an
//! id and nothing else (`scoreboard-wire`'s `list` module is the whole format,
//! and `WIRE_VERSION` is frozen), so the only way to learn who is playing is to
//! fetch that game's detail. "Warm the next missing crest in rotation order" is
//! therefore two steps, not one: [`Step::Probe`] learns a game's teams,
//! [`Step::Crest`] fetches one.
//!
//! A probe is a real request, so a warmer that forgot what it learned would pay
//! for one on every idle window forever. [`WarmIndex`] is what makes the probe a
//! once-per-game cost: it remembers a game's teams, and it is filled for free by
//! the poll loop's own commits ([`WarmIndex::learned`]), so a game the rotation
//! has already shown is never probed at all. Once every listed game is known and
//! every crest is in the pool, this module returns [`None`] and the idle
//! scoreboard makes no requests of any kind.
//!
//! # Why it is capacity-bounded and never evicts a record
//!
//! The pool holds `SLOTS` crests, so it holds at most `SLOTS / 2` *games* — a
//! warmer that tried to stay ahead of a longer slate would evict a crest it was
//! about to want, which is the thrash the pool expansion exists to end. So the
//! index is sized to what the pool can hold and simply stops recording when it
//! is full, and [`WarmIndex::next`] stops proposing probes at the same moment.
//! On a college-football Saturday the warmer fills the pool once and then goes
//! quiet for the day, which is the correct amount of work to do.
//!
//! [`WarmIndex::prune`] is what keeps that capacity honest: a record whose game
//! has left the slate is dropped, and games that merely left the *rotation* (the
//! live-first rule drops every pregame the moment one game starts) keep theirs.

use crate::slate::Slate;
use crate::snapshot::{ABBR, GAME_ID};
use crate::text::{Text, set_plain};

/// Consecutive failed attempts before a game is left alone.
///
/// Failures here are not the poll loop's failures — a warm fetch never touches
/// the failure streak — so nothing else will ever notice a game that cannot be
/// warmed. Without a ceiling, one team whose logo endpoint answers 404 would be
/// the first thing the selector proposed on every idle window for the rest of
/// the day, and nothing behind it in the rotation would ever be reached.
const ATTEMPTS: u8 = 3;

/// One game the warmer has heard of.
#[derive(Debug)]
struct Known {
    source: u8,
    id: Text<GAME_ID>,
    /// Both abbreviations, once a probe or a commit has supplied them.
    teams: Option<(Text<ABBR>, Text<ABBR>)>,
    /// Consecutive failures against this game; at [`ATTEMPTS`] it is skipped.
    misses: u8,
}

/// What the warmer should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Fetch this rotation entry's game detail, for its teams. The crests
    /// follow on later windows.
    Probe { position: u8 },
    /// Fetch this crest for this rotation entry.
    Crest {
        position: u8,
        abbreviation: Text<ABBR>,
    },
}

/// The teams of every game the warmer has learned about, and the rule for what
/// to fetch next.
///
/// `N` is the number of games worth remembering — `SLOTS / 2`, the most whose
/// crests the pool can hold at once. See the module docs.
#[derive(Debug)]
pub struct WarmIndex<const N: usize> {
    known: heapless::Vec<Known, N>,
}

impl<const N: usize> Default for WarmIndex<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> WarmIndex<N> {
    pub const fn new() -> Self {
        Self {
            known: heapless::Vec::new(),
        }
    }

    /// Record a game's teams — from a probe, or free of charge from a commit
    /// that had just decoded them anyway.
    ///
    /// Also clears the miss count: the game answered, so whatever went wrong
    /// before was not permanent.
    pub fn learned(&mut self, source: u8, id: &str, away: &str, home: &str) {
        let mut teams = (Text::new(), Text::new());
        set_plain(&mut teams.0, away);
        set_plain(&mut teams.1, home);
        match self.find_mut(source, id) {
            Some(known) => {
                known.teams = Some(teams);
                known.misses = 0;
            }
            None => self.push(source, id, Some(teams), 0),
        }
    }

    /// An attempt against this game failed. [`ATTEMPTS`] of these and the
    /// selector stops proposing it.
    pub fn missed(&mut self, source: u8, id: &str) {
        match self.find_mut(source, id) {
            Some(known) => known.misses = known.misses.saturating_add(1),
            // The first failure on a game the selector had only just proposed
            // probing. It proposes that only when there is room, so the push
            // lands.
            None => self.push(source, id, None, 1),
        }
    }

    /// Drop records for games the slate no longer lists. Call after a rebuild.
    pub fn prune(&mut self, slate: &Slate) {
        self.known
            .retain(|known| slate.lists(known.source, known.id.as_str()));
    }

    /// The next fetch worth making, or `None` when everything reachable is
    /// warm.
    ///
    /// Rotation order starting at the game on screen, which puts the *current*
    /// game's crests first — a commit whose crest fetch failed is retried here
    /// before anything is warmed ahead of it — and then walks forward,
    /// wrapping, so the next game to be shown is the next game warmed.
    ///
    /// `cached` answers whether a crest is already in the pool, given a league
    /// key and an abbreviation. The pool is the app's, and this is the whole of
    /// what this module needs to know about it.
    pub fn next<F>(&self, slate: &Slate, cached: F) -> Option<Step>
    where
        F: Fn(&str, &str) -> bool,
    {
        let len = slate.len();
        if len == 0 {
            return None;
        }
        let start = slate.position() as usize;
        for offset in 0..len {
            let position = ((start + offset) % len) as u8;
            let Some(entry) = slate.at(position) else {
                continue;
            };
            let Some(known) = self.find(entry.source, entry.id) else {
                // A game nothing has recorded. Probing it would record it, so
                // a full index has to stop asking — see the module docs.
                if self.known.len() < N {
                    return Some(Step::Probe { position });
                }
                continue;
            };
            if known.misses >= ATTEMPTS {
                continue;
            }
            let Some((away, home)) = known.teams.as_ref() else {
                return Some(Step::Probe { position });
            };
            for abbreviation in [away, home] {
                // An empty abbreviation is not a crest that exists — the pool's
                // own fetch refuses it — so it must not be proposed, or it
                // would be proposed forever.
                if !abbreviation.is_empty() && !cached(entry.league.key.as_str(), abbreviation) {
                    return Some(Step::Crest {
                        position,
                        abbreviation: abbreviation.clone(),
                    });
                }
            }
        }
        None
    }

    fn find(&self, source: u8, id: &str) -> Option<&Known> {
        self.known
            .iter()
            .find(|known| known.source == source && known.id == id)
    }

    fn find_mut(&mut self, source: u8, id: &str) -> Option<&mut Known> {
        self.known
            .iter_mut()
            .find(|known| known.source == source && known.id == id)
    }

    /// Add a record if there is room. Dropping it when there is none is the
    /// intended behaviour, not a failure: a full index means the pool is
    /// already carrying every game it can.
    fn push(&mut self, source: u8, id: &str, teams: Option<(Text<ABBR>, Text<ABBR>)>, misses: u8) {
        let mut known = Known {
            source,
            id: Text::new(),
            teams,
            misses,
        };
        set_plain(&mut known.id, id);
        let _ = self.known.push(known);
    }
}

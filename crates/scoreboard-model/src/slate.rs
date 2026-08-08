//! The merged game slate and the rotation over it.
//!
//! One poller owns every configured league — the API client allows a single
//! in-flight request — so the slates merge into one rotation rather than one
//! per league. Everything here is pure: the app fetches and decodes, then hands
//! entries in, and asks what to show next.

use scoreboard_wire::GameState;

use crate::feed::{LeagueId, ListSink};
use crate::snapshot::GAME_ID;
use crate::text::{Text, set_folded};

/// Leagues that can be configured at once: MLB, NBA, two football leagues,
/// and up to four soccer leagues, with room to spare. Also the width of the
/// filter bitmask.
pub const MAX_SOURCES: usize = 8;

/// Games held across every league. A college-football Saturday is the worst
/// case the corpus suggests (~70 games) alongside a full MLB slate; entries
/// past this are dropped, oldest league first, rather than displacing the
/// leagues already listed.
pub const MAX_SLATE: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlateEntry {
    source: u8,
    state: GameState,
    id: Text<GAME_ID>,
}

/// Every configured league's games, plus the rotation order and the position
/// in it.
#[derive(Debug, Clone)]
pub struct Slate {
    sources: heapless::Vec<LeagueId, MAX_SOURCES, u8>,
    /// Grouped by source only as an artifact of insertion; [`Slate::rebuild`]
    /// imposes the configured order, so removals never have to shift.
    entries: heapless::Vec<SlateEntry, MAX_SLATE, u8>,
    /// Indices into `entries`, in rotation order.
    rotation: heapless::Vec<u8, MAX_SLATE, u8>,
    index: u8,
    /// Rotation restricted to these sources; `None` is every league. Session
    /// state by design — the persisted config owns which leagues are polled at
    /// all, and this resets on reboot.
    filter: Option<u8>,
    locked: bool,
    /// The `(source, game id)` on screen, cached so a slate rebuild can put
    /// the index back on it even though the entries themselves moved.
    current: Option<(u8, Text<GAME_ID>)>,
}

impl Default for Slate {
    fn default() -> Self {
        Self::new()
    }
}

impl Slate {
    pub const fn new() -> Self {
        Self {
            sources: heapless::Vec::new(),
            entries: heapless::Vec::new(),
            rotation: heapless::Vec::new(),
            index: 0,
            filter: None,
            locked: false,
            current: None,
        }
    }

    /// Install the configured leagues, in poll order: MLB, NBA, football
    /// leagues, soccer leagues. Drops every cached game — a source list only
    /// changes on a config write, which invalidates the indices anyway.
    ///
    /// The session league filter clears with it. `poller.py` kept the filter
    /// keyed by league key so it could survive this; a config write is already
    /// a disruptive event and the menu is the only thing that sets a filter.
    pub fn set_sources(&mut self, sources: &[LeagueId]) {
        self.sources.clear();
        for source in sources.iter().take(MAX_SOURCES) {
            let _ = self.sources.push(source.clone());
        }
        self.entries.clear();
        self.rotation.clear();
        self.index = 0;
        self.filter = None;
        self.current = None;
    }

    pub fn sources(&self) -> &[LeagueId] {
        &self.sources
    }

    /// Bytes the merged slate occupies — a budget line in its own right, and
    /// the same on the host and on `thumbv8m` (see [`crate::text::Text`]).
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Replace one source's games: drop what it had, then take what
    /// [`crate::GameFeed::list`] decodes into the returned sink. The rotation
    /// only sees the result at the next [`Slate::rebuild`].
    ///
    /// A source that fails to refresh simply never calls this, so it keeps its
    /// cached slate: a dead league feed must not blank the others. A payload
    /// that decodes partway and *then* errors leaves that source short until
    /// the next refresh — the transport failures that dominate in practice
    /// never get this far.
    pub fn update_source(&mut self, source: u8) -> SourceUpdate<'_> {
        self.entries.retain(|entry| entry.source != source);
        SourceUpdate {
            slate: self,
            source,
        }
    }

    /// Rebuild the rotation from the merged slate. Call once per refresh pass,
    /// after every source has had its turn.
    ///
    /// **Live-first across the whole slate**: while any listed game anywhere is
    /// live, only live games rotate. With none live, finals rotate before
    /// pregames — leagues in configured order, feed order within a league.
    ///
    /// The game on screen keeps its position if it is still listed, so an
    /// unrelated game flipping state cannot yank the view mid-dwell.
    pub fn rebuild(&mut self) {
        let filter = match self.filter {
            // Every filtered league left the configured sources.
            Some(mask) if mask & self.source_mask() == 0 => {
                self.filter = None;
                None
            }
            other => other,
        };

        self.collect_rotation(filter);
        if self.rotation.is_empty() && filter.is_some() {
            // The filter is kept — its games may come back — but a board that
            // shows nothing is worse than one showing an unfiltered game.
            self.collect_rotation(None);
        }

        let restored = match self.current.as_ref() {
            Some((source, id)) => self.position_of(*source, id).unwrap_or(0),
            None => 0,
        };
        self.index = restored;
        self.sync_current();
    }

    fn source_mask(&self) -> u8 {
        match self.sources.len() {
            0 => 0,
            len => (1u16 << len).wrapping_sub(1) as u8,
        }
    }

    fn collect_rotation(&mut self, filter: Option<u8>) {
        self.rotation.clear();
        self.push_state(filter, GameState::Live);
        if self.rotation.is_empty() {
            self.push_state(filter, GameState::Final);
            self.push_state(filter, GameState::Pregame);
        }
    }

    fn push_state(&mut self, filter: Option<u8>, state: GameState) {
        for source in 0..self.sources.len() as u8 {
            if filter.is_some_and(|mask| mask & (1 << source) == 0) {
                continue;
            }
            for (position, entry) in self.entries.iter().enumerate() {
                if entry.source == source && entry.state == state {
                    let _ = self.rotation.push(position as u8);
                }
            }
        }
    }

    fn position_of(&self, source: u8, id: &str) -> Option<u8> {
        let position = self.rotation.iter().position(|&entry| {
            let entry = &self.entries[entry as usize];
            entry.source == source && entry.id == id
        })?;
        u8::try_from(position).ok()
    }

    fn sync_current(&mut self) {
        let current = self.rotation.get(self.index as usize).map(|&entry| {
            let entry = &self.entries[entry as usize];
            (entry.source, entry.id.clone())
        });
        self.current = current;
    }

    /// True when no league listed a single game — the only thing that shows
    /// the `no games` screen.
    pub fn is_empty(&self) -> bool {
        self.rotation.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rotation.len()
    }

    /// The league and game id to poll now.
    pub fn current(&self) -> Option<(&LeagueId, &str)> {
        let entry = &self.entries[*self.rotation.get(self.index as usize)? as usize];
        Some((&self.sources[entry.source as usize], entry.id.as_str()))
    }

    /// Step to the next game.
    pub fn advance(&mut self) {
        if self.rotation.is_empty() {
            return;
        }
        self.index = (self.index + 1) % self.rotation.len() as u8;
        self.sync_current();
    }

    /// Step to the first game of the next league, scanning forward cyclically.
    /// A single-league slate degrades to a plain [`Slate::advance`].
    ///
    /// Stays *within* the league filter: the filter is a deliberate
    /// multi-select, unlike the one-league lock a skip used to escape.
    pub fn advance_league(&mut self) {
        let len = self.rotation.len() as u8;
        if len == 0 {
            return;
        }
        let Some(&(current_source, _)) = self.current.as_ref() else {
            self.advance();
            return;
        };
        for step in 1..=len {
            let candidate = (self.index + step) % len;
            let entry = &self.entries[self.rotation[candidate as usize] as usize];
            if entry.source != current_source {
                self.index = candidate;
                self.sync_current();
                return;
            }
        }
        self.advance();
    }

    /// Rotation lock. Polling of the current game continues either way.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Toggle the lock, returning the new state.
    pub fn toggle_lock(&mut self) -> bool {
        self.locked = !self.locked;
        self.locked
    }

    /// The active filter as a source bitmask; `None` is every league.
    pub fn filter(&self) -> Option<u8> {
        self.filter
    }

    /// Restrict the rotation to the leagues named by `keys`. A set covering
    /// every configured source clears the filter instead of storing a
    /// no-op. Returns whether anything changed.
    ///
    /// The lock is independent and survives. If the locked game's league is
    /// filtered out, the rebuild moves the board to a filtered-in game despite
    /// the lock — the user just excluded that league on purpose.
    pub fn set_filter(&mut self, keys: &[&str]) -> bool {
        let mut mask = 0u8;
        for (index, source) in self.sources.iter().enumerate() {
            if keys.contains(&source.key.as_str()) {
                mask |= 1 << index;
            }
        }
        let filter = (mask != self.source_mask()).then_some(mask);
        if filter == self.filter {
            return false;
        }
        self.filter = filter;
        self.rebuild();
        true
    }

    pub fn clear_filter(&mut self) -> bool {
        if self.filter.is_none() {
            return false;
        }
        self.filter = None;
        self.rebuild();
        true
    }
}

/// One source's list refresh in progress. Entries land as they decode; the
/// update is only visible to the rotation after [`Slate::rebuild`].
pub struct SourceUpdate<'a> {
    slate: &'a mut Slate,
    source: u8,
}

impl ListSink for SourceUpdate<'_> {
    fn entry(&mut self, state: GameState, id: &str) -> bool {
        let mut entry = SlateEntry {
            source: self.source,
            state,
            id: Text::new(),
        };
        set_folded(&mut entry.id, id);
        self.slate.entries.push(entry).is_ok()
    }
}

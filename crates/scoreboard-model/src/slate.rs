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

/// Leagues that can be configured at once: exactly the registered leagues
/// ([`crate::feed::LEAGUES`] — MLB, NBA, NHL, two football, four soccer), so
/// every combination the settings page offers fits.
///
/// No headroom, on purpose. Each source costs a league descriptor here and an
/// ETag slot in the app's poller, and spare slots would hide the moment this
/// needs rethinking: `scoreboard-config`'s
/// `every_registered_league_fits_the_slate` fails the day a tenth league is
/// registered, and that failure is the cue to bump this and re-measure the
/// poller arena. A hand-written config can still name more (unregistered
/// slugs poll by design); `DeviceConfig::league_sources` counts those rather
/// than dropping them silently.
///
/// Also why the configuration's league mask (`DeviceConfig::enabled_leagues`,
/// one bit per registered league) is a `u16`.
pub const MAX_SOURCES: usize = 9;

const _: () = assert!(MAX_SOURCES <= u16::BITS as usize);

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

/// One rotation entry, as [`Slate::at`] hands it out.
///
/// `source` is the index into [`Slate::sources`] rather than the league itself:
/// it is one byte, it is stable for as long as the configured sources are, and
/// it is what anything keying per-game state can afford to store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    pub source: u8,
    pub league: &'a LeagueId,
    pub id: &'a str,
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
            locked: false,
            current: None,
        }
    }

    /// Install the configured leagues, in poll order: MLB, NBA, NHL, football
    /// leagues, soccer leagues. Drops every cached game: the sources change
    /// whenever the configuration does — a settings save, or the device's own
    /// league menu, which edits the same `sports` section — and every entry's
    /// source index is meaningless against the new list anyway.
    ///
    /// The lock survives. It is about the game on screen, not the league list,
    /// and the caller's next refresh decides what is on screen now.
    pub fn set_sources(&mut self, sources: &[LeagueId]) {
        self.sources.clear();
        for source in sources.iter().take(MAX_SOURCES) {
            let _ = self.sources.push(source.clone());
        }
        self.entries.clear();
        self.rotation.clear();
        self.index = 0;
        self.current = None;
    }

    pub fn sources(&self) -> &[LeagueId] {
        &self.sources
    }

    /// Bytes the merged slate occupies — a budget line in its own right, and
    /// the same on the host and on `thumbv8m` (see [`crate::text::Text`]).
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Replace one source's games: drop what it had, then take what
    /// [`crate::WireFeed::list`] decodes into the returned sink. The rotation
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
        self.collect_rotation();
        let restored = match self.current.as_ref() {
            Some((source, id)) => self.position_of(*source, id).unwrap_or(0),
            None => 0,
        };
        self.index = restored;
        self.sync_current();
    }

    fn collect_rotation(&mut self) {
        self.rotation.clear();
        self.push_state(GameState::Live);
        if self.rotation.is_empty() {
            self.push_state(GameState::Final);
            self.push_state(GameState::Pregame);
        }
    }

    fn push_state(&mut self, state: GameState) {
        for source in 0..self.sources.len() as u8 {
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

    /// Where in the rotation the board is. Only meaningful next to
    /// [`Slate::at`], which is why the two are the whole of the read side.
    pub fn position(&self) -> u8 {
        self.index
    }

    /// The rotation entry at `position`, or `None` past the end.
    pub fn at(&self, position: u8) -> Option<Entry<'_>> {
        let entry = &self.entries[*self.rotation.get(position as usize)? as usize];
        Some(Entry {
            source: entry.source,
            league: &self.sources[entry.source as usize],
            id: entry.id.as_str(),
        })
    }

    /// The game to poll now.
    pub fn current(&self) -> Option<Entry<'_>> {
        self.at(self.index)
    }

    /// Whether the merged slate still lists this game — *anywhere*, not only in
    /// the rotation.
    ///
    /// The distinction is the live-first rule: one game going live drops every
    /// pregame out of the rotation without those games leaving the day. A
    /// consumer that keys state to a game (the crest warmer's index) wants the
    /// slate's answer, not the rotation's, or it discards and rebuilds that
    /// state every time a game starts.
    pub fn lists(&self, source: u8, id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.source == source && entry.id == id)
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

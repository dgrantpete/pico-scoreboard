//! The team-crest pool: eight 24×24 RGB565 slots, LRU, split across the two
//! cores.
//!
//! Port of `display.py`'s `LogoPool` (`:496-585`). The pool exists because
//! repeated allocation fragmented MicroPython's heap; here there is no heap at
//! all, so it exists for the other reason — a snapshot carries a
//! [`LogoRef`](scoreboard_model::snapshot::LogoRef) handle rather than 2,304
//! bytes of pixels, and the handle has to point at something that outlives the
//! commit.
//!
//! # Why the pixels live on core 1 and the bookkeeping on core 0
//!
//! MicroPython kept one pool that Core 0 filled and Core 1 read, and accepted
//! the race: evicting a slot the displayed state still referenced would tear a
//! crest mid-frame. That is a benign race in Python and undefined behaviour in
//! Rust — a `&[LogoSlot]` handed to the renderer asserts the bytes do not
//! change while it lives, and no amount of care on core 0 makes that true if
//! core 0 can write them.
//!
//! So the pool is cut where the two cores' needs actually divide:
//!
//! - **Core 1 owns the pixels** ([`CrestPool`]), exclusively, by value. The
//!   renderer's borrow is sound because there is no second reference.
//! - **Core 0 owns the directory** ([`CrestDirectory`]): which key is in which
//!   slot, the LRU order, and the fetch. No pixels.
//! - New pixels cross as a message ([`CrestUpdate`]) on a channel core 1 drains
//!   at the top of a frame, exactly as [`crate::settings`] hands over a config
//!   change.
//!
//! It costs one 1,152-byte copy per crest fetched — a few microseconds, on a
//! path that runs once per team per boot — and it removes the tear entirely.
//!
//! # Ordering
//!
//! Core 1 **latches the snapshot first, then drains this channel**. Core 0 does
//! the reverse: it sends the update, then publishes the snapshot that
//! references it. A snapshot naming slot 3 can therefore never be rendered
//! before slot 3's pixels have arrived — if the latch saw the publish, the send
//! that preceded it is already in the channel.
//!
//! Eviction adds the other half of the same argument: a slot the *published*
//! snapshot references is never chosen as a victim ([`CrestDirectory::hold`]),
//! so the pixels behind a handle core 1 is currently drawing cannot be
//! replaced. `LogoPool` relied on LRU order to make that unlikely; here it is
//! a rule.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use scoreboard_model::feed::LEAGUE_KEY;
use scoreboard_model::snapshot::{ABBR, LogoRef};
use scoreboard_model::store::Logos;
use scoreboard_render::game::{LOGO_BYTES, LogoSlot};

use crate::net::api_client::{ApiClient, url};

/// Crests held at once. `display.py`'s `LogoPool(size=8)`: two per game, so
/// eight covers the current game, the one before it, and two more.
pub const SLOTS: usize = 8;

/// `{league key}/{abbreviation}` — `LeagueSource.logo_key`. League-namespaced
/// because a soccer "POR" crest is not an MLB "POR".
pub type CrestKey = heapless::String<{ LEAGUE_KEY + 1 + ABBR }>;

/// One slot's new pixels, on their way to core 1.
pub struct CrestUpdate {
    slot: u8,
    pixels: LogoSlot,
}

/// Capacity two: a commit installs at most two crests before it publishes, so
/// the poller never waits on core 1 inside a commit. A third would mean two
/// commits inside one frame, where waiting a frame is the right answer anyway.
static UPDATES: Channel<CriticalSectionRawMutex, CrestUpdate, 2> = Channel::new();

/// Core 1's half: the pixels, and nothing else.
pub struct CrestPool {
    slots: [LogoSlot; SLOTS],
}

impl CrestPool {
    pub const fn new() -> CrestPool {
        CrestPool {
            slots: [[0; LOGO_BYTES]; SLOTS],
        }
    }

    /// Apply every pending update. Call after latching the snapshot and before
    /// rendering it — see the module docs' ordering rule.
    pub fn apply_pending(&mut self) {
        while let Ok(update) = UPDATES.try_receive() {
            if let Some(slot) = self.slots.get_mut(update.slot as usize) {
                slot.copy_from_slice(&update.pixels);
            }
        }
    }

    /// What the renderer borrows for the frame.
    pub fn slots(&self) -> &[LogoSlot] {
        &self.slots
    }
}

impl Default for CrestPool {
    fn default() -> CrestPool {
        CrestPool::new()
    }
}

/// Core 0's half: which key is where, and how to get one that is missing.
pub struct CrestDirectory {
    /// The key in each slot; `None` for a slot that has never been filled.
    keys: [Option<CrestKey>; SLOTS],
    /// Slot indices, least recently used first.
    lru: heapless::Vec<u8, SLOTS>,
    /// Slots the published snapshot is drawing from, and therefore not
    /// available to evict.
    held: Logos,
}

impl CrestDirectory {
    pub const fn new() -> CrestDirectory {
        CrestDirectory {
            keys: [const { None }; SLOTS],
            lru: heapless::Vec::new(),
            held: Logos {
                away: None,
                home: None,
            },
        }
    }

    /// Record which crests the snapshot just published draws. Called by the
    /// poller immediately after `publish`.
    pub fn hold(&mut self, logos: Logos) {
        self.held = logos;
    }

    /// The crest for `key`, fetching it on a miss.
    ///
    /// `LogoPool.get`, with two of its three guards gone as unreachable. The
    /// post-`await` cache re-check guarded against a second caller having
    /// filled the same key while this one was suspended; there is one caller
    /// and it holds `&mut self`, so no second call can exist. The
    /// `_free_slots` bookkeeping is likewise unnecessary — a slot is free
    /// exactly when its key is `None`.
    ///
    /// A failed fetch returns `None` and **is not an error**: a league whose
    /// crest endpoint 404s must not count against the poll's failure streak.
    /// Nothing is written to the slot, so a failure cannot leave a torn crest
    /// behind either, which is one better than `LogoPool` managed.
    pub async fn get(
        &mut self,
        base: &str,
        league_key: &str,
        abbreviation: &str,
        client: &mut ApiClient,
        buf: &mut [u8],
    ) -> Option<LogoRef> {
        if abbreviation.is_empty() {
            return None;
        }
        let mut key = CrestKey::new();
        // Lower-cased, as `LogoPool.get` did — ESPN's abbreviation casing is
        // not stable across endpoints and a case flip must not cost a slot.
        for character in league_key
            .chars()
            .chain(core::iter::once('/'))
            .chain(abbreviation.chars())
        {
            if key.push(character.to_ascii_lowercase()).is_err() {
                break;
            }
        }

        if let Some(slot) = self.cached(&key) {
            self.touch(slot);
            defmt::debug!("logo: hit {=str} in slot {}", key.as_str(), slot);
            return Some(LogoRef(slot));
        }

        let url = url(
            base,
            format_args!("/{league_key}/teams/{abbreviation}/logo?width=24&height=24&background_color=000000"),
        )
        .ok()?;
        // `background_color=000000` is `LogoPool`'s: the panel has no alpha, so
        // transparency has to be resolved against the background the crest is
        // drawn on, and that background is black.
        let pixels = match client.team_logo(&url, buf).await {
            Ok(Some(pixels)) => pixels,
            Ok(None) => return None,
            Err(error) => {
                crate::error!(
                    "logo: {} failed, {}",
                    key.as_str(),
                    crate::poller::describe(&error)
                );
                return None;
            }
        };
        if pixels.len() != LOGO_BYTES {
            crate::error!(
                "logo: {} is {} B, expected {}",
                key.as_str(),
                pixels.len(),
                LOGO_BYTES
            );
            return None;
        }

        let slot = self.claim()?;
        let mut update = CrestUpdate {
            slot,
            pixels: [0; LOGO_BYTES],
        };
        update.pixels.copy_from_slice(pixels);
        UPDATES.send(update).await;

        self.keys[slot as usize] = Some(key.clone());
        self.touch(slot);
        crate::debug!("logo: cached {} in slot {}", key.as_str(), slot);
        Some(LogoRef(slot))
    }

    fn cached(&self, key: &CrestKey) -> Option<u8> {
        self.keys
            .iter()
            .position(|held| held.as_ref() == Some(key))
            .map(|slot| slot as u8)
    }

    /// Move `slot` to the most-recently-used end.
    fn touch(&mut self, slot: u8) {
        self.lru.retain(|held| *held != slot);
        // Capacity is `SLOTS` and every entry is distinct, so the push after a
        // retain of the same value cannot overflow.
        let _ = self.lru.push(slot);
    }

    /// A slot to fill: an empty one, else the least recently used that the
    /// published snapshot is not drawing from.
    ///
    /// `None` is unreachable — at most two slots are held and there are eight —
    /// and returning it rather than asserting means a future pool of two would
    /// skip the crest instead of panicking on a live device.
    fn claim(&mut self) -> Option<u8> {
        if let Some(empty) = self.keys.iter().position(Option::is_none) {
            return Some(empty as u8);
        }
        let held = [self.held.away, self.held.home];
        let victim = self
            .lru
            .iter()
            .copied()
            .find(|slot| !held.contains(&Some(LogoRef(*slot))))?;
        // `LogoPool` logged every eviction, and it is the line that says whether
        // eight slots is enough: a pool that thrashes evicts a key it is about
        // to want again, which shows up here as the same key cycling.
        crate::debug!(
            "logo: evicting slot {} ({})",
            victim,
            self.keys[victim as usize]
                .as_ref()
                .map_or("", CrestKey::as_str)
        );
        self.keys[victim as usize] = None;
        Some(victim)
    }
}

impl Default for CrestDirectory {
    fn default() -> CrestDirectory {
        CrestDirectory::new()
    }
}

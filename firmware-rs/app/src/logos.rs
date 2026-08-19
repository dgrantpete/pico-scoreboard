//! The team-crest pool: thirty-two 24×24 RGB565 slots, LRU, split across the
//! two cores.
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
//! a rule. The rule does not care how many slots there are — two are held out
//! of [`SLOTS`], whatever `SLOTS` is — so growing the pool leaves it exactly as
//! it was, with more room between the held slots and the victim.
//!
//! # Two ways in, and only one of them evicts
//!
//! [`CrestDirectory::get`] is the rotation's: a commit needs this crest, so it
//! takes a free slot or makes one. [`CrestDirectory::prefetch`] is the idle
//! warmer's ([`crate::poller`]), and it **only ever fills a free slot**. That
//! asymmetry is what makes the warmer unable to hurt the thing it exists to
//! help: it cannot evict a crest the rotation is about to want, so a slate with
//! more teams than the pool holds ends with a full pool and a silent warmer
//! rather than with two fetchers evicting each other's work.
//!
//! # Two arms below it, and only one of them decodes
//!
//! A proxy build asks the backend for a finished 1,152-byte sprite. A `direct`
//! build has no backend, so it asks ESPN's CDN for a PNG and decodes it here —
//! `get_direct` and `prefetch_direct`, at the bottom of this file. That is the
//! whole of the difference.
//!
//! Everything *above* the fetch is written twice and deliberately so: four
//! entry points whose preambles are five lines each, because the alternative is
//! a combinator that returns "the key, unless it was a hit, unless there is no
//! room" and reads worse than the duplication. Everything *below* it —
//! `install`'s claim, the channel send, the LRU record — exists exactly once,
//! because that is where the core-1 safety argument lives and an argument with
//! two implementations is an argument that is true of one of them.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use scoreboard_model::feed::LEAGUE_KEY;
use scoreboard_model::snapshot::{ABBR, LogoRef};
use scoreboard_model::store::Logos;
use scoreboard_render::game::{LOGO_BYTES, LogoSlot};

use crate::net::api_client::{ApiClient, url};

/// Crests held at once, at 1,152 B of core-1 RAM each.
///
/// `display.py`'s `LogoPool(size=8)` covered the current game, the one before
/// it and two more, which is enough to rotate without thrashing and not enough
/// to ever stop fetching: a full MLB day is 15 games and 30 teams, so an
/// eight-slot pool evicted a crest it wanted again one lap later, every lap,
/// for the whole evening.
///
/// Thirty-two is the number that ends that. It holds a full MLB slate with two
/// slots spare and an NFL Sunday's 32 teams exactly, so on the two days that
/// dominate the corpus the pool converges after one lap and never evicts again.
/// It costs 27,648 B of core-1 RAM over the eight-slot pool, sanctioned against
/// the ≥ 40 % headroom target in `firmware-rs/BUDGET.md`.
///
/// A college-football Saturday (~70 games) still overflows it, and is meant to:
/// see [`crate::poller`]'s warmer, which stops rather than thrash.
pub const SLOTS: usize = 32;

/// Games the warmer remembers the teams of — see
/// [`scoreboard_model::prefetch`], which explains why it needs to remember any.
///
/// Two crests per game, so a pool of [`SLOTS`] can hold this many games and no
/// more. Sizing the memory to the pool is what makes "the index is full" and
/// "the pool is full" the same moment, and both of them mean *stop*.
pub const WARM_GAMES: usize = SLOTS / 2;

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
///
/// The warmer does not need a third either, and this is the one place its
/// per-window budget could have mattered. It does not, because the budget is
/// spread rather than burst: every send has a whole HTTP request in front of
/// it, and core 1 drains this once per frame at 60 FPS, so a send finds the
/// channel empty for the same reason a commit's does — however many the warmer
/// is allowed in a window.
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
    #[cfg_attr(
        feature = "direct",
        allow(
            dead_code,
            reason = "the direct build's crests come from the CDN through `get_direct`; this arm is the proxy build's"
        )
    )]
    pub async fn get(
        &mut self,
        base: &str,
        league_key: &str,
        abbreviation: &str,
        client: &mut ApiClient,
        buf: &mut [u8],
    ) -> Option<LogoRef> {
        let key = Self::key(league_key, abbreviation)?;
        if let Some(slot) = self.cached(&key) {
            self.touch(slot);
            defmt::debug!("logo: hit {=str} in slot {}", key.as_str(), slot);
            return Some(LogoRef(slot));
        }
        let url = Self::logo_url(base, league_key, abbreviation)?;
        self.fetch(&url, key, Room::Evict, client, buf).await
    }

    /// Fill a **free** slot with a crest the rotation has not asked for yet.
    ///
    /// The idle warmer's entry point, and the only difference from
    /// [`CrestDirectory::get`] is the one that matters: this never evicts. A
    /// warmed crest is a guess about what the rotation will want next, and a
    /// guess must never displace a crest that something already wanted.
    #[cfg_attr(
        feature = "direct",
        allow(
            dead_code,
            reason = "the direct build warms through `prefetch_direct`; this arm is the proxy build's"
        )
    )]
    pub async fn prefetch(
        &mut self,
        base: &str,
        league_key: &str,
        abbreviation: &str,
        client: &mut ApiClient,
        buf: &mut [u8],
    ) -> Warm {
        let Some(key) = Self::key(league_key, abbreviation) else {
            return Warm::Failed;
        };
        if self.cached(&key).is_some() {
            return Warm::Cached;
        }
        if self.keys.iter().all(Option::is_some) {
            return Warm::Full;
        }
        let Some(url) = Self::logo_url(base, league_key, abbreviation) else {
            return Warm::Failed;
        };
        match self.fetch(&url, key, Room::Spare, client, buf).await {
            Some(_) => Warm::Cached,
            None => Warm::Failed,
        }
    }

    /// Whether this crest is in the pool already. The question the warmer's
    /// selector asks, and the whole of what `scoreboard-model` knows about the
    /// pool.
    ///
    /// Deliberately does **not** touch the LRU: asking about a crest is not
    /// using one, and a selector that reordered the LRU by scanning it would
    /// make eviction depend on how often the board is idle.
    pub fn holds(&self, league_key: &str, abbreviation: &str) -> bool {
        Self::key(league_key, abbreviation)
            .is_some_and(|key| self.cached(&key).is_some())
    }

    /// `{league key}/{abbreviation}`, lower-cased — as `LogoPool.get` did,
    /// because ESPN's abbreviation casing is not stable across endpoints and a
    /// case flip must not cost a slot.
    ///
    /// `None` for an empty abbreviation, which is not a crest that exists.
    fn key(league_key: &str, abbreviation: &str) -> Option<CrestKey> {
        if abbreviation.is_empty() {
            return None;
        }
        let mut key = CrestKey::new();
        for character in league_key
            .chars()
            .chain(core::iter::once('/'))
            .chain(abbreviation.chars())
        {
            if key.push(character.to_ascii_lowercase()).is_err() {
                break;
            }
        }
        Some(key)
    }

    /// The crest endpoint for one team.
    ///
    /// `background_color=000000` is `LogoPool`'s: the panel has no alpha, so
    /// transparency has to be resolved against the background the crest is
    /// drawn on, and that background is black.
    #[cfg_attr(
        feature = "direct",
        allow(
            dead_code,
            reason = "the direct build builds CDN combiner URLs instead; see `CREST_PIXELS`"
        )
    )]
    fn logo_url(
        base: &str,
        league_key: &str,
        abbreviation: &str,
    ) -> Option<heapless::String<{ crate::net::api_client::URL_BYTES }>> {
        url(
            base,
            format_args!("/{league_key}/teams/{abbreviation}/logo?width=24&height=24&background_color=000000"),
        )
        .ok()
    }

    /// The proxy build's fetch: the backend has already resized, flattened and
    /// packed the crest, so this is a request, a size check and
    /// [`CrestDirectory::install`]. Shared by both entry points; `room` is the
    /// only thing they disagree about.
    #[cfg_attr(
        feature = "direct",
        allow(
            dead_code,
            reason = "the direct build's fetch is `fetch_png`; this arm is the proxy build's"
        )
    )]
    async fn fetch(
        &mut self,
        url: &str,
        key: CrestKey,
        room: Room,
        client: &mut ApiClient,
        buf: &mut [u8],
    ) -> Option<LogoRef> {
        let pixels = match client.team_logo(url, buf).await {
            Ok(Some(pixels)) => pixels,
            Ok(None) => return None,
            Err(error) => {
                // The warmer's failures are not worth an ERROR line — nothing
                // is waiting on them and it will try again — so the level
                // follows who asked.
                match room {
                    Room::Evict => crate::error!(
                        "logo: {} failed, {}",
                        key.as_str(),
                        crate::poller::describe(&error)
                    ),
                    Room::Spare => crate::debug!(
                        "logo: warming {} failed, {}",
                        key.as_str(),
                        crate::poller::describe(&error)
                    ),
                }
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

        self.install(key, room, |slot| slot.copy_from_slice(pixels))
            .await
    }

    /// Find room, hand the pixels to core 1, record the key: everything a fetch
    /// does once it has a crest, whichever arm produced one.
    ///
    /// This is the half of a fetch the module docs' ordering rule is about, so
    /// it is the half that must not be written twice. Note the order it keeps —
    /// claim, fill, send, *then* record — which is why a failure upstream of
    /// here cannot leave a torn slot: nothing has been claimed yet.
    ///
    /// `fill` writes the update's pixels in place rather than the caller
    /// handing over a finished [`LogoSlot`], because a slot is 1,152 B and the
    /// direct arm is already holding a decoded sprite of its own — taking one
    /// by value would put a third copy of the same crest on core 0's stack for
    /// the length of the send, for nothing.
    async fn install(
        &mut self,
        key: CrestKey,
        room: Room,
        fill: impl FnOnce(&mut LogoSlot),
    ) -> Option<LogoRef> {
        let slot = self.claim(room)?;
        let mut update = CrestUpdate {
            slot,
            pixels: [0; LOGO_BYTES],
        };
        fill(&mut update.pixels);
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

    /// A slot to fill: an empty one, else — for [`Room::Evict`] only — the
    /// least recently used that the published snapshot is not drawing from.
    ///
    /// For [`Room::Evict`], `None` is unreachable: at most two slots are held
    /// and there are [`SLOTS`]. Returning it rather than asserting means a
    /// future pool of two would skip the crest instead of panicking on a live
    /// device. For [`Room::Spare`] it is the ordinary "pool is full" answer.
    fn claim(&mut self, room: Room) -> Option<u8> {
        if let Some(empty) = self.keys.iter().position(Option::is_none) {
            return Some(empty as u8);
        }
        if room == Room::Spare {
            return None;
        }
        let held = [self.held.away, self.held.home];
        let victim = self
            .lru
            .iter()
            .copied()
            .find(|slot| !held.contains(&Some(LogoRef(*slot))))?;
        // `LogoPool` logged every eviction, and it is the line that says whether
        // the pool is big enough: one that thrashes evicts a key it is about
        // to want again, which shows up here as the same key cycling. At
        // thirty-two slots this line going quiet for an evening *is* the
        // measurement that the expansion worked.
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

/// Whether a fetch may make room for itself. See the module docs' "two ways
/// in".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Room {
    /// The rotation's fetch: evict the least recently used unheld slot if the
    /// pool is full.
    Evict,
    /// The warmer's: free slots only, or nothing.
    Spare,
}

/// What a warming fetch did, as the poller's warmer reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warm {
    /// The crest is in the pool. It may already have been.
    Cached,
    /// Every slot is taken, so there is nothing this can do — now or later,
    /// until the rotation's own fetches change the picture.
    Full,
    /// The request failed, or the crest does not exist. Nothing was written.
    Failed,
}

impl Default for CrestDirectory {
    fn default() -> CrestDirectory {
        CrestDirectory::new()
    }
}

// ---------------------------------------------------------------------------
// The direct arm: ESPN's CDN, and the PNG decode the backend used to do
// ---------------------------------------------------------------------------

/// The CDN variant a direct fetch asks for, in pixels square.
///
/// A payload's own crest href points at 500 px artwork: 13–40 KB of PNG and
/// 156–209 ms to decode on this silicon. The combiner's 100 px variant is
/// 3–4 KB and ~8.3 ms (PARSE-PERF.md), and both of them are being box-filtered
/// down to 24×24 regardless, so the larger one buys twenty times the decode for
/// detail the panel cannot show. S3-DESIGN decision 5.
#[cfg(feature = "direct")]
const CREST_PIXELS: u16 = 100;

/// Why a direct fetch produced no crest. Every arm is a miss and none of them
/// is an error — the wire path's rule, unchanged.
///
/// Two shapes because the two failures are already described at different
/// layers: a transport failure arrives in the panel's own vocabulary, while a
/// decode failure arrives as `png_stream::Error`, which is `Copy`, `Debug` and
/// nothing else — a `no_std` decoder has no business depending on this
/// firmware's log crate.
#[cfg(feature = "direct")]
enum Miss {
    /// The transfer never completed: DNS, TLS, the socket, the timeout.
    Transport(scoreboard_model::poll::PollError),
    /// The transfer worked and there is still no crest — the CDN has none for
    /// this team, or what it sent is not a PNG this decoder renders.
    NoCrest(&'static str),
}

#[cfg(feature = "direct")]
impl CrestDirectory {
    /// [`CrestDirectory::get`] with the CDN behind it instead of the backend.
    ///
    /// `path` is the crest path the extract carried (or the poller's path
    /// index). `None` — and a path [`scoreboard_direct::crest_url`] cannot
    /// build a URL from — is a miss, not a panic: the games pipeline never read
    /// `team.logo` at all, so a payload that names no artwork must cost exactly
    /// what it costs the backend, which is nothing.
    pub async fn get_direct(
        &mut self,
        league_key: &str,
        abbreviation: &str,
        path: Option<&str>,
        client: &mut crate::net::espn::EspnClient,
        decode: &mut png_stream::Scratch,
    ) -> Option<LogoRef> {
        let key = Self::key(league_key, abbreviation)?;
        if let Some(slot) = self.cached(&key) {
            self.touch(slot);
            defmt::debug!("logo: hit {=str} in slot {}", key.as_str(), slot);
            return Some(LogoRef(slot));
        }
        let url = scoreboard_direct::crest_url(path?, CREST_PIXELS)?;
        self.fetch_png(&url, key, Room::Evict, client, decode).await
    }

    /// [`CrestDirectory::prefetch`] with the CDN behind it, and the same one
    /// difference from [`CrestDirectory::get_direct`] that matters: it never
    /// evicts. Path semantics are `get_direct`'s.
    pub async fn prefetch_direct(
        &mut self,
        league_key: &str,
        abbreviation: &str,
        path: Option<&str>,
        client: &mut crate::net::espn::EspnClient,
        decode: &mut png_stream::Scratch,
    ) -> Warm {
        let Some(key) = Self::key(league_key, abbreviation) else {
            return Warm::Failed;
        };
        if self.cached(&key).is_some() {
            return Warm::Cached;
        }
        if self.keys.iter().all(Option::is_some) {
            return Warm::Full;
        }
        let Some(url) = path.and_then(|path| scoreboard_direct::crest_url(path, CREST_PIXELS))
        else {
            return Warm::Failed;
        };
        match self.fetch_png(&url, key, Room::Spare, client, decode).await {
            Some(_) => Warm::Cached,
            None => Warm::Failed,
        }
    }

    /// The direct arm's [`CrestDirectory::fetch`]: stream, decode, install.
    ///
    /// The decode happens in full before anything is claimed, so the wire
    /// path's guarantee survives verbatim — a crest that fails to arrive, or
    /// fails to decode, writes nothing and cannot leave a torn slot behind it.
    async fn fetch_png(
        &mut self,
        url: &str,
        key: CrestKey,
        room: Room,
        client: &mut crate::net::espn::EspnClient,
        decode: &mut png_stream::Scratch,
    ) -> Option<LogoRef> {
        let sprite = match decode_crest(url, client, decode).await {
            Ok(sprite) => sprite,
            Err(miss) => {
                // The wire arm's two lines, in its words and at its levels: the
                // warmer's failures are not worth an ERROR line, because
                // nothing is waiting on them and it will try again. The two
                // reasons stay apart rather than collapsing into one string,
                // since only one of them has the panel's vocabulary.
                match (room, miss) {
                    (Room::Evict, Miss::Transport(error)) => crate::error!(
                        "logo: {} failed, {}",
                        key.as_str(),
                        crate::poller::describe(&error)
                    ),
                    (Room::Spare, Miss::Transport(error)) => crate::debug!(
                        "logo: warming {} failed, {}",
                        key.as_str(),
                        crate::poller::describe(&error)
                    ),
                    (Room::Evict, Miss::NoCrest(why)) => {
                        crate::error!("logo: {} failed, {}", key.as_str(), why)
                    }
                    (Room::Spare, Miss::NoCrest(why)) => {
                        crate::debug!("logo: warming {} failed, {}", key.as_str(), why)
                    }
                }
                return None;
            }
        };
        self.install(key, room, |slot| pack(&sprite, slot)).await
    }
}

/// One CDN crest, from URL to sprite, with the PNG never existing in one piece.
///
/// [`EspnClient::fetch`](crate::net::espn::EspnClient::fetch) hands the body
/// over in TLS-record-sized slices and each one goes straight into the decoder;
/// what outlives this call is a 24×24 sprite and nothing else — which is the
/// only shape available anyway, since a 500 px crest is 40 KB and this device
/// has no 40 KB to spare for one.
///
/// A decode error returns `false` from the sink, which aborts the transfer at
/// the socket rather than paying for the rest of a body that cannot be used.
/// The reason is stashed on the way past because it does not survive the trip:
/// by the time an abort reaches the caller, the transport has quite correctly
/// turned it into an ordinary `Io` failure, and "malformed png" is the more
/// useful of the two things to print.
#[cfg(feature = "direct")]
async fn decode_crest(
    url: &str,
    client: &mut crate::net::espn::EspnClient,
    decode: &mut png_stream::Scratch,
) -> Result<png_stream::Sprite, Miss> {
    use crate::net::espn::Fetched;

    let mut decoder = png_stream::SpriteDecoder::new(decode);
    let mut broken = None;
    let fetched = {
        let mut sink = |chunk: &[u8]| match decoder.write(chunk) {
            Ok(()) => true,
            Err(error) => {
                broken = Some(error);
                false
            }
        };
        client.fetch(url, &mut sink).await
    };

    // Read before the transport's own answer, for the reason the doc gives: an
    // abort surfaces there as a transport failure and this is what caused it.
    if let Some(error) = broken {
        return Err(Miss::NoCrest(decode_reason(error)));
    }
    match fetched {
        Ok(Fetched::Complete) => {}
        // A team with no artwork on the CDN. Ordinary — the same non-200 the
        // wire arm turns into a miss, and for the same reason: a league whose
        // crest 404s must not count against the poll's failure streak.
        Ok(Fetched::NotFound) => return Err(Miss::NoCrest("no crest on the cdn")),
        Err(error) => return Err(Miss::Transport(error)),
    }
    // Black because the panel has no alpha and the crest is drawn on it — the
    // same background `logo_url` asks the backend for, resolved here instead.
    decoder
        .finish(png_stream::Rgb8::new(0, 0, 0))
        .map_err(|error| Miss::NoCrest(decode_reason(error)))
}

/// `png_stream::Error` in the ring log's words.
///
/// The mapping lives at the one place that prints it rather than in the
/// decoder, which derives `Debug` and stops there — see [`Miss`].
#[cfg(feature = "direct")]
fn decode_reason(error: png_stream::Error) -> &'static str {
    match error {
        png_stream::Error::Signature => "not a png",
        png_stream::Error::Malformed => "malformed png",
        png_stream::Error::Unsupported => "unsupported png",
        png_stream::Error::TooLarge => "png too large",
        png_stream::Error::Deflate => "corrupt png stream",
        png_stream::Error::Truncated => "truncated png",
    }
}

/// A finished sprite into a pool slot.
///
/// Both sides are 24×24 RGB565, row-major, so the only thing that has to be
/// right is the byte order — and nothing at compile time would catch getting it
/// wrong, because a swapped pair is still a valid color and renders as a
/// plausible, wrong one. Verified by reading all three ends:
///
/// - `crates/png-stream/src/down.rs:113` packs `(r5 << 11) | (g6 << 5) | b5`
///   into a `u16` per pixel, row-major (`Sprite`, `[u16; 576]`).
/// - `crates/scoreboard-render/src/blit.rs:69` reads a slot back with
///   `u16::from_le_bytes([data[row + x * 2], data[row + x * 2 + 1]])` — so the
///   pool's bytes are little-endian pairs at stride 24.
/// - `backend/src/logo.rs:299` — whose bytes the proxy arm copies into these
///   same slots — packs identically and pushes the low byte first, which is
///   what makes the two arms agree rather than merely both be self-consistent.
///
/// Little-endian pairs in row-major order is exactly `to_le_bytes` over the
/// sprite, and that is the whole of the conversion.
#[cfg(feature = "direct")]
fn pack(sprite: &png_stream::Sprite, slot: &mut LogoSlot) {
    for (pixel, bytes) in sprite.iter().zip(slot.chunks_exact_mut(2)) {
        bytes.copy_from_slice(&pixel.to_le_bytes());
    }
}

/// The two sides agree on how many pixels there are. The doc on [`pack`] is
/// what makes them agree on the bytes; this is what would catch a sprite that
/// stopped being 24×24 under one of them.
#[cfg(feature = "direct")]
const _: () = assert!(
    png_stream::SPRITE_PIXELS * 2 == LOGO_BYTES,
    "a decoded sprite and a pool slot must be the same 24x24 image"
);

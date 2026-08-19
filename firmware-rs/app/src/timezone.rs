//! What time it is *here*: the browser-seeded UTC offset schedule.
//!
//! ESPN does not know what timezone a living room is in, and neither does a
//! device with no timezone database and no way to ask. The backend answered it
//! with a GeoIP lookup on the request's source address (`backend/src/clock.rs`,
//! `/time`'s `utc_offset`) — a guess from an IP, and one more reason the
//! MaxMind database has to be deployed somewhere. Phase S removes the backend,
//! so the question needs an answer that does not come from a server.
//!
//! It comes from the browser. The settings SPA runs on a phone or a laptop
//! sitting in the same household as the scoreboard, and its `Date` object
//! carries the full IANA rules for that household's zone. So on every visit the
//! page posts what it knows to `PUT /api/timezone`, and the device keeps it.
//!
//! # A schedule, not an offset
//!
//! Posting the current offset alone would be wrong for half of every year: a
//! device seeded in January displays an hour early from March onward, until
//! somebody happens to open the settings page again. Nobody opens the settings
//! page of a working scoreboard.
//!
//! So the page posts three numbers — the current offset, the instant of the
//! next DST transition, and the offset on the far side of it — all of which it
//! computes by probing `Date` over the coming year. The device stores them and
//! [`Record::offset_minutes_at`] flips at the instant. One visit therefore buys
//! correctness until the transition *after* the next one, which is roughly a
//! year: long enough that sparse visits are not a problem, and short enough
//! that the horizon is worth refreshing whenever the page is open anyway.
//!
//! The record deliberately does **not** rewrite itself when the transition
//! passes. Promoting `next` into `offset` would be a flash write on a timer for
//! a value the pure function already returns correctly, and flash writes on
//! this device stop the panel (see [`crate::storage`]). A device that is never
//! visited again holds the post-transition offset forever, which is right for
//! about six months and an hour wrong after that — the accepted cost of the
//! design, and the reason every page visit reseeds.
//!
//! # Precedence: manual beats the seed, and the seed beats nothing
//!
//! [`Record::manual_minutes`] is the escape hatch for the case the seed cannot
//! serve — a scoreboard gifted to another timezone and configured from the
//! giver's browser, a household that wants the board on a different clock than
//! the phone that set it up. When it is set it wins outright, transition and
//! all; the schedule stays stored underneath it so clearing the override
//! restores the seeded answer without another visit.
//!
//! With neither set the answer is `None`, and that is not `Some(0)`: the
//! display omits a start time entirely rather than show one from the wrong
//! timezone (`scoreboard_model::sports::LocalClock` states the same rule for
//! the same reason). [`offset_seconds_at`] hands that distinction straight
//! through to [`crate::net::timesync::local_clock`], which continues the chain
//! — backend GeoIP next, then nothing — and documents it in full. The rule
//! that decides the whole ordering is *strength of evidence*: a browser in the
//! household knows, a GeoIP lookup on a source address guesses, and a firmware
//! that invents `0` lies.
//!
//! # Why this is not part of the configuration document
//!
//! SPEC §9's argument for the OTA attempt record applies here almost word for
//! word: a different writer on a different cadence. The settings page writes
//! the configuration when a person presses Save; it writes this in the
//! background on every page load, with no person involved. Folding the offset
//! into the configuration would mean a `PUT /api/config` that changed the
//! brightness also rewrote the timezone — and worse, that the SPA's background
//! seed rewrote the wifi password, since one document is one write. They are
//! separate keys because they are separate facts with separate lifecycles.
//!
//! # The pure half lives in `scoreboard_config::timezone`
//!
//! `scoreboard_config::timezone` holds the record, its encoding and the flip,
//! and imports nothing device-shaped — SPEC §2's crate-boundary rule, applied
//! the way `scoreboard_log::breadcrumb` and `scoreboard_ota::attempt` apply
//! it. It sits in the *config* crate because that crate already carries the
//! serde surface for browser-posted documents, and because the app builds
//! only for thumbv8m: a record encoding whose tests cannot run has no tests,
//! and there they run. This module keeps what is genuinely the device's — the
//! storage key, the live cell, and the HTTP seam.

use core::cell::Cell;

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

pub use scoreboard_config::timezone::{Document, Invalid, MAX_BYTES, Record};


/// The running record, read from flash once at boot.
///
/// A `Cell` of a `Copy` record rather than a `RefCell`: reading it is a 20-byte
/// copy under a critical section, which is what makes [`offset_seconds_at`]
/// callable from the poll loop without touching flash. (20 in RAM against 12 on
/// flash — the encoding packs what the enums here spend on discriminants.) Only
/// [`load`] and [`apply`] write it, both from core 0.
static TIMEZONE: Mutex<CriticalSectionRawMutex, Cell<Option<Record>>> = Mutex::new(Cell::new(None));

/// Read the stored record and install it as the running one.
///
/// Call once from `main` **before core 1 starts**, for the reason
/// [`crate::storage::install`] gives: the flash read is free until the render
/// loop is up. There is deliberately no lazy path — a first read from the poll
/// loop would park core 1 to answer a question that is the same every time.
pub fn load() {
    let record = crate::storage::load_timezone();
    match record {
        // Epoch 0 because the clock has not synced yet, which reads the
        // schedule's pre-transition side — the offset this boot starts on.
        Some(record) => match record.offset_minutes_at(0) {
            Some(minutes) => crate::debug!("timezone: offset {} min at boot", minutes as i32),
            // Storable and meaningless: a record whose schedule and override
            // were both cleared. Worth a line, because it looks identical to a
            // never-seeded device from the display's side.
            None => crate::debug!("timezone: a record is stored but holds no offset"),
        },
        None => crate::debug!("timezone: nothing stored; local times stay hidden"),
    }
    TIMEZONE.lock(|slot| slot.set(record));
}

/// The running record, for `GET /api/timezone`.
pub fn stored() -> Option<Record> {
    TIMEZONE.lock(|slot| slot.get())
}

/// The UTC offset in force at `now_epoch_s`, in **seconds**, or `None` if this
/// device has never been told where it is.
///
/// # Minutes on the wire, seconds here
///
/// The browser's unit is minutes (`Date.prototype.getTimezoneOffset`), so the
/// endpoint and the flash record are in minutes and nothing has to round-trip
/// through a conversion the client would have to agree with. The display's unit
/// is seconds (`scoreboard_model::sports::LocalClock::utc_offset_s`, which the
/// backend's `/time` also answers in). This is the one seam between them, and
/// the multiply cannot overflow because the record's offsets are bounded at
/// ±840 minutes on both ingest and decode.
///
/// # Who reads this
///
/// [`crate::net::timesync::local_clock`], which resolves the two rungs above
/// and falls through to the backend's GeoIP answer when this returns `None`.
/// The full chain is documented there; this function owns the first two rungs
/// of it and nothing else.
///
/// It is deliberately **not** gated behind `direct`, because a browser in the
/// same household is better evidence than a GeoIP lookup on the request's
/// source address in *both* worlds — so in a default build the seed simply
/// outranks the backend, and in a `direct` build it is the only answer there
/// is.
///
/// The other caller is `GET /api/timezone`'s `effective_offset_minutes`, which
/// reports what this returns rather than re-deriving it, so that what the
/// display will use is observable from the settings page.
pub fn offset_seconds_at(now_epoch_s: u32) -> Option<i32> {
    let minutes = stored()?.offset_minutes_at(now_epoch_s)?;
    Some(i32::from(minutes) * 60)
}

/// Store `record`. **At most one flash write, and only when something changed.**
///
/// The comparison is against the running copy rather than a fresh read, which
/// is what makes the change check free: the SPA posts on every page load, so
/// the steady state is a `PUT` that matches what is already there, and that
/// case must not cost the panel a frame. The device's own record is the only
/// thing that could disagree with the RAM copy, and nothing writes it but this.
///
/// The flash write happens first and the RAM copy second, so a failed write
/// leaves the device serving what flash actually holds rather than a value it
/// would lose at the next boot.
pub fn apply(record: Record) -> bool {
    if stored() == Some(record) {
        return true;
    }
    if !crate::storage::save_timezone(&record) {
        return false;
    }
    TIMEZONE.lock(|slot| slot.set(Some(record)));
    crate::debug!("timezone: record updated and saved");
    true
}

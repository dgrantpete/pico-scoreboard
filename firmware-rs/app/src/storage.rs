//! The 980 KB of flash the device gets to keep things in.
//!
//! SPEC §9: a `sequential-storage` map over the region
//! [`scoreboard_layout`] reserves at `0x30_B000`. Four keys, and the reasons
//! for each are in [`Key`]. Everything else about persistence — the
//! merge, the defaults, the never-raises promise — belongs to
//! [`scoreboard_config`] and is host-tested there; what is here is the flash.
//!
//! # Writing flash stops both cores. Every caller is written knowing that.
//!
//! On the RP2350 a program or erase runs out of RAM with XIP disabled, so
//! embassy-rp's `Flash` parks core 1 through the multicore FIFO for the whole
//! operation and holds a critical section around it. Core 1 executes the render
//! loop from XIP; the panel therefore **stops** for the duration. That is not a
//! detail to hide behind an `async fn` — a caller that writes flash is a caller
//! that drops frames, and it should look like it.
//!
//! So the API here is **blocking**, and the `async` of `sequential-storage`'s
//! map is driven with [`embassy_futures::block_on`]. That is honest rather than
//! lazy: the futures underneath are
//! [`BlockingAsync`](embassy_embedded_hal::adapter::BlockingAsync), which never
//! return `Pending`, so there is no executor progress being denied and no
//! wakeup being lost. Writing `.await` instead would tell the reader that other
//! tasks run during a config save, and they do not.
//!
//! Measured cost of one `PUT /api/config` batch is in BUDGET.md, alongside the
//! frame the panel loses to it.
//!
//! # One owner, two very different borrowers
//!
//! Phase 4 gave the flash peripheral a second consumer. `sequential-storage`'s
//! map wants the device **by value**; `embassy-boot`'s
//! `FirmwareUpdaterConfig::from_linkerfile_blocking` wants
//! `&Mutex<NoopRawMutex, RefCell<Flash>>` and holds partitions borrowed from it
//! for the life of an update. There is one `FLASH` peripheral and both of them
//! need it, so neither can own it.
//!
//! So this module owns it, once, in exactly the shape embassy-boot demands, and
//! lends it out:
//!
//! * [`with_map`] builds a fresh `MapStorage` around a `&mut` borrow for the
//!   duration of one operation. Free, because the map's only state is its
//!   `Uncached` cache, which has none — the running configuration lives in
//!   [`crate::config`]'s static, not here.
//! * [`flash`] hands the mutex itself to [`crate::ota`], which is the only
//!   caller that needs access to outlive a single call: a download writes DFU
//!   across many `await`s.
//!
//! **The raw mutex is `NoopRawMutex`, and that is embassy-boot's choice rather
//! than ours** — its signature names the type. It is sound here for the reason
//! the old critical-section one was never needed: every caller is on core 0's
//! single executor, no operation awaits with the lock held, and core 1 never
//! touches flash. What a `NoopRawMutex` does not protect against is
//! *re-entrancy* from the same executor, which would double-borrow the
//! `RefCell` and panic — loudly, which is the right outcome for a bug that
//! shape.
//!
//! # Why the map is uncached
//!
//! `sequential_storage::cache::Uncached` means every fetch walks the region's
//! page states. With 245 pages of mostly-erased flash that is a few hundred
//! reads out of the XIP window and it happens twice at boot and never again —
//! the running configuration lives in [`crate::config`]'s static, not here.
//! Paying RAM for a cache to speed up an operation that runs twice per boot is
//! the wrong trade.

use core::cell::{Cell, RefCell};
use core::ops::Range;

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::block_on;
use embassy_rp::Peri;
use embassy_rp::flash::{Blocking, Flash as RpFlash};
use embassy_rp::peripherals::FLASH;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use scoreboard_config::{DeviceConfig, LoadComplaint};
use scoreboard_log::breadcrumb::{self, Breadcrumb};
use scoreboard_ota::Attempt;
use sequential_storage::cache::{Cache, Uncached};
use sequential_storage::map::{MapConfig, MapStorage};
use static_cell::StaticCell;

/// The whole 4 MB part. embassy-rp wants the device size as a const parameter
/// so it can bounds-check every offset against it.
const FLASH_BYTES: usize = scoreboard_layout::FLASH_SIZE as usize;

/// The flash driver itself.
///
/// Public because [`crate::ota`] has to name it: `BlockingFirmwareUpdater` is
/// generic over its two partitions and the concrete type has to be written out
/// somewhere.
pub type Device = RpFlash<'static, FLASH, Blocking, FLASH_BYTES>;

/// The shape `embassy-boot` requires, and therefore the shape the one handle
/// takes. See the module docs on `NoopRawMutex`.
pub type FlashCell = Mutex<NoopRawMutex, RefCell<Device>>;

type NoCache = Cache<Uncached, Uncached, Uncached, u8>;
type Map<'a> = MapStorage<u8, BlockingAsync<&'a mut Device>, NoCache>;
type MapError = sequential_storage::Error<embassy_rp::flash::Error>;

/// The scratch every map operation needs: long enough for the largest key and
/// value together.
///
/// The largest value is the configuration document — about 1.3 KB with every
/// league slot full, which is the same figure `http::scratch` is sized against
/// and for the same reason. 3 KB leaves the configuration room to grow a
/// section without this becoming the thing that breaks.
///
/// It is a **stack** buffer, allocated inside each function here, and that is
/// deliberate: these are plain `fn`s called from async handlers, so the bytes
/// live on core 0's stack for the length of one call rather than inside a
/// handler's future, where picoserve's nested-router generics would instantiate
/// them once per layer. `http::scratch`'s module docs measured that at 22×.
const BUFFER_BYTES: usize = 3 * 1024;

/// What is stored, and what deliberately is not.
///
/// SPEC §9 lists four things: wifi credentials, device config, the OTA
/// channel/dev flag, and sticky user preferences. Two of them turned out not to
/// be separate records, and one is not this phase's:
///
/// - **Wifi credentials are the config's `network` section.** `config.json`
///   held them, `GET /api/config` returns them, and the setup page writes them
///   through the same `PUT` as everything else. A second record would be a
///   second source of truth for the same four fields, and `reset-network` would
///   have to clear both.
/// - **There are no sticky preferences.** The rotation lock and the league
///   filter are the only two user-set things outside the config, and both are
///   deliberately *session* state — `menu.py` says so in as many words ("resets
///   to all-checked on reboot"), and the lock follows it. Brightness is
///   `display.brightness`, which is config.
/// - **The OTA dev flag is a config field, as SPEC §8 said.** `ota.enabled` and
///   `ota.channel` are things a person sets, so they live in the configuration
///   document and `GET /api/config` returns them.
///
/// # The third key, and why it is not config
///
/// Phase 4 added one: [`Key::OtaAttempt`]. It is the record that stops a bad
/// image being downloaded, swapped, reverted and downloaded again forever
/// ([`scoreboard_ota::attempt`] carries the whole argument). It is deliberately
/// **not** part of the configuration document, for three reasons that all point
/// the same way: nothing outside the OTA client writes it, `GET /api/config`
/// has no business returning it and a `PUT` that reset it would re-arm the loop
/// it exists to break, and it changes on a different clock — once per update
/// attempt rather than once per settings save. Folding it in would rewrite the
/// whole document, wifi password and all, every time an update started.
///
/// # The fourth key, and why it is not config either
///
/// Phase S added [`Key::Timezone`] — the browser-seeded UTC offset schedule
/// ([`crate::timezone`]). The OTA argument above transfers almost unchanged: a
/// different writer on a different cadence. The settings page writes the
/// configuration when a person presses Save; it writes this in the background
/// on every page load, with no person involved. One document is one write, so
/// folding the offset in would mean the SPA's background seed rewriting the
/// wifi password — and a `PUT /api/config` that changed the brightness
/// discarding the timezone. They are separate facts with separate lifecycles,
/// which is the whole of SPEC §9's test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Key {
    /// The `DeviceConfig` document, as the JSON `GET /api/config` serves.
    ///
    /// JSON rather than a packed struct so that the forward-compatibility
    /// `DeviceConfig`'s serde defaults already provide extends to *storage*: a
    /// document written by a firmware that had one fewer key reads back with
    /// that key's default, which is the property `config.py`'s deep merge
    /// existed for. A packed struct would make every added field a migration.
    Config = 1,
    /// The last abnormal shutdown. See [`scoreboard_log::breadcrumb`].
    Breadcrumb = 2,
    /// What the OTA client last tried to install, and how it went. See
    /// [`scoreboard_ota::attempt`].
    OtaAttempt = 3,
    /// The UTC offset schedule the settings page seeded. See
    /// [`crate::timezone`].
    Timezone = 4,
}

/// The flash handle's storage. Written once, by [`install`].
static FLASH_CELL: StaticCell<FlashCell> = StaticCell::new();

/// A shared reference to the one handle, in a wrapper that can live in a
/// `static`.
///
/// `NoopRawMutex` is deliberately `!Sync` — asserting single-threaded access is
/// its entire job — so `&'static FlashCell` cannot cross into a `static` on its
/// own. The refusal is correct in general and wrong here, and this is the
/// narrow claim that says why.
#[derive(Clone, Copy)]
struct FlashRef(&'static FlashCell);

// SAFETY: the claim is that the `FlashCell` behind this reference is only ever
// reached from core 0. Three things hold it up. `install` runs in `main` before
// `spawn_core1`, so the write happens before a second core exists. Every reader
// goes through [`flash`] or [`with_map`], and every caller of those is on core
// 0's single executor — the poll loop, the HTTP handlers, the boot sequence.
// And core 1 physically cannot use it even by mistake: embassy-rp refuses a
// flash write from the non-owning core with `Error::InvalidCore`, which
// `crate::supervise`'s breadcrumb design already depends on.
unsafe impl Send for FlashRef {}
unsafe impl Sync for FlashRef {}

/// The handle, once there is one.
///
/// A `Cell` of a shared reference rather than the peripheral itself: what
/// [`crate::ota`] needs is a `&'static FlashCell` it can keep across the awaits
/// of a download, and what [`with_map`] needs is a borrow for one call. Both
/// come from here, and the critical section covers copying a pointer.
static FLASH: Mutex<CriticalSectionRawMutex, Cell<Option<FlashRef>>> = Mutex::new(Cell::new(None));

/// The region, as a byte range within the flash device.
///
/// Offsets, not XIP addresses: embassy-rp's `Flash` addresses flash from zero.
/// Getting this wrong writes into the DFU partition, which is why it comes from
/// the layout crate rather than from a literal.
const fn region() -> Range<u32> {
    scoreboard_layout::STORAGE_OFFSET
        ..scoreboard_layout::STORAGE_OFFSET + scoreboard_layout::STORAGE_SIZE
}

/// Hand the flash peripheral over. Call once, from `main`, before core 1 starts.
///
/// Before core 1 starts is not a suggestion. `Flash`'s operations park core 1
/// through the multicore FIFO and wait for it to answer; with core 1 not yet
/// running, `pause_core1` is a no-op, so the boot-time reads and the breadcrumb
/// promotion cost nothing at all. Once the render loop is up, the same calls
/// cost the panel a frame.
pub fn install(flash: Peri<'static, FLASH>) {
    let cell = FLASH_CELL.init(Mutex::new(RefCell::new(RpFlash::new_blocking(flash))));
    FLASH.lock(|slot| slot.set(Some(FlashRef(cell))));
}

/// The one flash handle, for the one caller whose access outlives a single
/// call.
///
/// [`crate::ota`] builds a `BlockingFirmwareUpdater` on this and holds it across
/// the `await`s of a download. That is safe *because* the updater does not hold
/// the lock — its `BlockingPartition`s take it per read or write — so a
/// configuration save landing between two chunks serialises against the chunk
/// rather than against the whole update.
pub fn flash() -> Option<&'static FlashCell> {
    FLASH.lock(|slot| slot.get()).map(|handle| handle.0)
}

/// Run one operation against the map. `None` means storage is unavailable.
///
/// The `MapStorage` is built per call and dropped at the end of it. That is not
/// wasteful: its only state is the `Uncached` cache, which has none, so
/// construction is moving three fields. The alternative — keeping the map alive
/// — would mean it owning the flash device, which is exactly what [`flash`]'s
/// caller also needs.
fn with_map<R>(operation: impl FnOnce(&mut Map<'_>) -> R) -> Option<R> {
    let cell = flash()?;
    Some(cell.lock(|device| {
        let mut borrowed = device.borrow_mut();
        let mut map = MapStorage::new(
            BlockingAsync::new(&mut *borrowed),
            MapConfig::new(region()),
            NoCache::new_uncached(),
        );
        operation(&mut map)
    }))
}

// ---------------------------------------------------------------------------
// The configuration
// ---------------------------------------------------------------------------

/// What the boot read out of flash.
pub struct Stored {
    pub config: DeviceConfig,
    /// What was wrong with the stored document, if anything. `config.py:_load`
    /// logged exactly this and carried on.
    pub complaint: Option<LoadComplaint>,
    /// Whether there was a document at all. **This is the dev-seam switch** —
    /// see [`crate::config::load`].
    pub present: bool,
}

/// Read the stored configuration.
///
/// Never fails in a way a caller has to handle: unreadable storage and an
/// unparseable document both come back as defaults with a complaint, which is
/// `config.py`'s promise that a corrupt file cannot brick a boot.
pub fn load_config(keep_alive: &mut impl FnMut()) -> Stored {
    let mut buffer = [0u8; BUFFER_BYTES];
    let fetched = with_map(|map| {
        block_on(map.fetch_item::<&[u8]>(&mut buffer, &(Key::Config as u8)))
    });
    match fetched {
        Some(Ok(Some(document))) => {
            let (config, complaint) = DeviceConfig::from_json(document);
            Stored {
                config,
                complaint,
                present: true,
            }
        }
        // No document: a device on its first Rust boot, which SPEC §9 says
        // takes the unprovisioned path. MicroPython's littlefs is not read and
        // never will be — a migrated unit is reprovisioned once, deliberately.
        Some(Ok(None)) => Stored {
            config: DeviceConfig::new(),
            complaint: None,
            present: false,
        },
        Some(Err(error)) => {
            defmt::error!("storage: config read failed: {}", error);
            // A region that does not read as a map at all is not a config
            // problem, it is a *region* problem, and it has one honest cause on
            // this device: those 980 KB have held other things. SPEC §9 says a
            // migrated unit finds MicroPython's littlefs there, and the Phase 4
            // boot spike used the region's first sector as a command mailbox.
            // Leaving it means a device that can never save its configuration
            // and never says why, so it is erased once and the boot continues
            // down the unprovisioned path — which is exactly what SPEC §9 says
            // a first Rust boot does.
            reset_region(keep_alive);
            Stored {
                config: DeviceConfig::new(),
                complaint: Some(LoadComplaint::Unparseable),
                present: false,
            }
        }
        None => {
            defmt::error!("storage: config read before install");
            Stored {
                config: DeviceConfig::new(),
                complaint: None,
                present: false,
            }
        }
    }
}

/// Write the configuration. **One flash write per call, and one call per
/// `PUT`.**
///
/// `config.py`'s `update_many` was emphatic about this: the settings page sends
/// a whole section per save, and doing a write per key would multiply the wear
/// on the part that wears out by the number of fields in the form. The batching
/// is [`crate::http::routes::put_config`]'s — it merges everything first and
/// calls this once — and this is the other half of that contract.
pub fn save_config(config: &DeviceConfig) -> bool {
    let mut buffer = [0u8; BUFFER_BYTES];
    let Ok(length) = config.to_json(&mut buffer) else {
        defmt::error!(
            "storage: the configuration did not fit {} B, not saved",
            BUFFER_BYTES as u32
        );
        return false;
    };
    // Split so the document and the map's scratch are disjoint slices: the
    // value is borrowed for the length of the call, and `store_item` writes
    // headers into the buffer it is given.
    let (document, scratch) = buffer.split_at_mut(length);
    let document: &[u8] = document;
    let stored = with_map(|map| {
        block_on(map.store_item(scratch, &(Key::Config as u8), &document))
    });
    match stored {
        Some(Ok(())) => {
            crate::debug!("storage: configuration saved, {} B", length);
            true
        }
        Some(Err(error)) => {
            crate::error!("storage: configuration save failed: {}", Complaint::of(&error));
            false
        }
        None => {
            crate::error!("storage: configuration save before install");
            false
        }
    }
}

/// Erase the whole region back to a usable empty map.
///
/// Only from the unreadable-region path above, and only ever once per device:
/// 245 sector erases at roughly 30 ms each is a boot that takes about seven
/// seconds longer, which is a fine price for the alternative being a device
/// that silently cannot persist anything. It runs before core 1 starts, so it
/// costs no frames.
///
/// # Why this is a loop and not `MapStorage::erase_all`
///
/// Seven seconds is *just* inside the 8 s watchdog `firmware-rs/boot` arms
/// before jumping here, and `erase_all` is a single call with nothing to hook.
/// A device whose flash erased a little slower than the estimate — a cold part,
/// a worn one — would reset partway through, come back, find the region still
/// unreadable, and start again: a boot loop caused entirely by the recovery
/// path. So the loop is ours and `keep_alive` runs between sectors, which turns
/// a total time nobody has measured into a per-sector time with 8 s of room.
fn reset_region(keep_alive: &mut impl FnMut()) {
    let Some(cell) = flash() else { return };
    defmt::warn!("storage: erasing the storage region; this takes a few seconds");
    let region = region();
    let mut failures = 0u32;
    for sector in region.step_by(scoreboard_layout::ERASE_SIZE as usize) {
        keep_alive();
        let erased = cell.lock(|device| {
            device
                .borrow_mut()
                .blocking_erase(sector, sector + scoreboard_layout::ERASE_SIZE)
        });
        if erased.is_err() {
            failures += 1;
        }
    }
    if failures == 0 {
        defmt::info!("storage: region erased and usable");
    } else {
        // Not fatal on its own — sequential-storage tolerates pages it cannot
        // read — but it is the signal that the part itself is going, which is
        // otherwise invisible until a save silently stops working.
        defmt::error!("storage: {} sectors failed to erase", failures);
    }
}

// ---------------------------------------------------------------------------
// The breadcrumb
// ---------------------------------------------------------------------------

/// Read the stored breadcrumb, if there is one and it is readable.
pub fn load_breadcrumb() -> Option<Breadcrumb> {
    let mut buffer = [0u8; BUFFER_BYTES];
    let fetched = with_map(|map| {
        block_on(map.fetch_item::<&[u8]>(&mut buffer, &(Key::Breadcrumb as u8)))
    })?;
    let record = match fetched {
        Ok(Some(record)) => record,
        Ok(None) => return None,
        Err(error) => {
            defmt::error!("storage: breadcrumb read failed: {}", error);
            return None;
        }
    };
    match Breadcrumb::decode(record) {
        Ok(crumb) => Some(crumb),
        Err(error) => {
            // Worth a line rather than a silent `None`: a record that is there
            // but unreadable means a firmware version changed under it, and
            // that is a different fact from "nothing has ever crashed".
            defmt::warn!(
                "storage: stored breadcrumb is unreadable ({=str})",
                error.as_str()
            );
            None
        }
    }
}

/// Write the breadcrumb, replacing whatever was there.
///
/// Called once at boot, from the promotion path — never from a panic handler.
/// See [`crate::supervise`] for why the panic handler writes RAM instead.
pub fn save_breadcrumb(crumb: &Breadcrumb) -> bool {
    let mut buffer = [0u8; BUFFER_BYTES];
    let mut record = [0u8; breadcrumb::MAX_BYTES];
    let Ok(length) = crumb.encode(&mut record) else {
        defmt::error!("storage: breadcrumb did not encode");
        return false;
    };
    let value: &[u8] = &record[..length];
    let stored = with_map(|map| {
        block_on(map.store_item(&mut buffer, &(Key::Breadcrumb as u8), &value))
    });
    match stored {
        Some(Ok(())) => true,
        Some(Err(error)) => {
            defmt::error!("storage: breadcrumb save failed: {}", error);
            false
        }
        None => false,
    }
}

/// A map error, in words the ring log can carry.
///
/// `sequential_storage::Error` is `defmt::Format` but not `core::fmt::Display`,
/// and [`crate::error!`] writes to both channels — so the interesting variants
/// get names here and everything else gets one.
struct Complaint(&'static str);

impl Complaint {
    fn of(error: &MapError) -> Complaint {
        Complaint(match error {
            MapError::Storage { .. } => "flash i/o failed",
            MapError::FullStorage => "storage region full",
            MapError::Corrupted { .. } => "storage corrupted",
            MapError::BufferTooBig => "value too large for the buffer",
            MapError::BufferTooSmall(_) => "buffer too small",
            MapError::SerializationError(_) => "value did not encode",
            _ => "storage error",
        })
    }
}

impl defmt::Format for Complaint {
    fn format(&self, formatter: defmt::Formatter<'_>) {
        defmt::write!(formatter, "{=str}", self.0);
    }
}

impl core::fmt::Display for Complaint {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.0)
    }
}

// ---------------------------------------------------------------------------
// The OTA attempt record
// ---------------------------------------------------------------------------

/// What the OTA client last tried to install.
///
/// `None` covers three cases the caller does not need to tell apart — nothing
/// has ever been attempted, the record is from a firmware whose encoding
/// changed, or the read failed — because all three mean the same thing to
/// [`scoreboard_ota::decide`]: no version is blocked.
pub fn load_ota_attempt() -> Option<Attempt> {
    let mut buffer = [0u8; BUFFER_BYTES];
    let fetched = with_map(|map| {
        block_on(map.fetch_item::<&[u8]>(&mut buffer, &(Key::OtaAttempt as u8)))
    })?;
    match fetched {
        Ok(Some(record)) => Attempt::decode(record),
        Ok(None) => None,
        Err(error) => {
            defmt::error!("storage: ota record read failed: {}", error);
            None
        }
    }
}

/// Write the OTA attempt record, replacing whatever was there.
///
/// One write per *update attempt* — not per chunk, not per boot — so the wear
/// this adds is a handful of writes a year on a device that already writes the
/// configuration once per settings save.
///
/// It happens before the download starts rather than after it finishes, and the
/// ordering is the whole point: a device that is reset mid-download must come
/// back having *already counted* the attempt, or the count never reaches its
/// limit and the loop the record exists to break runs forever.
#[cfg_attr(
    not(feature = "link-boot-integrated"),
    allow(dead_code, reason = "reached only through the OTA install path, which needs a bootloader")
)]
pub fn save_ota_attempt(record: &Attempt) -> bool {
    let mut buffer = [0u8; BUFFER_BYTES];
    let mut encoded = [0u8; scoreboard_ota::attempt::MAX_BYTES];
    let Ok(length) = record.encode(&mut encoded) else {
        defmt::error!("storage: ota record did not encode");
        return false;
    };
    let value: &[u8] = &encoded[..length];
    let stored = with_map(|map| {
        block_on(map.store_item(&mut buffer, &(Key::OtaAttempt as u8), &value))
    });
    match stored {
        Some(Ok(())) => true,
        Some(Err(error)) => {
            crate::error!("storage: ota record save failed: {}", Complaint::of(&error));
            false
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// The timezone record
// ---------------------------------------------------------------------------

/// Read the stored UTC offset schedule.
///
/// `None` covers the three cases [`crate::timezone`] treats alike — nothing has
/// ever been seeded, the record is from a firmware whose encoding changed, or
/// the read failed — because all three mean the display has no timezone to show
/// a local time in, which is a state it already handles.
pub fn load_timezone() -> Option<crate::timezone::Record> {
    let mut buffer = [0u8; BUFFER_BYTES];
    let fetched = with_map(|map| {
        block_on(map.fetch_item::<&[u8]>(&mut buffer, &(Key::Timezone as u8)))
    })?;
    match fetched {
        Ok(Some(record)) => crate::timezone::Record::decode(record),
        Ok(None) => None,
        Err(error) => {
            defmt::error!("storage: timezone read failed: {}", error);
            None
        }
    }
}

/// Write the UTC offset schedule, replacing whatever was there.
///
/// One write per *change*, not per `PUT`: the settings page posts on every
/// visit and [`crate::timezone::apply`] compares before it calls this, so a
/// page reload costs no flash and no dropped frame. That check is the caller's
/// and not this function's for the same reason `save_config`'s batching is
/// `put_config`'s — the module that knows what changed is the module that owns
/// the running copy.
pub fn save_timezone(record: &crate::timezone::Record) -> bool {
    let mut buffer = [0u8; BUFFER_BYTES];
    let mut encoded = [0u8; crate::timezone::MAX_BYTES];
    record.encode(&mut encoded);
    let value: &[u8] = &encoded;
    let stored = with_map(|map| {
        block_on(map.store_item(&mut buffer, &(Key::Timezone as u8), &value))
    });
    match stored {
        Some(Ok(())) => true,
        Some(Err(error)) => {
            crate::error!("storage: timezone save failed: {}", Complaint::of(&error));
            false
        }
        None => {
            crate::error!("storage: timezone save before install");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// The device's name in a log line
// ---------------------------------------------------------------------------

/// This board's unique id, as lowercase hex.
///
/// The Rust fleet's `X-Device-Id`, and the counterpart of `ota.py`'s
/// `machine.unique_id()` header. Nothing routes on it; it is what makes
/// `fly logs` a fleet dashboard rather than a stream of anonymous requests, and
/// it is the only way to tell two units apart when one of them is misbehaving
/// in somebody else's living room.
///
/// # Not the flash id
///
/// `ota.py` used `machine.unique_id()`, which on the RP2040 is the QSPI part's
/// id — and embassy-rp's equivalent, `Flash::blocking_unique_id`, is
/// `#[cfg(feature = "rp2040")]` and does not exist on this chip. The RP2350
/// carries its own id in OTP instead, which is strictly better for the purpose:
/// it identifies the *board*, so replacing the flash chip would not rename the
/// device, and reading it costs no flash access at all.
static DEVICE_ID: Mutex<CriticalSectionRawMutex, Cell<Option<&'static str>>> =
    Mutex::new(Cell::new(None));

/// Read and cache the chip id. Call once from `main`.
pub fn read_device_id() {
    static TEXT: StaticCell<heapless::String<16>> = StaticCell::new();

    let Ok(chipid) = embassy_rp::otp::get_chipid() else {
        defmt::warn!("storage: the chip id did not read; requests will be anonymous");
        return;
    };
    let mut text = heapless::String::new();
    if core::fmt::Write::write_fmt(&mut text, format_args!("{chipid:016x}")).is_err() {
        return;
    }
    let id: &'static str = TEXT.init(text).as_str();
    defmt::info!("device id {=str}", id);
    DEVICE_ID.lock(|slot| slot.set(Some(id)));
}

/// The cached id, or `"unknown"` before [`read_device_id`] or if it failed.
pub fn device_id() -> &'static str {
    DEVICE_ID.lock(|slot| slot.get()).unwrap_or("unknown")
}

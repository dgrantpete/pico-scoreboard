//! The 980 KB of flash the device gets to keep things in.
//!
//! SPEC §9: a `sequential-storage` map over the region
//! [`scoreboard_layout`] reserves at `0x30_B000`. Two keys, and the reasons for
//! there being only two are in [`Key`]. Everything else about persistence — the
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
//! # One owner, taken and returned
//!
//! [`with_storage`] lifts the whole `MapStorage` out of its cell, runs the
//! operation, and puts it back — so the critical section covers a pointer move
//! rather than the flash access. Re-entering while it is out answers `None`,
//! which is structurally unreachable (every caller is on core 0's single
//! executor and none of them awaits mid-operation) and is handled anyway,
//! because "the config did not save" is a thing to log rather than a thing to
//! panic about.
//!
//! # Why the map is uncached
//!
//! `sequential_storage::cache::Uncached` means every fetch walks the region's
//! page states. With 245 pages of mostly-erased flash that is a few hundred
//! reads out of the XIP window and it happens twice at boot and never again —
//! the running configuration lives in [`crate::config`]'s static, not here.
//! Paying RAM for a cache to speed up an operation that runs twice per boot is
//! the wrong trade.

use core::ops::Range;

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::block_on;
use embassy_rp::Peri;
use embassy_rp::flash::{Blocking, Flash as RpFlash};
use embassy_rp::peripherals::FLASH;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use scoreboard_config::{DeviceConfig, LoadComplaint};
use scoreboard_log::breadcrumb::{self, Breadcrumb};
use sequential_storage::cache::{Cache, Uncached};
use sequential_storage::map::{MapConfig, MapStorage};

/// The whole 4 MB part. embassy-rp wants the device size as a const parameter
/// so it can bounds-check every offset against it.
const FLASH_BYTES: usize = scoreboard_layout::FLASH_SIZE as usize;

type Device = BlockingAsync<RpFlash<'static, FLASH, Blocking, FLASH_BYTES>>;
type NoCache = Cache<Uncached, Uncached, Uncached, u8>;
type Map = MapStorage<u8, Device, NoCache>;
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
/// - **The OTA dev flag is Phase 4's, and is a config field there.** SPEC §8
///   retires `/ota_dev` in favour of "a config flag that pins the device to the
///   staging manifest channel". Adding a key here for a value nothing writes
///   would be a guess at Phase 4's shape recorded in flash format.
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
}

/// The map, or `None` before [`install`] and while an operation is in flight.
static STORAGE: Mutex<CriticalSectionRawMutex, core::cell::RefCell<Option<Map>>> =
    Mutex::new(core::cell::RefCell::new(None));

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
    let device = BlockingAsync::new(RpFlash::new_blocking(flash));
    let map = MapStorage::new(device, MapConfig::new(region()), NoCache::new_uncached());
    STORAGE.lock(|slot| *slot.borrow_mut() = Some(map));
}

/// Run one operation against the map. `None` means storage is unavailable.
fn with_storage<R>(operation: impl FnOnce(&mut Map) -> R) -> Option<R> {
    let mut map = STORAGE.lock(|slot| slot.borrow_mut().take())?;
    let result = operation(&mut map);
    STORAGE.lock(|slot| *slot.borrow_mut() = Some(map));
    Some(result)
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
pub fn load_config() -> Stored {
    let mut buffer = [0u8; BUFFER_BYTES];
    let fetched = with_storage(|map| {
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
            reset_region();
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
    let stored = with_storage(|map| {
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
fn reset_region() {
    defmt::warn!("storage: erasing the storage region; this takes a few seconds");
    let erased = with_storage(|map| block_on(map.erase_all()));
    match erased {
        Some(Ok(())) => defmt::info!("storage: region erased and usable"),
        Some(Err(error)) => defmt::error!("storage: region erase failed: {}", error),
        None => {}
    }
}

// ---------------------------------------------------------------------------
// The breadcrumb
// ---------------------------------------------------------------------------

/// Read the stored breadcrumb, if there is one and it is readable.
pub fn load_breadcrumb() -> Option<Breadcrumb> {
    let mut buffer = [0u8; BUFFER_BYTES];
    let fetched = with_storage(|map| {
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
    let stored = with_storage(|map| {
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

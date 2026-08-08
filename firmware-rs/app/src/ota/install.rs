//! Downloading, verifying, arming — and what the bootloader said about the
//! last time.
//!
//! Everything in here needs the `__bootloader_*` symbols, so it exists twice:
//! once for real under `link-boot-integrated`, and once as a refusal under
//! `link-standalone`. The refusal is not a stub in the apologetic sense — a
//! probe-flashed image at flash offset 0 has no bootloader, no DFU partition
//! and nothing to swap with, and "this build cannot install updates" is the
//! true and complete answer. Keeping the split *here*, behind one function
//! signature, is what stops `#[cfg]` leaking into the poll loop, the HTTP
//! surface and the supervision.

use scoreboard_ota::{Channel, Manifest, manifest};

use crate::net::api_client::{ApiClient, ResponseBuffer};

/// Fetch and parse `/fw/manifest`.
///
/// Shared by both profiles: a standalone build can still *check*, and answering
/// "current" or "an update exists but this build cannot take it" is more use
/// than refusing to look.
pub async fn fetch_manifest(
    client: &mut ApiClient,
    buffer: &mut ResponseBuffer,
    base_url: &str,
    api_key: &str,
    channel: Channel,
) -> Result<Manifest, &'static str> {
    let url = crate::net::api_client::url(
        base_url,
        format_args!("/fw/manifest?channel={}", channel.as_str()),
    )
    .map_err(|_| "the configured backend url does not fit")?;

    let fetched = client
        .fw_get(&url, api_key, "check", &mut buffer[..])
        .await
        .map_err(|_| "the backend did not answer")?;
    if fetched.status != 200 {
        // 404 is the ordinary "nothing published on this channel yet", and it
        // is worth its own words: on a fresh staging backend it is the answer
        // for as long as nobody has run publish-fw.
        return Err(if fetched.status == 404 {
            "no image is published on this channel"
        } else {
            "the backend refused the manifest request"
        });
    }
    manifest::parse(fetched.body, channel, scoreboard_layout::ACTIVE_SIZE)
        .map_err(scoreboard_ota::ManifestError::as_str)
}

#[cfg(feature = "link-boot-integrated")]
mod integrated {
    use embassy_boot_rp::{
        AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig, State as BootState,
    };
    use embassy_embedded_hal::flash::partition::BlockingPartition;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embassy_time::{Duration, Instant, Timer};
    use scoreboard_model::{Publisher, Store};
    use scoreboard_ota::{Attempt, Channel, Manifest, Progress};
    use sha2::Sha512;

    use crate::net::api_client::{ApiClient, ResponseBuffer};
    use crate::ota::{
        Answer, COUNTDOWN_SECONDS, DOWNLOAD_TIMEOUT, PUBLIC_KEY, State, VERSION, set_state,
    };

    /// A window onto the one flash handle. Both partitions are the same type
    /// because they are the same device with different bounds.
    type Partition = BlockingPartition<'static, NoopRawMutex, crate::storage::Device>;
    type Updater<'a> = BlockingFirmwareUpdater<'a, Partition, Partition>;

    /// The state partition's "word" is one byte: embassy-rp's flash driver
    /// exposes `WRITE_SIZE = 1` because it page-buffers internally, and the
    /// layout crate's const asserts are stated in the same units.
    const STATE_WORD: usize = 1;

    /// Run one short operation against the updater.
    ///
    /// Built per call rather than kept. It has no state worth preserving — the
    /// swap-progress bookkeeping lives in the state *partition*, not in the
    /// struct — and building it per call is what lets [`crate::storage`] stay
    /// the single owner of the peripheral.
    ///
    /// The download does **not** go through this: it needs the updater alive
    /// across `await`s, and a closure would mean `block_on`. See `download`.
    fn with_updater<R>(run: impl FnOnce(&mut Updater<'_>) -> R) -> Option<R> {
        let cell = crate::storage::flash()?;
        let config = FirmwareUpdaterConfig::from_linkerfile_blocking(cell, cell);
        let mut aligned = AlignedBuffer([0; STATE_WORD]);
        let mut updater = BlockingFirmwareUpdater::new(config, &mut aligned.0);
        Some(run(&mut updater))
    }

    /// Read the bootloader's verdict, and record a rollback if that is what it
    /// was.
    ///
    /// Call once from `main`, **before core 1 starts** — it both reads and (on
    /// a revert) writes flash, and a write once the render loop is up costs the
    /// panel a frame.
    ///
    /// # The quirk the spike found
    ///
    /// On the boot that *performs* a revert, `prepare_boot` returns
    /// `State::Swap` — the magic it read on entry. The app sees `State::Revert`
    /// from then on, which is what this reads, and it stays `Revert` until
    /// something calls `mark_booted`. So a revert is not a transient: a device
    /// that rolled back and then never confirmed would report `Revert` forever.
    pub fn read_boot_state() {
        let state = with_updater(|updater| updater.get_state()).and_then(Result::ok);
        match state {
            Some(BootState::Swap) => {
                crate::error!(
                    "ota: TRIAL boot of {} — the bootloader swapped, nothing has confirmed it",
                    VERSION
                );
                set_state(State::Trial);
            }
            Some(BootState::Revert) => {
                // The image that failed is the one the record names. Marking it
                // is what stops the next check downloading it again tomorrow,
                // and the day after.
                match crate::storage::load_ota_attempt() {
                    Some(mut record) => {
                        crate::error!(
                            "ota: ROLLED BACK to {}; {} failed its health gate and will not be retried",
                            VERSION,
                            record.target.as_str()
                        );
                        record.reverted = true;
                        crate::storage::save_ota_attempt(&record);
                    }
                    // No record and a revert: the update was installed by a
                    // firmware that did not keep one, or the region was erased
                    // between. Nothing to block, and the version compare will
                    // offer the same image again — which is right, because
                    // there is no evidence about which image failed.
                    None => crate::error!(
                        "ota: ROLLED BACK to {}, with no attempt record to say what failed",
                        VERSION
                    ),
                }
                set_state(State::RolledBack);
            }
            Some(_) => {}
            None => crate::error!("ota: the bootloader state partition did not read"),
        }
    }

    /// Confirm this image: `mark_booted`, which cleans `Swap` or `Revert` back
    /// to `Boot`.
    ///
    /// Called by [`crate::supervise`] once the health gate passes. Both states
    /// need it — a `Revert` that is never cleaned leaves the state machine
    /// somewhere the next update cannot start from.
    pub fn confirm() -> bool {
        let marked = with_updater(|updater| updater.mark_booted());
        match marked {
            Some(Ok(())) => {
                set_state(State::Idle);
                true
            }
            Some(Err(_)) => {
                crate::error!("ota: mark_booted failed; this image will be rolled back on reset");
                false
            }
            None => false,
        }
    }

    /// Download, verify, arm, count down, reset. Never returns on success.
    #[expect(clippy::too_many_arguments, reason = "every one is a distinct borrow from the poll loop; bundling them would be a struct that exists for the lint")]
    pub async fn install(
        client: &mut ApiClient,
        buffer: &mut ResponseBuffer,
        base_url: &str,
        api_key: &str,
        channel: Channel,
        store: &mut Store,
        publisher: &mut Publisher<'_>,
        manifest: &Manifest,
        record: Option<Attempt>,
    ) -> Answer {
        crate::error!(
            "ota: installing {} ({} bytes) over {}",
            manifest.version.as_str(),
            manifest.size,
            VERSION
        );

        // Write 1 of 3: the attempt, BEFORE the download. See the module docs
        // in `ota/mod.rs` for why the order is load-bearing.
        let mut attempt = match record {
            Some(mut existing) if existing.is_about(&manifest.version) => {
                existing.again();
                existing
            }
            _ => match Attempt::first(&manifest.version) {
                Ok(fresh) => fresh,
                Err(_) => return Answer { status: "error", message: Some("the manifest version is too long to record") },
            },
        };
        attempt.reverted = false;
        crate::storage::save_ota_attempt(&attempt);

        set_state(State::Downloading);
        if let Err(message) = download(
            client, buffer, base_url, api_key, channel, store, publisher, manifest,
        )
        .await
        {
            crate::error!("ota: download failed: {}", message);
            return Answer { status: "error", message: Some(message) };
        }

        set_state(State::Verifying);
        if let Err(message) = verify(buffer, manifest) {
            crate::error!("ota: {}", message);
            return Answer { status: "error", message: Some(message) };
        }

        // Write 3 of 3. From here the bootloader will swap on the next boot.
        set_state(State::Restarting);
        crate::error!("ota: {} armed; restarting", manifest.version.as_str());

        for remaining in (1..=COUNTDOWN_SECONDS).rev() {
            store.set_updating_countdown(remaining);
            publisher.publish(store.snapshot());
            Timer::after(Duration::from_secs(1)).await;
        }
        crate::supervise::force_reset()
    }

    /// Stream the image into DFU.
    #[expect(clippy::too_many_arguments, reason = "as above")]
    async fn download(
        client: &mut ApiClient,
        buffer: &mut ResponseBuffer,
        base_url: &str,
        api_key: &str,
        channel: Channel,
        store: &mut Store,
        publisher: &mut Publisher<'_>,
        manifest: &Manifest,
    ) -> Result<(), &'static str> {
        let url = crate::net::api_client::url(
            base_url,
            format_args!("/fw/image?channel={}", channel.as_str()),
        )
        .map_err(|_| "the configured backend url does not fit")?;

        // The union SPEC §11's budget table anticipated: the poller is not
        // polling while this runs, so its 4 KB receive buffer becomes a header
        // half and a chunk half and the download costs no RAM at all.
        let (header, chunk) = buffer.split_at_mut(scoreboard_model::poll::DETAIL_BYTES);

        // The updater is built here rather than through `with_updater`, and the
        // difference matters: it has to stay alive across every `await` of the
        // transfer. Wrapping the download in a sync closure would mean
        // `block_on`, which would hold the executor for the whole minute — and
        // the watchdog feeder is a *task*, so it would never run and the device
        // would reset a third of the way through every update.
        //
        // Nothing is starved by the writes themselves: each chunk is one
        // erase-and-program of a few tens of milliseconds, and the executor
        // runs again while the next chunk is in flight.
        let cell = crate::storage::flash().ok_or("the flash peripheral is not available")?;
        let config = FirmwareUpdaterConfig::from_linkerfile_blocking(cell, cell);
        let mut aligned = AlignedBuffer([0; STATE_WORD]);
        let mut updater = BlockingFirmwareUpdater::new(config, &mut aligned.0);

        let mut progress = Progress::new(manifest.size);
        let mut offset = 0usize;
        let mut fault: Option<&'static str> = None;

        let outcome = embassy_time::with_timeout(
            DOWNLOAD_TIMEOUT,
            client.download(&url, api_key, header, chunk, |bytes| {
                if updater.write_firmware(offset, bytes).is_err() {
                    fault = Some("the DFU partition would not take the image");
                    return false;
                }
                offset += bytes.len();
                if let Some(percent) = progress.advance(bytes.len() as u32) {
                    crate::ota::set_percent(percent);
                    store.set_updating_progress(percent, manifest.version.as_str());
                    publisher.publish(store.snapshot());
                }
                true
            }),
        )
        .await;

        match outcome {
            Err(_) => return Err("the download timed out"),
            Ok(Err(_)) => return Err(fault.unwrap_or("the download was interrupted")),
            Ok(Ok(())) => {}
        }
        if let Some(message) = fault {
            return Err(message);
        }
        if offset as u32 != manifest.size {
            return Err("the backend sent fewer bytes than the manifest promised");
        }
        Ok(())
    }

    /// Hash the DFU partition and check the signature.
    ///
    /// The two things that make this different from
    /// `verify_and_mark_updated`, and the reasons, are in
    /// [`scoreboard_ota::verify`]'s module docs: the 4 KB chunk buffer instead
    /// of embassy-boot's hardcoded two, and `verify_strict`.
    fn verify(buffer: &mut ResponseBuffer, manifest: &Manifest) -> Result<(), &'static str> {
        // The one call in the whole update that blocks the executor for an
        // unbounded time, so the watchdog gets a full window immediately before
        // it. `crate::supervise::feed_watchdog` explains why that is legitimate
        // from a task that does not own the peripheral.
        crate::supervise::feed_watchdog();
        let started = Instant::now();

        let mut digest = [0u8; 64];
        let hashed = with_updater(|updater| {
            updater.hash::<Sha512>(manifest.size, &mut buffer[..], &mut digest)
        });
        match hashed {
            Some(Ok(())) => {}
            Some(Err(_)) => return Err("the DFU partition would not read back"),
            None => return Err("the flash peripheral is not available"),
        }

        // Drill day reads this number off the ring log; it is the one figure
        // that decides whether embassy-boot's own two-byte path would have been
        // survivable, and it is at ERROR so it outlives the default log level.
        crate::error!(
            "ota: hashed {} bytes of DFU in {} ms",
            manifest.size,
            started.elapsed().as_millis() as u32
        );
        crate::supervise::feed_watchdog();

        let verified = scoreboard_ota::verify(&digest, &PUBLIC_KEY, &manifest.signature)
            .map_err(scoreboard_ota::SignatureError::as_str)?;
        arm_swap(verified)
    }

    /// Request the swap.
    ///
    /// Takes [`Verified`](scoreboard_ota::Verified) **by value**, which is the
    /// whole mechanism: the token has a private field, `scoreboard_ota::verify`
    /// is the only thing that can build one, and so a call that arms a swap
    /// without having verified the image does not compile. That is the
    /// guarantee `embassy-boot`'s verify feature would have given by deleting
    /// `mark_updated`, rebuilt where the deletion could not be taken.
    fn arm_swap(verified: scoreboard_ota::Verified) -> Result<(), &'static str> {
        let _ = verified;
        match with_updater(|updater| updater.mark_updated()) {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("the swap could not be armed"),
            None => Err("the flash peripheral is not available"),
        }
    }
}

#[cfg(not(feature = "link-boot-integrated"))]
mod integrated {
    use scoreboard_model::{Publisher, Store};
    use scoreboard_ota::{Attempt, Channel, Manifest};

    use crate::net::api_client::{ApiClient, ResponseBuffer};
    use crate::ota::Answer;

    /// No bootloader, so no state partition to read and no trial to be on.
    pub fn read_boot_state() {}

    /// Nothing to confirm. `true` because the health gate's caller reads this
    /// as "the boot is settled", and on a probe-flashed image it always is.
    pub fn confirm() -> bool {
        true
    }

    /// A standalone image links at flash offset 0 with no DFU partition behind
    /// it, so there is nowhere to stage an image and nothing to swap it with.
    /// Answering plainly is better than pretending: the settings page shows the
    /// message, and `cargo run` keeps working, which is the whole reason this
    /// profile exists.
    #[expect(clippy::too_many_arguments, reason = "the signature matches the boot-integrated one so the caller needs no cfg")]
    pub async fn install(
        _client: &mut ApiClient,
        _buffer: &mut ResponseBuffer,
        _base_url: &str,
        _api_key: &str,
        _channel: Channel,
        _store: &mut Store,
        _publisher: &mut Publisher<'_>,
        _manifest: &Manifest,
        _record: Option<Attempt>,
    ) -> Answer {
        crate::error!("ota: an update is published, but this image was built standalone");
        Answer {
            status: "error",
            message: Some("this build has no bootloader and cannot install updates"),
        }
    }
}

pub use integrated::{confirm, install, read_boot_state};

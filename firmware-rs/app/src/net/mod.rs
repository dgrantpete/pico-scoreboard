//! Core 0's networking: the CYW43 radio, the embassy-net stack, and the boot
//! path that decides between joining a network and becoming one.
//!
//! The port of `main.py`'s network phase. That phase is *blocking* in the
//! MicroPython firmware — `main()` runs `start_station_mode()` to completion
//! before it starts the poller, the buttons or the watchdog — and [`bringup`]
//! keeps that shape: everything after provisioning is spawned from inside it,
//! by the arm that won.
//!
//! # Resource map
//!
//! Three PIO blocks, sixteen DMA channels, and two peripheral access crates
//! that cannot see each other's bookkeeping. Who owns what:
//!
//! | Silicon | Owner | Enforced by |
//! |---|---|---|
//! | PIO0 | `hub75`, via `rp235x-pac` | `main`'s `_owned_by_hub75` binding |
//! | PIO1 | the button driver (task #12) | unclaimed; reserved here in writing |
//! | **PIO2** | **cyw43's `PioSpi`, SM0, IRQ0** | [`Irqs`] and `Peripherals` below |
//! | DMA CH0 | **cyw43's `PioSpi`** | [`Peripherals::dma`] |
//! | DMA CH12–15 | `hub75` | `hub75::driver`'s public channel constants |
//!
//! PIO2 exists only on the RP2350, which is one more thing the RP2040 ban in
//! SPEC §1 buys: the radio gets a whole PIO block and never has to share
//! instruction memory with the panel.
//!
//! The DMA split is the one that needs an argument, because embassy's
//! `dma::Channel::new` writes `DMA.INTE0` and enables `DMA_IRQ_0` in the NVIC —
//! a global, on hardware `hub75` also drives. It is safe because the write is
//! `write_set` against the RP2350's atomic set alias, so it touches bit 0 and
//! nothing else, and because `hub75` never enables a DMA interrupt: its four
//! channels run with `irq_quiet` set, and the one place it wants an IRQ (the
//! graceful `deinit` handshake) *polls* `DMA.INTR` instead of unmasking it.
//! Channel 0 firing `DMA_IRQ_0` therefore cannot be confused with anything the
//! panel is doing.
//!
//! # Socket budget
//!
//! [`SOCKETS`] sizes the whole stack, and station and AP mode want different
//! things, so it covers the larger of the two:
//!
//! | Socket | Station | AP | Owner |
//! |---|:-:|:-:|---|
//! | embassy-net's DNS resolver | 1 | 1 | embassy-net, always added |
//! | DHCP *client* | 1 | — | embassy-net, while `ConfigV4::Dhcp` |
//! | poller | 1 | — | task #11 |
//! | OTA | 1 | — | task #15 |
//! | HTTP server | 2 | 2 | task #10 |
//! | captive DNS | — | 1 | [`captive_dns`] |
//! | DHCP *server* | — | 1 | [`dhcp_server`] |
//! | **total** | **6** | **5** | |
//!
//! Seven is the working ceiling; [`SOCKETS`] is 8 so that adding one consumer
//! is a budget line rather than a rewrite.

pub mod captive_dns;
pub mod dhcp_server;
pub mod hosts;
#[cfg(feature = "net-probe")]
mod probe;
pub mod wifi;

use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use embassy_executor::Spawner;
use embassy_net::{Runner as NetRunner, Stack, StackResources};
use embassy_rp::Peri;
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_24, PIN_25, PIN_29, PIO2};
use embassy_rp::pio::Pio;
use embassy_rp::{bind_interrupts, dma, pio};
use scoreboard_model::{Mode, Publisher, StartupExit, Store};
use static_cell::StaticCell;

use crate::net::hosts::MyHosts;
use crate::net::wifi::{Credentials, Provisioned};

bind_interrupts!(struct Irqs {
    PIO2_IRQ_0 => pio::InterruptHandler<PIO2>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
});

/// Socket slots in the stack's `SocketSet`. See the module docs' table.
pub const SOCKETS: usize = 8;

/// The radio's silicon, taken from `Peripherals` in `main` so the resource map
/// is decided in one place and passed here whole.
pub struct NetPeripherals {
    pub pio: Peri<'static, PIO2>,
    pub dma: Peri<'static, DMA_CH0>,
    /// WL_ON — cuts power to the radio when low.
    pub pwr: Peri<'static, PIN_23>,
    pub cs: Peri<'static, PIN_25>,
    pub dio: Peri<'static, PIN_24>,
    pub clk: Peri<'static, PIN_29>,
}

/// The CYW43's own firmware, uploaded over SPI at every boot because the radio
/// has no flash of its own. Provenance and hashes: `cyw43-firmware/README.md`.
static FIRMWARE: &cyw43::Aligned<cyw43::A4, [u8]> =
    cyw43::aligned_bytes!("../../cyw43-firmware/43439A0.bin");
static NVRAM: &cyw43::Aligned<cyw43::A4, [u8]> =
    cyw43::aligned_bytes!("../../cyw43-firmware/nvram_rp2040.bin");
/// The Country Locale Matrix — channel and power limits per region. Handed to
/// `Control::init` rather than to `cyw43::new`, which is why it is a plain
/// slice and needs no alignment.
static CLM: &[u8] = include_bytes!("../../cyw43-firmware/43439A0_clm.bin");

static CYW43_STATE: StaticCell<cyw43::State> = StaticCell::new();
static STACK_RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();

/// Drives the radio: SPI transfers, firmware events, and the packet path in
/// both directions. Never returns; if it stops, the network stops.
#[embassy_executor::task]
async fn cyw43_runner(
    runner: cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO2, 0>>>,
) -> ! {
    runner.run().await
}

/// Drives smoltcp: timers, ARP, DHCP, and every socket's state machine.
#[embassy_executor::task]
async fn net_runner(mut runner: NetRunner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

/// Bring the radio up, provision, and hand off to whichever mode won.
///
/// Owns the [`Store`] for the whole boot, which is what makes the startup
/// screen work without a lock: `main.py` published progress from the same
/// single-threaded context that did the joining, and so does this. Where the
/// authoritative `Store` lives *after* the boot is task #11's call — the seam
/// is that [`wifi::provision`] takes it by `&mut`, so whoever ends up owning it
/// only has to lend it.
#[embassy_executor::task]
pub async fn bringup(spawner: Spawner, mut publisher: Publisher<'static>, p: NetPeripherals) {
    let mut store = Store::new();
    // Step 1 of 5. `main.py` commits this from its display-init block, before
    // core 1 starts; here core 1 is already running, so this is the first thing
    // it has to render that is not the empty snapshot.
    store.set_startup_step(1, wifi::STARTUP_STEPS, "Display", "Initialized", 0, 0);
    publisher.publish(store.snapshot());

    let pwr = Output::new(p.pwr, Level::Low);
    let cs = Output::new(p.cs, Level::High);
    let mut pio = Pio::new(p.pio, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        // Not `DEFAULT_CLOCK_DIVIDER`. On the RP2350 that divider lands the
        // GSPI clock at 37.5 MHz, which this silicon does not reliably survive
        // (embassy-rs/embassy#3960) — the symptom is a firmware upload that
        // completes but leaves the radio wedged. RM2_CLOCK_DIVIDER is ÷3, so
        // 150 MHz → 50 MHz PIO → 25 MHz GSPI, and it is what embassy's own
        // rp235x Wi-Fi example uses.
        RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.dio,
        p.clk,
        dma::Channel::new(p.dma, Irqs),
    );

    let state = CYW43_STATE.init(cyw43::State::new());
    let (device, mut control, runner) = cyw43::new(state, pwr, spi, FIRMWARE, NVRAM).await;
    spawner.spawn(defmt::unwrap!(cyw43_runner(runner)));
    control.init(CLM).await;

    let credentials = Credentials::from_dev_build();

    // Seeds smoltcp's TCP sequence numbers and ephemeral port choice. ROSC
    // entropy rather than the TRNG peripheral: this is the only randomness the
    // firmware needs and it does not justify claiming a peripheral.
    let seed = RoscRng.next_u64();
    let resources = STACK_RESOURCES.init(StackResources::new());
    // `ConfigV4::None` to start: `wifi::provision` sets DHCP or the AP's static
    // address itself, per attempt, so a half-finished lease never survives into
    // the next one.
    let (stack, runner) = embassy_net::new(device, Default::default(), resources, seed);
    spawner.spawn(defmt::unwrap!(net_runner(runner)));

    let outcome = wifi::provision(
        &mut control,
        stack,
        &mut store,
        &mut publisher,
        &credentials,
    )
    .await;

    let hosts = match outcome {
        Provisioned::Station { ip } => {
            // `main.py` syncs time here (step 5 covers both) before it starts
            // any service. Time sync is task #11's, on the same seam.
            store.set_startup_step(5, wifi::STARTUP_STEPS, "Starting", "Services", 0, 0);
            publisher.publish(store.snapshot());
            store.finish_startup(StartupExit::Mode(Mode::Idle));
            publisher.publish(store.snapshot());

            defmt::info!(
                "station mode: {} as {=str}, services starting",
                ip,
                credentials.device_name
            );
            // The placeholder core-0 producer, until the poller (task #11)
            // replaces it. It takes the publisher, so nothing here can publish
            // afterwards — which is the point: one writer.
            spawner.spawn(defmt::unwrap!(crate::demo::feed(publisher)));
            #[cfg(feature = "net-probe")]
            spawner.spawn(defmt::unwrap!(probe::fetch_time(stack)));
            MyHosts::station(credentials.device_name, &wifi::address_text(ip))
        }
        Provisioned::Ap { reason, ip } => {
            store.finish_startup(StartupExit::Setup {
                reason,
                ap_ssid: credentials.device_name,
                ap_ip: wifi::AP_IP_TEXT,
                wifi_ssid: credentials.ssid,
            });
            publisher.publish(store.snapshot());

            defmt::info!(
                "setup mode: reason {=str}, ap ssid {=str}, ap ip {}",
                reason_name(reason),
                credentials.device_name,
                ip
            );
            spawner.spawn(defmt::unwrap!(captive_dns::serve(stack, ip)));
            spawner.spawn(defmt::unwrap!(dhcp_server::serve(stack, ip)));
            MyHosts::ap(credentials.device_name, wifi::AP_IP_TEXT)
        }
    };

    // Task #10's Host-header check reads this. Published rather than returned
    // because the HTTP server is a sibling task, not a child of this one.
    hosts::publish(hosts);
    hosts::with(|hosts| {
        // Worth a line: it says exactly which `Host` values will be served and
        // which will be treated as a hijacked probe, which is otherwise only
        // discoverable by pointing a phone at the device.
        defmt::info!(
            "http host check: \"{=str}\", \"{=str}.local\", \"{=str}\"{=str}",
            hosts.device_name(),
            hosts.device_name(),
            hosts.address(),
            if hosts.captive() {
                ", anything else redirects to the setup page"
            } else {
                ", anything else is served normally"
            }
        );
    });

    spawner.spawn(defmt::unwrap!(watch_link(stack)));

    // The radio and `control` outlive this function either way — the runner
    // task owns the hardware. What ends here is the *boot*, which is why this
    // task returns instead of parking: nothing it holds, the `Store` included,
    // has a reader once the handoff above is done.
}

/// The API spelling of a setup reason, for logs and for task #10's
/// `/api/status`. `api_routes.py`'s `setup_reason` field, verbatim — note that
/// it is **not** the model's enum name: the wire says `no_network_configured`
/// where the model says `NoConfig`.
pub fn reason_name(reason: scoreboard_model::SetupReason) -> &'static str {
    use scoreboard_model::SetupReason;
    match reason {
        SetupReason::NoConfig => "no_network_configured",
        SetupReason::ConnectionFailed => "connection_failed",
        SetupReason::BadAuth => "bad_auth",
    }
}

/// Log every transition of the stack's IPv4 configuration.
///
/// Deliberately **not** a reconnect loop: `main.py` has none either. A station
/// that loses its link surfaces as poller failures, then the error screen, then
/// the watchdog, and inventing a different recovery here would be a behaviour
/// change smuggled in under "networking". What this buys is the difference
/// between diagnosing an overnight drop from the log and guessing at it.
#[embassy_executor::task]
async fn watch_link(stack: Stack<'static>) -> ! {
    loop {
        stack.wait_config_up().await;
        match stack.config_v4() {
            Some(config) => defmt::info!("link: up, address {}", config.address),
            None => defmt::info!("link: up, no ipv4 configuration"),
        }
        stack.wait_config_down().await;
        defmt::warn!("link: ipv4 configuration dropped");
    }
}

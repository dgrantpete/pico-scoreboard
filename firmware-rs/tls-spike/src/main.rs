//! S2 on-silicon TLS bring-up (PHASE-S-CHECKLIST.md, S2): what a TLS 1.3
//! handshake, a kept-alive request, and the client's buffers actually cost
//! on the RP2350, measured before any figure is believed — the house rule
//! the verify-hash estimate earned at 8×.
//!
//! Three targets, in escalating realism:
//!  1. a LAN TLS terminator fronting the `tools/espn` mock (plain-HTTP mock,
//!     TLS added in front — the S2 rig the checklist prescribes),
//!  2. `site.api.espn.com` — the real data plane (RSA chain, so this runs
//!     unauthenticated: embedded-tls's `rsa` verification needs `alloc`,
//!     and whether the firmware ever takes that is the S2 design decision
//!     these numbers feed),
//!  3. `github.com` — the one all-ECDSA chain, where full `rustpki`
//!     verification is available without `alloc` (measured separately once
//!     the verifier is wired; this binary measures the transport first).
//!
//! Per target: N cold connections (TCP + TLS + GET, timed as one unit the
//! way the poller would pay it), then a keep-alive GET on the same
//! connection — the difference is what the handshake costs and what
//! keep-alive amortizes. Exists to produce numbers, not to be a product.

#![no_std]
#![no_main]

use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use defmt::{info, unwrap, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use embassy_net::{Runner as NetRunner, StackResources};
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO2};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::{bind_interrupts, dma};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use panic_probe as _;
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
use reqwless::request::{Method, RequestBuilder as _};
use static_cell::{ConstStaticCell, StaticCell};

bind_interrupts!(struct Irqs {
    PIO2_IRQ_0 => PioInterruptHandler<PIO2>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
});

/// The CYW43's firmware, same blobs and provenance as the app.
static FIRMWARE: &cyw43::Aligned<cyw43::A4, [u8]> =
    cyw43::aligned_bytes!("../../app/cyw43-firmware/43439A0.bin");
static NVRAM: &cyw43::Aligned<cyw43::A4, [u8]> =
    cyw43::aligned_bytes!("../../app/cyw43-firmware/nvram_rp2040.bin");
static CLM: &[u8] = include_bytes!("../../app/cyw43-firmware/43439A0_clm.bin");

static CYW43_STATE: StaticCell<cyw43::State> = StaticCell::new();
static STACK_RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

/// One pooled TCP connection, the poller's shape. Sized like the app's
/// api_client socket: 1,536 B receive / 512 B send.
static TCP_STATE: ConstStaticCell<TcpClientState<1, 512, 1536>> =
    ConstStaticCell::new(TcpClientState::new());

/// embedded-tls record buffers. The read buffer must hold one full TLS
/// record (16 KB payload + record overhead); the write buffer bounds our
/// request size. These two are the headline RAM cost S2 exists to price.
const TLS_READ_BYTES: usize = 16_640;
const TLS_WRITE_BYTES: usize = 4_096;
static TLS_READ: ConstStaticCell<[u8; TLS_READ_BYTES]> = ConstStaticCell::new([0; TLS_READ_BYTES]);
static TLS_WRITE: ConstStaticCell<[u8; TLS_WRITE_BYTES]> =
    ConstStaticCell::new([0; TLS_WRITE_BYTES]);

/// Response sink: bodies are drained and discarded, sized for the largest
/// headers + a chunk of body the reads pull per call.
static RESPONSE: ConstStaticCell<[u8; 4_096]> = ConstStaticCell::new([0; 4_096]);

/// The backend's allowlisted User-Agent prefix (`backend/config/default.toml`
/// — ESPN's edge 403s unknown agents; S2 checklist item).
const USER_AGENT: &str = "python-requests/2.32.3 pico-scoreboard/1.0";

const COLD_REPS: u32 = 5;
const KEEPALIVE_REPS: u32 = 5;

#[embassy_executor::task]
async fn cyw43_runner(
    runner: cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO2, 0>>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_runner(mut runner: NetRunner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}

struct Target {
    label: &'static str,
    /// A full URL for reqwless; the terminator entry is built at runtime
    /// from the build-time address.
    url: &'static str,
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("=== tls-spike up: sys {=u32} Hz ===", embassy_rp::clocks::clk_sys_freq());

    // Radio bring-up, identical to the app's (including the RM2_CLOCK_DIVIDER
    // workaround for embassy-rs/embassy#3960 — the default divider wedges
    // this silicon's GSPI).
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO2, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        RM2_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        dma::Channel::new(p.DMA_CH0, Irqs),
    );

    let state = CYW43_STATE.init(cyw43::State::new());
    let (device, mut control, runner) = cyw43::new(state, pwr, spi, FIRMWARE, NVRAM).await;
    spawner.spawn(unwrap!(cyw43_runner(runner)));
    control.init(CLM).await;
    // App parity (net/wifi.rs does the same): the first measurement round
    // ran PowerSave and its BUDGET numbers carry that caveat.
    control
        .set_power_management(cyw43::PowerManagementMode::None)
        .await;

    let seed = RoscRng.next_u64();
    let resources = STACK_RESOURCES.init(StackResources::new());
    let (stack, runner) = embassy_net::new(device, Default::default(), resources, seed);
    spawner.spawn(unwrap!(net_runner(runner)));

    // Station join with the dev.toml identity; simple bounded retry — this
    // is a bench, not provisioning.
    stack.set_config_v4(embassy_net::ConfigV4::Dhcp(Default::default()));
    let ssid = env!("SPIKE_WIFI_SSID");
    let password = env!("SPIKE_WIFI_PASSWORD");
    loop {
        match with_timeout(
            Duration::from_secs(20),
            control.join(ssid, cyw43::JoinOptions::new(password.as_bytes())),
        )
        .await
        {
            Ok(Ok(())) => break,
            outcome => {
                warn!("join attempt failed ({=bool} timeout), retrying", outcome.is_err());
                Timer::after(Duration::from_secs(2)).await;
            }
        }
    }
    info!("joined {=str}", ssid);
    stack.wait_config_up().await;
    let config = unwrap!(stack.config_v4());
    info!("ip {}", config.address);

    let tcp_state = TCP_STATE.take();
    let tls_read = TLS_READ.take();
    let tls_write = TLS_WRITE.take();
    let response = RESPONSE.take();
    let tls_seed = RoscRng.next_u64();

    const MOCK_URL: &str = concat!("https://", env!("SPIKE_TERMINATOR"), "/baseball/mlb/scoreboard");

    // Soak build (SPIKE_SOAK=1): sustained keep-alive polling of the
    // fronted mock at the poller's cadence — the S2 exit line's soak.
    // Never touches the real hosts; the one-pass build does that.
    if option_env!("SPIKE_SOAK").is_some() {
        soak(stack, tcp_state, tls_read, tls_write, response, tls_seed, MOCK_URL).await;
    }

    let targets = [
        Target {
            label: "terminator-mock",
            // The mock's own path shape (tools/espn/mockserver.py), not
            // ESPN's — the terminator is a byte pipe, not a rewriter.
            url: MOCK_URL,
        },
        Target {
            label: "espn-real",
            url: "https://site.api.espn.com/apis/site/v2/sports/baseball/mlb/scoreboard",
        },
        Target {
            label: "github-ecdsa",
            url: "https://github.com/robots.txt",
        },
    ];

    for target in &targets {
        info!("--- target {=str}: {=str}", target.label, target.url);

        // Cold connections: TCP + TLS handshake + GET + drain, timed as one
        // unit — the cost the poll loop pays when keep-alive has lapsed.
        for rep in 0..COLD_REPS {
            let tcp = TcpClient::new(stack, tcp_state);
            let dns = DnsSocket::new(stack);
            let tls = TlsConfig::new(tls_seed, tls_read, tls_write, TlsVerify::None);
            let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

            let t0 = Instant::now();
            // Bounded like the app poller's requests: a dead target costs
            // one timeout, never a wedged lane.
            let outcome =
                with_timeout(Duration::from_secs(20), fetch_once(&mut client, target.url, response))
                    .await;
            let us = t0.elapsed().as_micros();
            match outcome {
                Ok(Ok((status, body_len))) => info!(
                    "COLD {=str} rep={=u32}: {=u64} us (status {=u16}, {=usize} B body)",
                    target.label, rep, us, status, body_len
                ),
                Ok(Err(e)) => warn!("COLD {=str} rep={=u32}: FAILED {}", target.label, rep, e),
                Err(_) => {
                    warn!("COLD {=str} rep={=u32}: TIMEOUT, skipping target", target.label, rep);
                    break;
                }
            }
            // The TLS buffers are exclusive to one client; dropping it frees
            // them for the next loop. A breather between reps keeps remote
            // rate limiting out of the numbers.
            Timer::after(Duration::from_millis(500)).await;
        }

        // Keep-alive: one connection, repeated GETs through a held resource.
        // The per-request delta against COLD is what the handshake costs.
        {
            let tcp = TcpClient::new(stack, tcp_state);
            let dns = DnsSocket::new(stack);
            let tls = TlsConfig::new(tls_seed, tls_read, tls_write, TlsVerify::None);
            let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

            let t0 = Instant::now();
            match with_timeout(Duration::from_secs(20), client.resource(target.url)).await {
                Err(_) => warn!("KEEPALIVE {=str}: setup TIMEOUT, skipping", target.label),
                Ok(Err(e)) => warn!("KEEPALIVE {=str}: resource setup FAILED {}", target.label, e),
                Ok(Ok(mut resource)) => {
                    let setup_us = t0.elapsed().as_micros();
                    info!("KEEPALIVE {=str} setup (connect+handshake): {=u64} us", target.label, setup_us);
                    for rep in 0..KEEPALIVE_REPS {
                        let t0 = Instant::now();
                        let outcome = async {
                            let mut request =
                                resource.get("").headers(&[("User-Agent", USER_AGENT)]);
                            let response = request.send(response).await?;
                            let status: u16 = response.status.0;
                            let body_len = drain_body(response.body().reader()).await?;
                            Ok::<_, reqwless::Error>((status, body_len))
                        }
                        .await;
                        let us = t0.elapsed().as_micros();
                        match outcome {
                            Ok((status, body_len)) => info!(
                                "KEEPALIVE {=str} rep={=u32}: {=u64} us (status {=u16}, {=usize} B body)",
                                target.label, rep, us, status, body_len
                            ),
                            Err(e) => {
                                warn!("KEEPALIVE {=str} rep={=u32}: FAILED {}", target.label, rep, e);
                                break;
                            }
                        }
                        Timer::after(Duration::from_millis(200)).await;
                    }
                }
            }
        }
    }

    info!("=== TLS SPIKE COMPLETE ===");
    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

/// The sustained soak: a kept-alive connection polled at the app's
/// cadence, reconnecting on any failure the way the real poller would.
/// Diverges (never returns) — the numbers stream out over defmt, one line
/// per poll and a summary every ten.
async fn soak(
    stack: embassy_net::Stack<'static>,
    tcp_state: &'static TcpClientState<1, 512, 1536>,
    tls_read: &'static mut [u8; TLS_READ_BYTES],
    tls_write: &'static mut [u8; TLS_WRITE_BYTES],
    response: &'static mut [u8; 4096],
    tls_seed: u64,
    url: &str,
) -> ! {
    const CADENCE: Duration = Duration::from_secs(30);
    let mut polls: u32 = 0;
    let mut ok: u32 = 0;
    let mut failed: u32 = 0;
    let mut reconnects: u32 = 0;
    let mut sum_us: u64 = 0;

    info!("SOAK start: {=str}, cadence {=u64} s", url, CADENCE.as_secs());
    loop {
        // One connection per iteration of this outer loop; the inner loop
        // holds it across polls until something breaks.
        let tcp = TcpClient::new(stack, tcp_state);
        let dns = DnsSocket::new(stack);
        let tls = TlsConfig::new(tls_seed, &mut *tls_read, &mut *tls_write, TlsVerify::None);
        let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);

        let setup = with_timeout(Duration::from_secs(20), client.resource(url)).await;
        let mut resource = match setup {
            Ok(Ok(resource)) => resource,
            outcome => {
                reconnects = reconnects.saturating_add(1);
                warn!(
                    "SOAK reconnect FAILED (timeout={=bool}); retrying in {=u64} s",
                    outcome.is_err(),
                    CADENCE.as_secs()
                );
                Timer::after(CADENCE).await;
                continue;
            }
        };

        loop {
            let t0 = Instant::now();
            let outcome = with_timeout(Duration::from_secs(20), async {
                let mut request = resource.get("").headers(&[("User-Agent", USER_AGENT)]);
                let sent = request.send(response).await?;
                let status: u16 = sent.status.0;
                let body_len = drain_body(sent.body().reader()).await?;
                Ok::<_, reqwless::Error>((status, body_len))
            })
            .await;
            let us = t0.elapsed().as_micros();
            polls += 1;
            match outcome {
                Ok(Ok((status, body_len))) => {
                    ok += 1;
                    sum_us += us;
                    info!(
                        "SOAK poll={=u32}: {=u64} us (status {=u16}, {=usize} B)",
                        polls, us, status, body_len
                    );
                }
                outcome => {
                    failed += 1;
                    warn!(
                        "SOAK poll={=u32}: FAILED (timeout={=bool}) after {=u64} us; reconnecting",
                        polls,
                        outcome.is_err(),
                        us
                    );
                    Timer::after(CADENCE).await;
                    break;
                }
            }
            if polls.is_multiple_of(10) {
                info!(
                    "SOAK-SUMMARY polls={=u32} ok={=u32} failed={=u32} reconnects={=u32} avg_ok={=u64} us",
                    polls,
                    ok,
                    failed,
                    reconnects,
                    sum_us / u64::from(ok.max(1))
                );
            }
            Timer::after(CADENCE).await;
        }
    }
}

/// One whole request on a fresh connection: DNS, TCP, TLS, GET, drain.
async fn fetch_once(
    client: &mut HttpClient<'_, TcpClient<'_, 1, 512, 1536>, DnsSocket<'_>>,
    url: &str,
    response_buf: &mut [u8],
) -> Result<(u16, usize), reqwless::Error> {
    let mut request = client.request(Method::GET, url).await?;
    let mut request = request.headers(&[("User-Agent", USER_AGENT)]);
    let response = request.send(response_buf).await?;
    let status: u16 = response.status.0;
    let body_len = drain_body(response.body().reader()).await?;
    Ok((status, body_len))
}

/// Stream the body away in chunks, the way the real poller consumes it —
/// `read_to_end` needs the whole body in RAM, which a 481 KB slate never
/// fits. Returns the byte count so the log proves the whole body moved.
async fn drain_body<B: embedded_io_async::Read<Error = reqwless::Error>>(
    mut reader: B,
) -> Result<usize, reqwless::Error> {
    let mut chunk = [0u8; 2048];
    let mut total = 0;
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok(total);
        }
        total += n;
    }
}

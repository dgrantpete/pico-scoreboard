//! The web server: the settings SPA, the REST surface, and the captive
//! portal's HTTP half.
//!
//! Port of `api_routes.py` plus `main.py`'s two catch-all handlers, on
//! picoserve instead of Microdot. [`routes`] is the surface itself; this module
//! is the sockets, the buffers and the four tasks that own them.
//!
//! # Four connections, and why that is a number and not a default
//!
//! `net`'s socket budget reserves exactly four slots here. picoserve serves
//! one connection per task, so there are four tasks, and a fifth simultaneous
//! client waits in the listen backlog rather than being refused.
//!
//! It was two — enough for the SPA's logs page polling `/api/logs` on a 3 s
//! timer beside the settings page — until drill day (2026-08-16) put an
//! iPhone's captive-portal sheet in front of the setup flow. iOS's hotspot
//! login is a connection storm: the captive probe, the page, Safari's
//! speculative preconnects that never send a request, and the SPA's API
//! fetches, all at once — and the SWD capture showed a stalled writer pinning
//! one of the two slots through its full 10 s timeout while the sheet's
//! fetches died in the backlog. "Connection Error" on the setup page, on the
//! network the device itself is running. Four slots absorb the storm — the
//! capture never showed more than three live connections plus one wedged —
//! and each costs its buffers below, so the number is a budget line, not a
//! shrug.
//!
//! # Timeouts, and the socket leak that motivates them
//!
//! `lib/microdot.py`'s one local modification (`:8-15`) is a 60 s
//! per-connection timeout, added because clients that opened a connection and
//! never completed a request — a browser's speculative preconnect, a phone
//! sleeping mid-request — parked their handler task forever and permanently
//! pinned one of lwIP's few sockets. They accumulated over days until inbound
//! connections were silently dropped while the rest of the firmware stayed
//! healthy. With four sockets instead of lwIP's several, this firmware would
//! reach that state sooner.
//!
//! picoserve's [`Timeouts`] cover the same failure in finer grain, and they are
//! set here rather than left at their defaults because the defaults are tuned
//! for a server on a wired LAN:
//!
//! | Timeout | Ours | Default | Why |
//! |---|--:|--:|---|
//! | `start_read_request` | 10 s | 5 s | The leak itself: a connection that never says anything. Ten seconds is longer than any real client's think time and four hundred times shorter than the failure. |
//! | `persistent_start_read_request` | 2 s | 1 s | Keep-alive idle. Bounds how long one client can hold a slot another needs. |
//! | `read_request` | 5 s | 3 s | A request that stops halfway. |
//! | `write` | 10 s | 1 s | **Per write call, not per response.** It fires when the client stops draining, and the SPA is a 54 KB body: on marginal Wi-Fi a phone can legitimately stall a window for longer than a second, and the default would abort the download of the page it was asking for. |
//!
//! Keep-alive is on, which picoserve's docs make conditional on having more
//! than one socket — we have four, and the 2 s idle bound is what keeps the
//! next client's wait short.

pub mod routes;
pub mod scratch;
pub mod spa;
pub mod status;

use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_time::Duration;
use picoserve::{Config, Server, Timeouts};

/// The port `main.py` served on, unchanged.
pub const PORT: u16 = 80;

/// Connections served at once. One task each; see the module docs.
pub const CONNECTIONS: usize = 4;

/// Per-connection TCP receive buffer.
///
/// Requests are small — the largest is a `PUT /api/config` whose body is the
/// whole configuration, about 1.3 KB — and this only has to hold segments in
/// flight, because picoserve copies out into [`HTTP_BUFFER_BYTES`].
const TCP_RX_BYTES: usize = 1536;

/// Per-connection TCP send buffer.
///
/// Two maximum segments. The SPA is 54 KB and this is the window the panel's
/// core-0 executor refills between frames; one segment measurably lengthens the
/// transfer, and four buys little because the client's window is the binding
/// constraint on a phone.
const TCP_TX_BYTES: usize = 2920;

/// Per-connection request buffer: the request line, the headers, and the body.
///
/// Sized by the largest body, which is a full-configuration `PUT` at about
/// 1.3 KB, plus a browser's header block — Chrome's `User-Agent`, `Accept`,
/// `sec-ch-ua-*` and cookies run past 1 KB on their own. 4 KB leaves room for
/// the configuration to grow a section without this becoming the reason a save
/// fails.
const HTTP_BUFFER_BYTES: usize = 4096;

/// Start the server. Called once provisioning has decided which mode won, so
/// [`crate::net::hosts`] already knows what to answer to.
pub fn start(spawner: Spawner, stack: Stack<'static>) {
    for id in 0..CONNECTIONS {
        // A pool of `CONNECTIONS`, so this can only fail if the loop bound and
        // the pool size disagree — a bug here, not a runtime condition.
        spawner.spawn(defmt::unwrap!(serve(id, stack)));
    }
}

#[embassy_executor::task(pool_size = CONNECTIONS)]
async fn serve(id: usize, stack: Stack<'static>) -> ! {
    // Each task builds its own router rather than sharing one out of a
    // `StaticCell`. The router is a tree of stateless handlers — every route
    // reads a global or the request, and none of them captures anything — so
    // the copies are function pointers, not duplicated state. Sharing one
    // would mean naming its type, and the type of a `Router` built by chained
    // `.route()` calls is only expressible as `impl PathRouter`, which cannot
    // be an associated type on stable (picoserve's own `AppBuilder` docs say
    // as much). This is the version that compiles without nightly.
    let app = routes::build();
    let config = Config::new(Timeouts {
        start_read_request: Duration::from_secs(10),
        persistent_start_read_request: Duration::from_secs(2),
        read_request: Duration::from_secs(5),
        write: Duration::from_secs(10),
    })
    // Four connections, so one client holding a connection open cannot lock
    // the others out for longer than the idle timeout above.
    .keep_connection_alive();

    // Task-local, so each connection's buffers live in its own arena rather
    // than in a shared static that would need a lock. BUDGET.md counts them
    // once per task.
    let mut tcp_rx = [0u8; TCP_RX_BYTES];
    let mut tcp_tx = [0u8; TCP_TX_BYTES];
    let mut http = [0u8; HTTP_BUFFER_BYTES];

    crate::debug!("http: serving on port {} (connection {})", PORT, id);

    loop {
        // `listen_and_serve` owns the accept loop and only returns on a
        // graceful shutdown, which is never signalled here. Rebuilding the
        // `Server` around the same buffers costs nothing and means a future
        // shutdown path has somewhere to land.
        Server::new(&app, &config, &mut http)
            .listen_and_serve(id, stack, PORT, &mut tcp_rx, &mut tcp_tx)
            .await;
    }
}

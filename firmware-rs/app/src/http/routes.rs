//! The REST surface, endpoint for endpoint against `api_routes.py`.
//!
//! Everything mounted under `/api` in MicroPython, plus `main.py`'s `GET /` and
//! its `/<path:path>` catch-all. [`Dispatch`] answers all of them from one
//! service that tests the path itself and falls off its end into the catch-all,
//! the way Microdot matched in registration order — not from a `.route()`
//! chain. picoserve's chain is a *type*: each link wraps the previous as its
//! fallback, and every link contributes its own frame to the "handle one
//! request" future, which every connection task materialises — three times
//! over, until the picoserve fork this crate pins fixed its internal select
//! duplication. See BUDGET.md, "A buffer in a picoserve handler costs its
//! size times the router's depth" and its 2026-08-17 addendum. Collapsing
//! the nine links to one measured 7,360 B off `.bss` (pre-fork) and 1,352 B
//! off `.text`; what it does not recover is the method router and request
//! handler beneath each route, which are picoserve's and stay one frame each.
//!
//! # `POST /api/check-update`, and the one thing it does differently
//!
//! `api_routes.py` answered this synchronously: the handler itself fetched the
//! manifest, blocking the asyncio loop for a second or two, and returned a
//! verdict. That is not available here and the reason is structural rather than
//! stylistic — the backend client and the display state are both owned by
//! [`crate::poller`] as task locals, and a handler has no path to either.
//!
//! So the handler *asks* and waits: [`crate::ota::request_check`] signals the
//! poll loop, which runs the check at the top of its next iteration and signals
//! the verdict back. The status strings the SPA reads are unchanged, and the
//! wait is bounded below the 25 s the settings page allows.
//!
//! The one case that cannot be answered this way is a device in setup mode,
//! where there is no poll loop to ask. `api_routes.py` had the same hole and
//! the same answer for it — `no_network`, which it returned when its OTA task
//! had not started.
//!
//! # `GET` and `PUT /api/timezone`
//!
//! The one route with no counterpart in `api_routes.py`: the browser tells the
//! device what timezone it is in, because nothing upstream can (BACKLOG 95,
//! SPEC §9's fourth key, and [`crate::timezone`]'s module docs for the whole
//! argument). The contract, in full:
//!
//! ```text
//! GET  /api/timezone  →  200 application/json
//! PUT  /api/timezone  ←  application/json  →  200 (the stored document)
//!
//! {
//!   "offset_minutes":           -360 | null,   // UTC offset now, minutes east
//!   "next_offset_minutes":      -300 | null,   // the offset after the change
//!   "transition_epoch_s": 1805270400 | null,   // when it changes, unix seconds
//!   "manual_offset_minutes":     330 | null,   // the override, if one is set
//!   "effective_offset_minutes": -360 | null    // GET only; see below
//! }
//! ```
//!
//! **Minutes east of UTC**, which is `-Date.prototype.getTimezoneOffset()` — the
//! browser's own unit, so nothing has to agree about a conversion. The device
//! converts to seconds once, at [`crate::timezone::offset_seconds_at`], because
//! seconds is what the display speaks.
//!
//! **`PUT` replaces; it does not patch.** Every absent field is an absent
//! value, so a body of `{}` clears the timezone entirely. That is the opposite
//! of `PUT /api/config` and deliberately so: the configuration is a large
//! document edited a section at a time by ten different cards, where patching
//! is the only workable shape; this is four numbers written by one page, where
//! patching would need "absent" and "null" to mean different things — a
//! distinction `serde` cannot express through `Option` without a custom
//! deserializer, and one that would exist purely to let a client clear an
//! override. Replacement makes the state after a `PUT` exactly the body of the
//! `PUT`, which is also what makes `GET`-then-`PUT` a safe way to change one
//! field. The SPA does exactly that.
//!
//! Validation is all-or-nothing, like `DeviceConfig::apply`: an offset outside
//! UTC−12:00..=UTC+14:00 is `400 invalid_offset`, and a half-specified
//! transition — or an instant that is not plausibly unix seconds, which is how
//! a client that posted milliseconds finds out — is `400 invalid_schedule`.
//! Neither changes anything.
//!
//! `effective_offset_minutes` is `GET`-only and derived: the offset the device
//! would use *right now*, after the override precedence and the transition
//! flip. It exists so the page can show what the device believes rather than
//! what the browser assumes, and the `PUT` parser ignores it — so a body read
//! from `GET` can be posted back unchanged.
//!
//! # The routes that touch flash
//!
//! `PUT /api/config` and `POST /api/reset-network` both end in [`persist`],
//! which is **one** write per request and stops the panel for its duration —
//! see that function. `PUT /api/timezone` is the third, with one difference
//! that matters: the SPA posts it on *every* page load, so it writes only when
//! the values actually changed ([`crate::timezone::apply`] holds that check).
//! The steady state is therefore a `PUT` that costs no flash and no frame.
//! Nothing else here reaches storage: `/api/logs/previous` serves a record read
//! once at boot.
//!
//! # `POST /api/induce-panic`
//!
//! Behind `--features induce-panic`, off in every shipped build. It exists for
//! one drill — panic, breadcrumb, reboot, read the breadcrumb back — which is
//! otherwise only reachable by finding a real bug, and a recovery path that has
//! never been exercised is a recovery path that does not work.

use core::fmt::Write as _;

use embassy_time::{Duration, with_timeout};
use picoserve::request::Path;
use picoserve::response::{IntoResponse, Response, StatusCode, chunked};
use picoserve::routing::{MethodHandler as _, get, post};
use picoserve::{ResponseSent, io};
use scoreboard_config::ConfigPatch;

use crate::http::scratch::{self, Lease};
use crate::http::{spa, status::Status};
use crate::{config, poller, ringlog, settings, supervise, timezone};

/// Build the router. See the module docs for why it is one layer deep.
pub fn build() -> picoserve::Router<impl picoserve::routing::PathRouter> {
    picoserve::Router::from_service(Dispatch)
}

/// The route table, as one path test per route.
///
/// The tests are picoserve's own: `Path`'s `PartialEq` is the comparison
/// `.route()` performs, percent-decoding as it walks and demanding that the
/// whole path be consumed — so `/api/config/` reaches [`catch_all`] here for
/// the same reason it always did.
///
/// What each route is handed to is picoserve's too: the `get`/`post` value that
/// `.route()` would have stored, invoked directly. Method dispatch, the `405`
/// body and `HEAD` answered by the `GET` handler with the body dropped all stay
/// picoserve's, which is the only way to have them — the body swap `HEAD` needs
/// goes through `Response`'s fields, and those are `pub(crate)`.
struct Dispatch;

impl<State> picoserve::routing::PathRouterService<State> for Dispatch {
    async fn call_path_router_service<R: io::Read, W: picoserve::response::ResponseWriter<Error = R::Error>>(
        &self,
        state: &State,
        _path_parameters: (),
        path: Path<'_>,
        request: picoserve::request::Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        if path == "/" {
            return get(index)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        if path == "/api/config" {
            return get(get_config)
                .put(put_config)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        if path == "/api/timezone" {
            return get(get_timezone)
                .put(put_timezone)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        if path == "/api/status" {
            return get(get_status)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        if path == "/api/logs" {
            return get(get_logs)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        if path == "/api/logs/previous" {
            return get(get_previous_log)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        if path == "/api/check-update" {
            return post(check_update)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        if path == "/api/reboot" {
            return post(reboot)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        if path == "/api/reset-network" {
            return post(reset_network)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        #[cfg(feature = "induce-panic")]
        if path == "/api/induce-panic" {
            return post(induce_panic)
                .call_method_handler(state, (), request, response_writer)
                .await;
        }
        catch_all(state, request, response_writer).await
    }
}

/// `POST /api/induce-panic` — panic on core 0, on purpose. See the module docs.
#[cfg(feature = "induce-panic")]
async fn induce_panic() -> impl IntoResponse {
    panic!("induced by POST /api/induce-panic");
    #[expect(unreachable_code, reason = "the panic is the whole handler")]
    ok_json("{}")
}

// ---------------------------------------------------------------------------
// Extractors
// ---------------------------------------------------------------------------

/// The `Host` header, and whether it is one of ours.
///
/// The whole captive portal turns on this one question — see
/// [`scoreboard_portal::MyHosts`] for the rules and the two deviations from
/// `main.py` that they encode.
struct HostCheck {
    mine: bool,
    /// Where a hijacked request is sent. `None` in station mode, where nothing
    /// is redirected.
    redirect_to: Option<heapless::String<15>>,
}

impl<'r, State> picoserve::extract::FromRequestParts<'r, State> for HostCheck {
    type Rejection = core::convert::Infallible;

    async fn from_request_parts(
        _state: &'r State,
        parts: &picoserve::request::RequestParts<'r>,
    ) -> Result<HostCheck, Self::Rejection> {
        let header = parts
            .headers()
            .get("Host")
            .and_then(|value| value.as_str().ok())
            .unwrap_or("");
        Ok(crate::net::hosts::with(|hosts| {
            let mut redirect_to = heapless::String::new();
            let _ = redirect_to.push_str(hosts.address());
            HostCheck {
                mine: hosts.is_mine(header),
                redirect_to: hosts.captive().then_some(redirect_to),
            }
        })
        // Before provisioning finishes nothing is listening, so this is
        // unreachable; treating it as "ours" means a request that somehow
        // arrives early is answered rather than bounced to an address that is
        // certainly not up.
        .unwrap_or(HostCheck {
            mine: true,
            redirect_to: None,
        }))
    }
}

impl HostCheck {
    /// `302` to the setup page, the response that turns a captive-portal probe
    /// into an open browser window.
    ///
    /// Owned rather than borrowing `self`, because a handler returns its
    /// response *after* its locals are gone.
    fn redirect(&self) -> Option<Redirect> {
        let target = self.redirect_to.as_ref()?;
        let mut location = heapless::String::new();
        let _ = location.push_str("http://");
        let _ = location.push_str(target);
        let _ = location.push_str("/#/setup");
        Some(Redirect { location })
    }
}

/// `302 Location: http://<address>/#/setup`, with an empty body.
struct Redirect {
    location: heapless::String<40>,
}

impl IntoResponse for Redirect {
    async fn write_to<R: io::Read, W: picoserve::response::ResponseWriter<Error = R::Error>>(
        self,
        connection: picoserve::response::Connection<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        Response::empty(StatusCode::FOUND)
            .with_header("Location", self.location.as_str())
            .write_to(connection, response_writer)
            .await
    }
}

/// `If-None-Match`, owned so it outlives the borrow of the request parts.
struct IfNoneMatch(Option<heapless::String<64>>);

impl<'r, State> picoserve::extract::FromRequestParts<'r, State> for IfNoneMatch {
    type Rejection = core::convert::Infallible;

    async fn from_request_parts(
        _state: &'r State,
        parts: &picoserve::request::RequestParts<'r>,
    ) -> Result<IfNoneMatch, Self::Rejection> {
        Ok(IfNoneMatch(
            parts
                .headers()
                .get("If-None-Match")
                .and_then(|value| value.as_str().ok())
                .and_then(|value| heapless::String::try_from(value).ok()),
        ))
    }
}

/// The `PUT /api/config` body, already parsed.
///
/// A dedicated extractor rather than `picoserve::extract::Json` or a raw
/// `&[u8]`, for two reasons. A borrowed slice cannot be a handler argument at
/// all — picoserve's handler impls need one concrete type per extractor, and
/// `&'r [u8]` is a different type per request. And `Json`'s rejection writes
/// picoserve's own error body, where this route owes the SPA a specific one.
/// So parsing happens here and **failure is data**, not a rejection: `None`
/// means "the body was not a configuration document" and the handler decides
/// what to say about it.
struct ConfigBody(Option<ConfigPatch>);

impl<'r, State> picoserve::extract::FromRequest<'r, State> for ConfigBody {
    // Infallible on purpose: a body that cannot be read is indistinguishable
    // to the client from one that cannot be parsed, and both deserve this
    // route's `400`, not picoserve's generic rejection page.
    type Rejection = core::convert::Infallible;

    async fn from_request<R: io::Read>(
        _state: &'r State,
        _parts: picoserve::request::RequestParts<'r>,
        body: picoserve::request::RequestBody<'r, R>,
    ) -> Result<ConfigBody, Self::Rejection> {
        Ok(ConfigBody(
            body.read_all()
                .await
                .ok()
                .and_then(|bytes| ConfigPatch::from_json(bytes).ok()),
        ))
    }
}

/// The `PUT /api/timezone` body, already parsed.
///
/// [`ConfigBody`]'s shape for [`ConfigBody`]'s reasons — a borrowed slice
/// cannot be a handler argument, and this route owes the SPA its own error body
/// rather than picoserve's. Validation is *not* done here: the extractor's job
/// ends at "this was JSON of the right shape", and the handler decides whether
/// the numbers in it are a timezone, because those two failures deserve
/// different error codes.
struct TimezoneBody(Option<timezone::Document>);

impl<'r, State> picoserve::extract::FromRequest<'r, State> for TimezoneBody {
    type Rejection = core::convert::Infallible;

    async fn from_request<R: io::Read>(
        _state: &'r State,
        _parts: picoserve::request::RequestParts<'r>,
        body: picoserve::request::RequestBody<'r, R>,
    ) -> Result<TimezoneBody, Self::Rejection> {
        Ok(TimezoneBody(body.read_all().await.ok().and_then(|bytes| {
            serde_json_core::from_slice::<timezone::Document>(bytes)
                .ok()
                .map(|(document, _)| document)
        })))
    }
}

/// `?since=<seq>`, the log stream's cursor.
struct Since(u32);

impl<'r, State> picoserve::extract::FromRequestParts<'r, State> for Since {
    type Rejection = core::convert::Infallible;

    async fn from_request_parts(
        _state: &'r State,
        parts: &picoserve::request::RequestParts<'r>,
    ) -> Result<Since, Self::Rejection> {
        // A `since` that is absent, empty or not a number is 0 — "send me
        // everything you have". `api_routes.py` swallowed the `ValueError` for
        // the same reason: a client with a corrupt cursor should resynchronise,
        // not get an error page instead of its logs.
        // `try_into_string` percent-decodes; a query longer than this is not one
        // we produced and resolves to "send everything", which is the same
        // answer a missing cursor gets.
        let decoded = parts
            .query()
            .and_then(|query| query.try_into_string::<128>().ok());
        let since = decoded
            .as_deref()
            .and_then(|query| {
                query
                    .split('&')
                    .filter_map(|pair| pair.split_once('='))
                    .find(|(key, _)| *key == "since")
                    .map(|(_, value)| value)
            })
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        Ok(Since(since))
    }
}

// ---------------------------------------------------------------------------
// The SPA and the catch-all
// ---------------------------------------------------------------------------

/// `GET /` — the settings app, or a redirect for a hijacked request.
async fn index(host: HostCheck, if_none_match: IfNoneMatch) -> impl IntoResponse {
    if !host.mine
        && let Some(redirect) = host.redirect()
    {
        return Either::A(redirect);
    }
    Either::B(spa::respond(
        if_none_match.0.as_deref(),
        config::with(|config| config.server.cache_max_age_seconds),
    ))
}

/// Everything that matched no route.
///
/// `main.py:544-558`: ours gets `404`, foreign gets `302`. The station-mode
/// half of that comparison is where `MyHosts` fixes a latent bug — MicroPython
/// built its host set from the AP interface, which does not exist in station
/// mode, so a request for an unknown path on a joined network was answered with
/// a redirect to 192.168.4.1, an address on a network the client is not on.
///
/// It answers a `HEAD` with a body, because it has never looked at the method
/// and the routed handlers above are where picoserve drops the body.
async fn catch_all<State, R: io::Read, W: picoserve::response::ResponseWriter<Error = R::Error>>(
    state: &State,
    request: picoserve::request::Request<'_, R>,
    response_writer: W,
) -> Result<ResponseSent, W::Error> {
    let host = picoserve::from_request_parts!(state, request, response_writer, HostCheck);

    let connection = request.body_connection.finalize().await?;
    match host.redirect() {
        Some(redirect) if !host.mine => redirect.write_to(connection, response_writer).await,
        _ => Response::new(StatusCode::NOT_FOUND, "Not found")
            .write_to(connection, response_writer)
            .await,
    }
}

// ---------------------------------------------------------------------------
// /api/config
// ---------------------------------------------------------------------------

/// `GET /api/config` — `config.raw`, the whole merged dictionary.
async fn get_config() -> impl IntoResponse {
    json(render_config())
}

/// Serialize the running configuration into a pooled buffer.
fn render_config() -> Option<JsonBody> {
    let mut lease = scratch::claim()?;
    let len = config::with(|config| config.to_json(lease.as_mut()).ok())?;
    Some(JsonBody { lease, len })
}

/// `PUT /api/config` — merge, validate, apply live.
///
/// The order matters and is `api_routes.py`'s: validate and merge first, then
/// re-apply, then echo the config back. A rejected request applies nothing at
/// all — see [`DeviceConfig::apply`], which checks the cadence against the
/// merged pair before it moves a single field.
async fn put_config(body: ConfigBody) -> impl IntoResponse {
    let patch = match body.0 {
        Some(patch) => patch,
        None => {
            crate::error!("api: PUT /api/config body was not a configuration document");
            return Either::A(error_response(
                StatusCode::BAD_REQUEST,
                r#"{"error":"invalid_json","message":"Body is not a configuration document"}"#,
            ));
        }
    };

    let outcome = config::with_mut(|config| -> Result<_, config::Rejection> {
        let applied = config.apply(&patch)?;
        // Built while the lock is held so the update describes exactly the
        // configuration that was just committed, not one a second request
        // could have replaced in between.
        let update = settings::DisplayUpdate::new(config, applied);
        let colors = applied.ui_colors.then(|| config.ui_colors());
        let level = applied.log_level.then(|| config.log_level());
        Ok((applied, update, colors, level))
    });

    let (applied, update, colors, level) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            crate::error!(
                "api: PUT /api/config rejected, poll {} s is not under rotation {} s",
                error.poll_interval_seconds,
                error.game_rotation_seconds
            );
            return Either::A(error_response(
                StatusCode::BAD_REQUEST,
                // `invalid_cadence` is the code the SPA's settings form keys
                // its inline error on; the message is for a human reading a
                // response body.
                r#"{"error":"invalid_cadence","message":"poll_interval_seconds must be < game_rotation_seconds"}"#,
            ));
        }
    };

    // Live-apply, outside the lock: none of these can be reached from a config
    // read, and holding a critical section across them would put the ring log's
    // lock inside the config lock.
    if applied.touches_core1() {
        settings::publish_display(update);
    }
    if let Some(colors) = colors {
        settings::publish_ui_colors(colors);
        // And nudge the snapshot's owner, because colours ride *in* the
        // snapshot: without this they would wait for the next commit, which on
        // an idle scoreboard is a poll interval away, on a screen the render
        // loop is skipping every frame. `update_ui_colors` wrote into a module
        // the renderers read directly and the change appeared on the next
        // frame; this is what keeps that promise across the core boundary.
        poller::command(poller::Command::ColorsChanged);
    }
    if let Some(level) = level {
        ringlog::set_level(level);
    }
    if applied.any() {
        crate::debug!("api: config updated, live-apply ran");
    }
    persist();

    Either::B(json(render_config()))
}

/// The single flash write `update_many` performed.
///
/// It is one call in one place on purpose. `config.py` was emphatic that a
/// batched update is **one** write — the flash on these boards is the part that
/// wears out, and the settings page sends a whole section per save. Both
/// callers above have already finished mutating before they reach this.
///
/// It is also the one place in the HTTP surface that **stops the panel**: a
/// flash program parks core 1 for its duration. Measured at one dropped frame
/// per save, which is why it happens once per request and not once per key.
fn persist() {
    if !config::persist() {
        crate::error!("api: the configuration was applied but not saved");
    }
}

// ---------------------------------------------------------------------------
// /api/timezone
// ---------------------------------------------------------------------------

/// `GET /api/timezone` — what the device believes, in a body it will accept
/// back. See the module docs for the contract.
async fn get_timezone() -> impl IntoResponse {
    json(render_timezone())
}

/// `PUT /api/timezone` — replace the schedule and the override together.
///
/// The order is `put_config`'s: parse, validate the whole document, and only
/// then commit. A rejected request changes nothing, including the fields in it
/// that were fine.
async fn put_timezone(body: TimezoneBody) -> impl IntoResponse {
    let Some(document) = body.0 else {
        crate::error!("api: PUT /api/timezone body was not a timezone document");
        return Either::A(error_response(
            StatusCode::BAD_REQUEST,
            r#"{"error":"invalid_json","message":"Body is not a timezone document"}"#,
        ));
    };

    let record = match document.into_record() {
        Ok(record) => record,
        Err(timezone::Invalid::Offset) => {
            crate::error!("api: PUT /api/timezone carried an offset outside UTC-12:00..UTC+14:00");
            return Either::A(error_response(
                StatusCode::BAD_REQUEST,
                r#"{"error":"invalid_offset","message":"Offsets must be between -720 and 840 minutes"}"#,
            ));
        }
        Err(timezone::Invalid::Schedule) => {
            crate::error!("api: PUT /api/timezone carried an incoherent schedule");
            return Either::A(error_response(
                StatusCode::BAD_REQUEST,
                r#"{"error":"invalid_schedule","message":"next_offset_minutes and transition_epoch_s (unix seconds) go together, and need an offset_minutes to transition from"}"#,
            ));
        }
    };

    // Writes flash only if this differs from what is stored — see
    // `timezone::apply`, and the module docs on why that check lives there.
    if !timezone::apply(record) {
        return Either::A(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"error":"storage_failed","message":"The timezone was not saved"}"#,
        ));
    }

    Either::B(json(render_timezone()))
}

/// Serialize the stored record into a pooled buffer.
///
/// Built by hand into a small stack string and copied, exactly as
/// [`render_check_answer`] does and for the same two reasons: a `serde`
/// serializer would want an owned struct threaded through picoserve's response
/// machinery to save nothing, and the string is a local of a plain `fn` so it
/// never lands inside a handler's future, where BUDGET.md's multiplier would
/// charge for it once per router layer.
fn render_timezone() -> Option<JsonBody> {
    let record = timezone::stored().unwrap_or_default();
    let schedule = record.schedule;
    let transition = schedule.and_then(|s| s.next);
    let mut body = TimezoneJson::new();

    body.push('{').ok()?;
    write_offset(&mut body, "offset_minutes", schedule.map(|s| s.offset_minutes))?;
    body.push(',').ok()?;
    write_offset(
        &mut body,
        "next_offset_minutes",
        transition.map(|next| next.offset_minutes),
    )?;
    match transition {
        Some(next) => write!(&mut body, r#","transition_epoch_s":{}"#, next.at_epoch_s).ok()?,
        None => body.push_str(r#","transition_epoch_s":null"#).ok()?,
    }
    body.push(',').ok()?;
    write_offset(&mut body, "manual_offset_minutes", record.manual_minutes)?;
    body.push(',').ok()?;
    // Derived, and `PUT` ignores it. It comes from the same accessor the
    // display will read rather than from the record above, which is the whole
    // point of the field: if the two ever disagreed, a value re-derived here
    // would hide exactly the bug this exists to show. Seconds are the display's
    // unit and minutes are the endpoint's, so the seam is crossed back here —
    // exactly, because every stored offset is a whole number of minutes.
    //
    // The device's clock is the other input, so before the first sync this is
    // the schedule's *current* offset. That is the honest answer: a transition
    // cannot have passed on a device that does not know what time it is.
    write_offset(
        &mut body,
        "effective_offset_minutes",
        timezone::offset_seconds_at(crate::net::timesync::local_clock().now_epoch_s)
            .map(|seconds| (seconds / 60) as i16),
    )?;
    body.push('}').ok()?;

    let mut lease = scratch::claim()?;
    let bytes = body.as_bytes();
    lease.as_mut().get_mut(..bytes.len())?.copy_from_slice(bytes);
    Some(JsonBody {
        lease,
        len: bytes.len(),
    })
}

/// The timezone document, before it is copied into a pooled slot.
///
/// 224 B against a widest rendering of 143 B — five numbers, five keys and the
/// braces, with `transition_epoch_s` at its full ten digits. Sized here rather
/// than borrowing a [`scratch`] slot for the assembly because the slot is 3 KB
/// and this is the whole response.
type TimezoneJson = heapless::String<224>;

/// `"<key>":<minutes|null>`. The `null` is load-bearing — it is how the SPA
/// tells "no override" from "an override of UTC+00:00".
fn write_offset(body: &mut TimezoneJson, key: &str, minutes: Option<i16>) -> Option<()> {
    match minutes {
        Some(minutes) => write!(body, r#""{key}":{minutes}"#).ok(),
        None => write!(body, r#""{key}":null"#).ok(),
    }
}

// ---------------------------------------------------------------------------
// /api/status, /api/logs
// ---------------------------------------------------------------------------

async fn get_status() -> impl IntoResponse {
    json(scratch::claim().and_then(|mut lease| {
        let len = Status::read().to_json(lease.as_mut()).ok()?;
        Some(JsonBody { lease, len })
    }))
}

/// `GET /api/logs?since=<seq>` — the RAM ring as NDJSON.
///
/// One `[seq, ts, level, msg]` array per line, oldest first, which the SPA's
/// logs page tail-follows by sending the last line's seq back as the next
/// `?since=`. `api_routes.py` streamed it from a sync generator specifically to
/// avoid building one large body on-device, and the same constraint applies
/// harder here: a full ring is tens of kilobytes and there is nowhere to put
/// it.
///
/// So the body is **chunked**, and each chunk is one pass over the ring. The
/// alternative — `Content-Length` — would mean either measuring the whole ring
/// first and racing another task's log line into the gap between measuring and
/// sending, or holding the ring's lock across every socket write, which is a
/// critical section held across an `await`.
async fn get_logs(since: Since) -> impl IntoResponse {
    chunked::ChunkedResponse::new(LogChunks {
        since: since.0,
        lease: scratch::claim(),
    })
}

struct LogChunks {
    since: u32,
    lease: Option<Lease>,
}

impl chunked::Chunks for LogChunks {
    fn content_type(&self) -> &'static str {
        "application/x-ndjson"
    }

    async fn write_chunks<W: io::Write>(
        mut self,
        mut writer: chunked::ChunkWriter<W>,
    ) -> Result<chunked::ChunksWritten, W::Error> {
        // No slot free means no buffer to render into, and an empty stream is
        // the right answer: the client's cursor does not move, so its next poll
        // picks up everything this one skipped.
        let Some(lease) = self.lease.as_mut() else {
            return writer.finalize().await;
        };
        let mut since = self.since;
        loop {
            // The ring's lock is taken and dropped inside this call, so the
            // write below never happens under a critical section.
            let rendered = ringlog::render_ndjson_since(since, lease.as_mut());
            if rendered.len > 0 {
                writer.write_chunk(&lease.as_slice()[..rendered.len]).await?;
            }
            if !rendered.more {
                break;
            }
            since = rendered.next_since;
        }
        writer.finalize().await
    }
}

/// `GET /api/logs/previous` — the crash breadcrumb, as plain text.
///
/// MicroPython rotated the whole ring to `previous.log` at every boot and served
/// the file; SPEC §9 replaces it with one record written when something dies
/// (see [`scoreboard_log::breadcrumb`]). Both are `text/plain` and both answer
/// `404` when there is nothing, which is what the SPA's `getPreviousLog` reads
/// — it takes `response.text()` and maps a `404` to `null`.
///
/// The record is read from flash once at boot and served from RAM, so opening
/// the logs page does not park core 1.
async fn get_previous_log() -> impl IntoResponse {
    let rendered = scratch::claim().and_then(|mut lease| {
        let len = supervise::render_previous_record(lease.as_mut())?;
        Some(TextBody { lease, len })
    });
    match rendered {
        Some(body) => Either::A(Response::ok(body)),
        None => Either::B(error_response(
            StatusCode::NOT_FOUND,
            r#"{"error":"not_found","message":"No previous-boot record"}"#,
        )),
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// How long the handler waits for the poll loop's verdict.
///
/// Under the SPA's own 25 s timeout, so a device that answers slowly still
/// answers rather than looking dead. The poll loop reaches the check at the top
/// of its next iteration, which is immediate when it is sleeping and up to one
/// request (15 s) when it is mid-poll.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// `POST /api/check-update` — ask the poll loop, and tell the page what it
/// said. See the module docs for why it is not answered here.
async fn check_update() -> impl IntoResponse {
    // No poller, no check. Reading the published network status is how this
    // knows: it is set by whichever provisioning arm won, and only the station
    // arm spawns a poll loop.
    let station = matches!(
        crate::net::status::read(),
        Some(crate::net::status::NetStatus::Station { .. })
    );
    if !station {
        return Either::A(ok_json(
            r#"{"status":"no_network","message":"The scoreboard is not on a network"}"#,
        ));
    }

    crate::ota::request_check();
    let Ok(answer) = with_timeout(CHECK_TIMEOUT, crate::ota::wait_for_answer()).await else {
        // Not an error: the check is still running and will finish. The SPA
        // already treats a dropped or slow response as "possibly updating" and
        // starts watching `app_version`, which is exactly the right behaviour
        // here — a download that started is a download that will finish.
        crate::debug!("api: check-update did not answer within {} s", CHECK_TIMEOUT.as_secs() as u32);
        return Either::A(ok_json(
            r#"{"status":"updating","message":"The check is still running"}"#,
        ));
    };

    Either::B(json(scratch::claim().and_then(|mut lease| {
        let len = render_check_answer(&answer, lease.as_mut())?;
        Some(JsonBody { lease, len })
    })))
}

/// `{"status": ..., "version": ..., "message": ...}`.
///
/// Built by hand rather than through serde: the three fields are two static
/// strings and one `&'static str`, and a derive would want an owned struct with
/// lifetimes threaded through picoserve's response machinery to save nothing.
fn render_check_answer(answer: &crate::ota::Answer, out: &mut [u8]) -> Option<usize> {
    let mut body = heapless::String::<192>::new();
    write!(
        &mut body,
        r#"{{"status":"{}","version":"{}""#,
        answer.status,
        crate::ota::VERSION
    )
    .ok()?;
    if let Some(message) = answer.message {
        write!(&mut body, r#","message":"{message}""#).ok()?;
    }
    body.push('}').ok()?;

    let bytes = body.as_bytes();
    out.get_mut(..bytes.len())?.copy_from_slice(bytes);
    Some(bytes.len())
}

/// `POST /api/reboot` — answer first, then reset.
///
/// The delay is `_delayed_reboot`'s and it exists for one reason: the response
/// has to reach the browser before the device stops existing. `api_routes.py`
/// spawned a task that slept a second, flushed the log to flash and called
/// `machine.reset()`; the flush has no counterpart yet (the ring is RAM, and
/// task #12 owns the breadcrumb), so the sleep is doing the whole job.
async fn reboot() -> impl IntoResponse {
    crate::debug!("api: reboot scheduled in 1 s");
    supervise::request_reboot();
    ok_json(r#"{"message":"Rebooting in 1 second..."}"#)
}

/// `POST /api/reset-network` — forget the credentials, stay connected.
///
/// `api_routes.py` cleared the stored SSID and password and said "reboot to
/// enter setup mode". It deliberately did **not** drop the live connection, and
/// neither does this: taking the link down here would kill the response before
/// it reached the browser that asked for it.
async fn reset_network() -> impl IntoResponse {
    config::with_mut(|config| -> Result<(), core::convert::Infallible> {
        config.network.ssid.clear();
        config.network.password.clear();
        Ok(())
    })
    .ok();
    crate::net::status::clear_credentials();
    persist();
    crate::debug!("api: network credentials cleared, setup mode on next boot");
    ok_json(r#"{"message":"Network configuration cleared. Reboot to enter setup mode."}"#)
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

/// A JSON body, or a `500` if it did not fit the buffer.
///
/// Truncated JSON would reach the SPA as a parse error with no explanation, so
/// an overflow says so in the status line instead. It is a build-time
/// impossibility — [`JSON_BYTES`] is sized for the largest response — which is
/// exactly why it should be loud if it ever happens.
fn json(body: Option<JsonBody>) -> impl IntoResponse {
    match body {
        Some(body) => Either::A(Response::ok(body)),
        None => {
            defmt::error!(
                "api: no response buffer free, or the body did not fit {} B",
                scratch::BYTES as u32
            );
            Either::B(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"error":"response_too_large"}"#,
            ))
        }
    }
}

fn error_response(status: StatusCode, body: &'static str) -> Response<impl picoserve::response::HeadersIter, impl picoserve::response::Body> {
    Response::new(status, StaticJson(body)).with_status_code(status)
}

fn ok_json(body: &'static str) -> Response<impl picoserve::response::HeadersIter, impl picoserve::response::Body> {
    Response::ok(StaticJson(body))
}

/// A JSON body that is a string literal — every error and acknowledgement here.
struct StaticJson(&'static str);

impl picoserve::response::Content for StaticJson {
    fn content_type(&self) -> &'static str {
        "application/json"
    }

    fn content_length(&self) -> usize {
        self.0.len()
    }

    async fn write_content<W: io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.0.as_bytes()).await
    }
}

/// A JSON body built at runtime, in a pooled buffer.
///
/// The [`Lease`] is eight bytes; the 3 KB it names lives in
/// [`scratch`](super::scratch), for the reason that module's docs give —
/// holding the array here instead cost 100 KB per server task.
struct JsonBody {
    lease: Lease,
    len: usize,
}

/// The same, as `text/plain`. `GET /api/logs/previous`'s only shape.
struct TextBody {
    lease: Lease,
    len: usize,
}

impl picoserve::response::Content for TextBody {
    fn content_type(&self) -> &'static str {
        "text/plain"
    }

    fn content_length(&self) -> usize {
        self.len
    }

    async fn write_content<W: io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(&self.lease.as_slice()[..self.len]).await
    }
}

impl picoserve::response::Content for JsonBody {
    fn content_type(&self) -> &'static str {
        "application/json"
    }

    fn content_length(&self) -> usize {
        self.len
    }

    async fn write_content<W: io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(&self.lease.as_slice()[..self.len]).await
    }
}

/// Two response types from one handler.
///
/// picoserve's handlers return `impl IntoResponse`, which is one type — and
/// several routes here legitimately answer with two shapes (a redirect or the
/// SPA; the config or a `400`). This is the standard either-way wrapper.
enum Either<A, B> {
    A(A),
    B(B),
}

impl<A: IntoResponse, B: IntoResponse> IntoResponse for Either<A, B> {
    async fn write_to<R: io::Read, W: picoserve::response::ResponseWriter<Error = R::Error>>(
        self,
        connection: picoserve::response::Connection<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        match self {
            Either::A(response) => response.write_to(connection, response_writer).await,
            Either::B(response) => response.write_to(connection, response_writer).await,
        }
    }
}

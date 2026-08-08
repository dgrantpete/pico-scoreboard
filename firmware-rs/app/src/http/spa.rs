//! The settings web app, embedded in the image.
//!
//! `main.py` looked for `/index.html.gz` on littlefs and fell back to
//! `/rom/index.html.gz` in ROMFS (`_find_index`, `:303-315`), then served
//! whichever it found with `send_file(compressed='gzip')`. SPEC §7.3 deletes
//! both paths along with the filesystem: the bundle is `include_bytes!`d, so it
//! is always present, always the one this image was built with, and the
//! "web bundle missing — redeploy the app" branch has nothing to report.
//!
//! # Served compressed, never decompressed
//!
//! The device has neither the RAM nor the reason to inflate 54 KB. It sends the
//! gzip with `Content-Encoding: gzip` and the browser does the work, exactly as
//! MicroPython's `compressed='gzip'` did. There is no identity-encoded copy to
//! fall back to, and no client since IE6 has needed one — a request without
//! `Accept-Encoding: gzip` still gets the gzip, which is the same bet `send_file`
//! made.
//!
//! # Caching
//!
//! Two headers, both `main.py`'s. The `ETag` is a build-time hash (see
//! `build.rs`), so a client that already has this exact bundle gets a `304` and
//! the 54 KB stays home. `Cache-Control: max-age` comes from
//! `config.server.cache_max_age_seconds` and is **omitted entirely at zero**,
//! which is how the config documents "no caching" and how `main.py` spelled it
//! (`if config.cache_max_age_seconds > 0`).

use picoserve::response::{Content, IntoResponse, Response, StatusCode};
use scoreboard_portal::conditional;

/// The bundle. Regenerated from `frontend/` — see `assets/README.md`.
pub const BUNDLE: &[u8] = include_bytes!("../../assets/index.html.gz");

/// First 8 bytes of the bundle's SHA-1, as 16 lowercase hex characters.
///
/// `main.py:319-336`'s digest, computed by `build.rs` instead of at boot.
pub const ETAG: &str = env!("SPA_ETAG");

/// The gzip, with the content type of what it decompresses to.
///
/// `Content-Type` describes the *representation*, not the encoding — a gzipped
/// HTML document is `text/html` with `Content-Encoding: gzip`, not
/// `application/gzip`. Getting this backwards makes the browser offer to
/// download the settings page instead of rendering it.
struct Bundle;

impl Content for Bundle {
    fn content_type(&self) -> &'static str {
        "text/html; charset=utf-8"
    }

    fn content_length(&self) -> usize {
        BUNDLE.len()
    }

    async fn write_content<W: picoserve::io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(BUNDLE).await
    }
}

/// Serve the bundle, or `304` if the client already has it.
///
/// `cache_max_age_seconds` is read by the caller rather than in here, so this
/// stays a pure function of its arguments and the route keeps the config lock
/// in one place.
pub fn respond(if_none_match: Option<&str>, cache_max_age_seconds: u32) -> SpaResponse {
    let mut quoted = [0u8; conditional::ETAG_HEADER_LEN];
    // `ETAG_HEADER_LEN` is sized for exactly this tag, so the `None` arm is
    // unreachable; falling back to the bare tag rather than panicking keeps a
    // build-time mistake from taking the settings page down.
    let etag = conditional::quoted(ETAG, &mut quoted).unwrap_or(ETAG);

    let fresh = if_none_match.is_some_and(|header| conditional::if_none_match(header, ETAG));

    // `Cache-Control` is built into a small buffer because the max-age is a
    // runtime value; `heapless` rather than `format_args!` so the string
    // outlives the expression that made it.
    let mut cache_control = heapless::String::<32>::new();
    if cache_max_age_seconds > 0 {
        use core::fmt::Write as _;
        let _ = write!(&mut cache_control, "max-age={cache_max_age_seconds}");
    }

    SpaResponse {
        fresh,
        etag: heapless::String::try_from(etag).unwrap_or_default(),
        cache_control,
    }
}

pub struct SpaResponse {
    fresh: bool,
    etag: heapless::String<{ conditional::ETAG_HEADER_LEN }>,
    cache_control: heapless::String<32>,
}

impl IntoResponse for SpaResponse {
    async fn write_to<R: picoserve::io::Read, W: picoserve::response::ResponseWriter<Error = R::Error>>(
        self,
        connection: picoserve::response::Connection<'_, R>,
        response_writer: W,
    ) -> Result<picoserve::ResponseSent, W::Error> {
        // A 304 carries the validator and the caching headers and no body —
        // RFC 9110 §15.4.5. `main.py` sent the ETag alone; adding
        // `Cache-Control` is what lets a client refresh its freshness lifetime
        // without another round trip.
        let headers = [
            ("ETag", self.etag.as_str()),
            ("Cache-Control", self.cache_control.as_str()),
        ];
        // An empty `Cache-Control` would be a malformed header rather than an
        // absent one, so zero max-age drops the pair entirely.
        let headers = &headers[..if self.cache_control.is_empty() { 1 } else { 2 }];

        if self.fresh {
            return Response::empty(StatusCode::NOT_MODIFIED)
                .with_headers(headers)
                .write_to(connection, response_writer)
                .await;
        }

        Response::ok(Bundle)
            .with_headers(headers)
            .with_header("Content-Encoding", "gzip")
            .write_to(connection, response_writer)
            .await
    }
}

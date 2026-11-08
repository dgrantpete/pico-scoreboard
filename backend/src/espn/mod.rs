//! ESPN integration layer: HTTP client with caching, plus the shared
//! plumbing between the `scoreboard-espn` streaming extractors and the
//! backend's handlers (quirk → tracing bridge, 404-vs-502 verdicts). The
//! transform itself lives in `crates/scoreboard-espn`.

pub(crate) mod adapt;
mod client;
pub mod league;
pub(crate) mod types;

pub use client::EspnClient;

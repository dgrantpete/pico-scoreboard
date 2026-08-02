//! ESPN integration layer: HTTP client with caching. Sport-specific
//! deserialization types live in each sport's module; this layer owns
//! transport, caching, and the uniform deserialize-with-logging choke point.

mod client;
pub mod league;
pub(crate) mod types;

pub use client::EspnClient;

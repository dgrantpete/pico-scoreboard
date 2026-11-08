//! Shared ESPN-JSON → game-model transform.
//!
//! One `no_std` implementation of the ESPN transform, which the backend runs
//! for every sport. It was built `no_std` in Phase S1 so the firmware's
//! direct feed could share it; that feed was deleted on 2026-11-08, and the
//! backend is the only consumer. The ESPN-specific knowledge lives in one
//! declarative const path table per sport; a small streaming engine
//! (`path`) drives them from picojson's push events.
//!
//! The correctness contract is byte-identical wire output against the
//! backend's existing pipeline over the full fixture corpus; DESIGN.md in
//! this crate's root carries the architecture and the decisions.
#![no_std]

pub mod common;
pub mod football;
pub mod hockey;
pub mod mlb;
pub mod nba;
pub mod path;
pub mod soccer;

pub use path::{ContainerKind, Directive, Error, Pattern, Seg, Sink, StreamMatcher, Value};

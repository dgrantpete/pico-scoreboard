//! Shared ESPN-JSON → game-model transform (Phase S1).
//!
//! One `no_std` implementation of the ESPN transform, shared by the backend
//! and the firmware's direct feed — the `scoreboard-wire` move applied to
//! the other half of the proxy. The ESPN-specific knowledge lives in one
//! declarative const path table per sport; a small streaming engine
//! (`path`) drives them from picojson's push events.
//!
//! The correctness contract is byte-identical wire output against the
//! backend's existing pipeline over the full fixture corpus; DESIGN.md in
//! this crate's root carries the architecture and the decisions.
#![no_std]

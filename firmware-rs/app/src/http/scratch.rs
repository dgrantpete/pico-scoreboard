//! Response buffers, held in a pool rather than in a handler's future.
//!
//! # Why this exists — a buffer in a future is not a buffer
//!
//! The obvious way to build a JSON response is a `heapless::Vec` local to the
//! handler. It is caller-owned, it is bounded, and on any normal target it
//! costs what it says. Here it cost **22 times** what it says.
//!
//! picoserve's router is a *type*: each `.route()` wraps the previous one as
//! its fallback, so nine routes are nine layers of nested generics, and the
//! future for "handle a request" contains, at every layer, the future for that
//! layer's handler *and* the future for the whole fallback chain beneath it.
//! Any buffer inside a handler's future is therefore instantiated once per
//! layer it appears under. Measured on this router: raising the response buffer
//! from 256 B to 3,072 B and the log chunk from 256 B to 2,048 B grew the two
//! server tasks' arenas by **202,752 B** — 4,608 B of buffer, multiplied.
//!
//! So the buffers live here, in a pool of one per connection, and a handler's
//! future holds a [`Lease`] — an index and a length, eight bytes. The pool is
//! 2 × 3,072 B of `.bss` that BUDGET.md can point at, instead of a number that
//! moves when a route is added.
//!
//! # Why a pool and not one buffer per task
//!
//! Per-task would be better and is not reachable: the two server tasks share
//! one `Router` value, handlers are plain functions with no task identity, and
//! picoserve's `State` is shared across connections by construction. A pool
//! sized to the number of connections is the same allocation with a claim step
//! — and the claim cannot fail in practice, because there is one slot per
//! connection and a connection handles one request at a time.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use super::CONNECTIONS;

/// One slot's size, set by the largest thing that goes in one.
///
/// That is `GET /api/config` — the whole merged configuration, about 1.3 KB
/// with every league slot full. 3 KB leaves the configuration room to grow a
/// section. The log stream's chunk shares these slots (a handler does one or
/// the other, never both) and needs only [`scoreboard_log::MAX_LINE`], so this
/// bound is the configuration's.
pub const BYTES: usize = 3072;

struct Slot {
    taken: AtomicBool,
    bytes: UnsafeCell<[u8; BYTES]>,
}

// SAFETY: `bytes` is only ever reached through a `Lease`, and a `Lease` is only
// ever created by winning the `taken` compare-exchange — so at most one
// reference to a given slot exists at a time, and it is released in `Drop`.
unsafe impl Sync for Slot {}

#[expect(
    clippy::declare_interior_mutable_const,
    reason = "the const is an array initialiser, not a shared value: each \
              repetition constructs a distinct slot"
)]
const FREE: Slot = Slot {
    taken: AtomicBool::new(false),
    bytes: UnsafeCell::new([0; BYTES]),
};

static SLOTS: [Slot; CONNECTIONS] = [FREE; CONNECTIONS];

/// Exclusive use of one slot, released when dropped.
pub struct Lease {
    index: usize,
}

/// Take a slot, or `None` if every one is in use.
///
/// Unreachable in practice — see the module docs — but a `None` that turns into
/// a `503` is a better answer than one that panics, because "every buffer is
/// busy" is a load condition and not a bug.
pub fn claim() -> Option<Lease> {
    SLOTS
        .iter()
        .position(|slot| {
            slot.taken
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        })
        .map(|index| Lease { index })
}

impl Lease {
    pub fn as_mut(&mut self) -> &mut [u8; BYTES] {
        // SAFETY: this lease won the slot's flag and has not been dropped, so
        // no other reference to these bytes exists.
        unsafe { &mut *SLOTS[self.index].bytes.get() }
    }

    pub fn as_slice(&self) -> &[u8; BYTES] {
        // SAFETY: as above.
        unsafe { &*SLOTS[self.index].bytes.get() }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        SLOTS[self.index].taken.store(false, Ordering::Release);
    }
}

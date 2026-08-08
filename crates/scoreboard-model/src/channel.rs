//! The core-0 → core-1 snapshot handoff.
//!
//! # Why three buffers
//!
//! SPEC §4 describes a double buffer: core 0 fills the inactive one and
//! publishes by storing its index. That is one buffer short. Core 1 latches an
//! index at the top of a frame and reads it for the whole 50 ms frame, so the
//! buffer it latched stays live *after* core 0 has published a newer one — and
//! core 0's next publish would target exactly that buffer. Two commits inside
//! one frame is not a corner case: every live commit is followed immediately
//! by the play-flash commit.
//!
//! Three is provably enough for one writer and one reader. At any moment one
//! buffer is published, one may be latched, and the writer takes whichever is
//! neither. That is what the MicroPython `TripleBufferedState` does, with a
//! lock around the index bookkeeping; the swap protocol below reaches the same
//! guarantee with a single atomic and no lock, so core 0 never waits on core 1.
//!
//! # The protocol
//!
//! Each of the three slots is owned by exactly one party at a time: the
//! publisher holds `back`, the reader holds `front`, and the shared cell holds
//! the third. Publishing swaps `back` into the shared cell; latching swaps
//! `front` out of it. A swap moves ownership atomically, so no slot is ever
//! reachable from both sides.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::snapshot::ScoreboardSnapshot;

const INDEX_MASK: u8 = 0b11;
/// Set by a publish, cleared by a latch: lets the reader skip the swap — and
/// the acquire fence — when nothing has been published since its last frame.
const FRESH: u8 = 0b100;

/// The shared slots. Lives in a `static` (`StaticCell` or a plain `static`);
/// [`SnapshotChannel::split`] hands out the two halves once.
pub struct SnapshotChannel {
    slots: [UnsafeCell<ScoreboardSnapshot>; 3],
    shared: AtomicU8,
    split: AtomicU8,
}

// SAFETY: every slot is reachable through exactly one of `Publisher` (which is
// `!Sync` and holds `back`), `Reader` (holds `front`), or the `shared` cell,
// and index ownership only moves by atomic swap. The two halves are handed out
// once, so no third party can alias a slot.
unsafe impl Sync for SnapshotChannel {}

impl SnapshotChannel {
    pub const fn new() -> Self {
        Self {
            slots: [
                UnsafeCell::new(ScoreboardSnapshot::new()),
                UnsafeCell::new(ScoreboardSnapshot::new()),
                UnsafeCell::new(ScoreboardSnapshot::new()),
            ],
            // Slot 0 is published, slot 1 goes to the reader, slot 2 to the
            // publisher.
            shared: AtomicU8::new(0),
            split: AtomicU8::new(0),
        }
    }

    /// Take the publisher and reader halves. Panics on a second call: two
    /// publishers would both claim `back`.
    pub fn split(&self) -> (Publisher<'_>, Reader<'_>) {
        assert_eq!(
            self.split.swap(1, Ordering::Relaxed),
            0,
            "SnapshotChannel::split called twice"
        );
        (
            Publisher {
                channel: self,
                back: 2,
            },
            Reader {
                channel: self,
                front: 1,
            },
        )
    }

    /// Total RAM the handoff costs: three snapshots plus two bytes of index.
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

impl Default for SnapshotChannel {
    fn default() -> Self {
        Self::new()
    }
}

/// Core 0's half.
pub struct Publisher<'a> {
    channel: &'a SnapshotChannel,
    back: u8,
}

impl Publisher<'_> {
    /// Make `snapshot` the state core 1 renders from its next frame on.
    pub fn publish(&mut self, snapshot: &ScoreboardSnapshot) {
        // SAFETY: `back` is this publisher's alone until the swap below hands
        // it over.
        unsafe { &mut *self.channel.slots[self.back as usize].get() }.clone_from(snapshot);
        let previous = self
            .channel
            .shared
            .swap(self.back | FRESH, Ordering::AcqRel);
        self.back = previous & INDEX_MASK;
    }
}

/// Core 1's half.
pub struct Reader<'a> {
    channel: &'a SnapshotChannel,
    front: u8,
}

impl Reader<'_> {
    /// Latch the newest published snapshot for this frame.
    ///
    /// The returned reference stays valid until the next call, which is the
    /// whole point: a frame renders from one consistent state even while core 0
    /// publishes underneath it.
    pub fn latch(&mut self) -> &ScoreboardSnapshot {
        if self.channel.shared.load(Ordering::Relaxed) & FRESH != 0 {
            let published = self.channel.shared.swap(self.front, Ordering::AcqRel);
            self.front = published & INDEX_MASK;
        }
        // SAFETY: `front` is this reader's alone until the next swap, and the
        // acquire half of that swap orders the publisher's writes before it.
        unsafe { &*self.channel.slots[self.front as usize].get() }
    }
}

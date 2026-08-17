# RAM Budget

The living version of SPEC.md §11. Seeded from the spec's estimates; lines get
replaced by measured values as each phase lands.

**Rule: any PR that adds a static ≥ 1 KB updates this table in the same PR.**

**Target: ≥ 40 % headroom** against the RP2350's 520 KiB (532,480 B) of SRAM —
a ceiling of 319,488 B for statics + stacks combined.

Every line says whether it is **MEASURED** (read out of a real ELF, with the
command that produced it below) or **ESTIMATE** (SPEC §11's seed, unverified).
No allocator exists, so this table plus the stacks *is* the RAM story — there is
no heap to absorb a mistake.

---

## The table

| Item | Bytes | Status | Notes |
|---|---:|---|---|
| HUB75 BCM framebuffers ×2 + driver statics | 65,609 | **MEASURED** | `crates/hub75`. 2 × 32,768 B bitplane buffers + 64 B timing stream + 8 B DMA pointer words + 1 B construction guard. See the breakdown below. |
| `Hub75Display` RGB565 frame buffer | 16,384 | **MEASURED** | 128 × 64 × 2 B. App-owned, not a driver static — the driver never allocates the drawing surface. Costs 16,385 B as linked (`ConstStaticCell` appends a 1 B taken flag). |
| Gamma LUT + fused pack tables | — | **MEASURED, inside the core-1 line** | The 256 B `[u8; 256]` and task #20's 1 KiB `FusedTables` both live **inside `Hub75Driver`**, not in statics of their own, so they land in whichever arena owns the driver. That is the core-1 render-loop arena below, and this row is a pointer rather than a number so they are not counted twice. |
| Snapshot handoff — `SnapshotChannel` | 8,552 | **MEASURED** | `crates/scoreboard-model`. Three `ScoreboardSnapshot` slots of 2,848 B + 2 B of index + padding. Three, not two — see the correction below. |
| `Store` (core-0 authoritative state) | 2,888 | **MEASURED** | `scoreboard-app`'s `STORE`. One more snapshot plus the startup flag and the soccer stale-clock guard, plus `StaticCell`'s 1 B taken flag and padding. A `static` rather than a task local because it is *lent* to `net::bringup` for the boot screen and then moves to the poller — see the state-sharing note below. |
| Poller task arena | 16,096 | **MEASURED** | `scoreboard-app`, tasks #11 and #18. The `Slate` (4,596 B — 160 entries × (source, state, 20-byte game id), the rotation order and 8 league descriptors, sized for a college-football Saturday alongside a full MLB slate), the 4,096 B receive buffer, eight per-source ETags, the crest directory's 32 keys and LRU, the warm index's 16 game records, and the futures of a whole tick: a detail fetch with two crest fetches nested inside it, each carrying its own 256 B URL. It grew 2,184 B for the 32-slot pool — 1,536 B of it is the directory's key array, which is 32 × `Option<CrestKey>` at 41 bytes plus a `usize` length. |
| Crest pool (core 1) + its channel | 39,201 | **MEASURED** | `CRESTS` 36,865 B — thirty-two 24×24 RGB565 slots, owned by core 1 — plus `logos::UPDATES` 2,336 B, the two-deep channel core 0 ships new crest pixels over. The channel does not scale with the pool: it carries one slot's pixels at a time and two of them is still enough. See the two notes below — one on why the pool is split across the cores (the alternative costs more *and* tears), one on why thirty-two is the number. |
| Poller TCP socket | 2,050 | **MEASURED** | `api_client::TCP_STATE`: one connection, 1,536 B receive + 512 B send. One, because a request is never concurrent with another — the client takes `&mut self`. |
| cyw43 driver state | 17,928 | **MEASURED** | `CYW43_STATE` 12,696 B (the driver's ioctl state and its 4 + 4 packet channel) plus the runner task's 5,232 B arena. Came in 9 % over SPEC §11's 16 KB guess. |
| embassy-net stack + captive-portal sockets | 13,892 | **MEASURED** | `StackResources<10>` **5,336 B** + the net runner's 136 B arena, plus the two AP-mode responders: 4,100 B of payload buffers, 400 B of packet metadata, and 3,920 B of task arenas (the DHCP and DNS scratch live in the arenas, not on a stack). The socket table was 4,584 B at `StackResources<8>`; 10 is the shipped size, covering an eight-socket worst case with two spare so a new consumer is a budget line and not a rewrite. **The TCP socket buffers are not in this line** — poller and HTTP ×4 bring their own, and OTA reuses the poller's. |
| mDNS responder (`net::mdns`) | 4,026 | **MEASURED** | Phase 4's `<device_name>.local` responder: a 1,776 B task arena, 1,537 B + 513 B of UDP payload buffers, and 200 B of packet metadata. It runs in both station and setup mode. Its socket cost nothing — it took the slot §7.1 had reserved for OTA. |
| Provisioning (`net::bringup` arena) | 1,656 | **MEASURED** | The scan's 64-entry BSSID table and the credentials. It was 4,400 B until the poller landed: the boot-time `Store` was a local of this task, and making it a `static` that bringup borrows removed 2,872 B rather than paying for a second copy in the poller's arena. Task-arena, so it is statically allocated but has no reader after the boot. |
| HTTP server — sockets, request buffers, task arenas | 100,612 | **MEASURED** | `scoreboard-app`, task #10, **re-measured at four connections (108bc85, drill day)**. 88,320 B of task arena — 22,080 each for four connections (1,536 B TCP receive + 2,920 B TCP send + 4,096 B request buffer, and picoserve's request-handling future around them) — plus 12,292 B of pooled response scratch, one 3,072 B slot per connection. It was 50,114 B at two connections. **The arena figure is dominated by the future, not the buffers** — see the multiplier note below before adding a route, and the four-connections note for why the count is what it is. |
| Receive/scratch buffers (OTA chunk) | 0 | **MEASURED** | The last of SPEC §11's 40,960 B guess, and it turned out to be nothing. An update is a *phase of the poll loop*, so the poller's 4,096 B receive buffer is idle for the whole download and splits into a 2,048 B header half and a 2,048 B chunk half; the `BlockingFirmwareUpdater`'s own aligned buffer is **one byte**, on the stack. See "The download costs zero RAM" below. |
| Glyph/font tables + compiled sprites | 0 | **MEASURED** | `crates/scoreboard-render`. **9,538 B of flash, 0 B of RAM** — read out of the crate's own object code, not asserted. Breakdown and the command below. |
| Core-1 render-loop task arena | 3,448 | **MEASURED** | `scoreboard-app`. The loop's `LoopState` — frame rail, prepared view, skip memo, probe — plus the display and everything else held across the frame's await. It lives in the **task arena, not on the core-1 stack**: an embassy task's future is a static. See the correction below. The crest pool it now owns is a `static` it borrows, so it is the line above and not this one. It was 1,384 B until task #20's repack: the driver travels in this arena, so `hub75::packing::FusedTables` (1 KiB) and its alignment came here rather than to a `static` of their own. |
| Input, brightness and watchdog task arenas | 888 | **MEASURED** | `scoreboard-app`, task #12. `supervise::watchdog` 360 B, `inputs::run` 264 B, `brightness::auto_brightness` 264 B. All three are small because the state that could have been large is not theirs: the button fold is two 24 B structs, the brightness chain is three floats, and the watchdog holds a counter. The 3 KB storage buffer is a **stack** local of a plain `fn`, deliberately — see the note below. |
| Crash breadcrumb — served copy | 228 | **MEASURED** | `supervise::PREVIOUS`, the decoded copy `/api/logs/previous` serves so that opening the logs page does not park core 1 to read flash. **The 240 B RAM cell is no longer in this budget**: since drill day it lives at `scoreboard_layout::BREADCRUMB_BASE`, in the top 256 B that no profile's linker map contains — reserved, not allocated. See the caveat at the end of this file. |
| Storage map | ~32 | **MEASURED** | `sequential-storage`'s `MapStorage` over an `Uncached` cache: a flash range, a `PhantomData`, and embassy-rp's `Flash` (which is itself a `PhantomData` plus an optional DMA channel). Under the 512 B reporting floor. The 3 KB scratch every operation needs is a stack local. |
| Core-0 task arenas (remaining) | 240 | **MEASURED** | The last estimate in this table is gone, and it was never spent: there is no OTA *task*. An update is a phase of the poll loop and lives in the poller's arena. What remains is `supervise::liveness` 104 B, `net::watch_link` 72 B and `supervise::reboot_on_request` 64 B. |
| Core-0 stack | — | **MEASURED (not a fixed line)** | With flip-link the stack is the whole remainder below the statics: **186,304 B** today (`_stack_start = 0x2002_d7c0`), growing *down*, guarded by MSPLIM at the bottom of RAM. It is not a number to budget; it is what the rest of the table does not spend, which is why it shrinks every time a static grows — it read 266,536 B before drill day added 50 KB of HTTP connections. The last published high-water is **25,816 B**, taken against the 266,536 B stack (9.7 % then, 13.9 % of what is left now) over a run that included a config save, whose 3 KB buffer is a stack local. **Not re-taken since drill day**; the depth should not have moved, but the denominator did. |
| Core-1 stack | 8,192 | **MEASURED** | Sized 8 KB by `scoreboard-app`; **high-water 3,348 B (41 %)** over a run of real backend data — up from 2,480 B under the demo, because the demo never drove the pregame screen. The setup screen, which is the only QR encoder caller and therefore the deepest frame, is still not among them. Guarded by MSPLIM at its bottom. See the core-1 notes below. |
| Ring log | 28,812 | **MEASURED** | `crates/scoreboard-log`, task #10. 200 slots × 144 B — a `u32` sequence, a `u32` timestamp, a level byte and a 128-byte bounded message, padded. SPEC §11's ~8 KB guess assumed a shorter message; 128 B was chosen against the measured distribution of the 87 log call sites in `firmware/src` (median 43 B, max 116 B), where 64 B would have truncated real lines. It is `.bss`, not `.data` — see the note below. |
| Misc statics | ~2,900 | **MEASURED** | embassy-rp's GPIO/DMA/PIO wakers, the time driver, the critical-section lock, the clock cache, the PAC singleton flags, task #10's small publishers (`net::hosts`, `net::status`, the config cell, the stack-watermark atomics), the live display settings, and — new with task #20 — `hub75::packing::pack_rgb565`'s 562 B, which is code that lives in `.data` because the pack loop runs from RAM rather than XIP. |
| **Measured total** | **337,712** | | **329.8 KiB** — read out of a real ELF, not summed from this table; see the breakdown below |

**Headroom: 36.6 %** (194,768 B free of 532,480 B). Against the 524,032 B the
linker actually declares — see the caveat below — it is 35.6 %.

**That is under the ≥ 40 % target, and it is the first time this file has had to
say so.** Statics alone are 18,224 B past the 319,488 B ceiling. The cause is
one line and it is not a surprise: the HTTP server went from two connections to
four on drill day and grew 50,498 B doing it, because picoserve's arena is
mostly *future* and each connection instantiates the whole router. That is
44 KB of the 53 KB the projection rose by since the last measurement.

The honest reading is that the target has been spent, not blown: nothing is
starved — core 0 still has 186,304 B of stack against a 25,816 B high-water, and
the panel, the poller and the crest pool are untouched — but the ≥ 40 % cushion
that existed to absorb the *next* thing is gone. Two levers are already written
down and priced. The cheap one is the fourth snapshot copy: folding `Store` into
the channel's back buffer recovers 2,848 B (see the §4 correction). The large
one is the crest pool, at 27,648 B for the last twenty-four slots. Neither
should be pulled on this measurement alone; what should happen first is the
decision the four-connection change deferred — whether 3,072 B of response
scratch per connection is the right slot size now that there are four of them.
Anything that wants a five-figure static before then has to say which of these
it is displacing.

The projection rose by 29,872 B for the 32-slot crest pool, and unlike every
other rise in this file it bought no new capability — the firmware fetched and
drew crests before. It bought a *property*: after one lap of a normal slate the
board fetches no logos at all. The section below argues why that was worth 5.6
points of headroom, and prices the doubling that would not fit.

The projection rose by 26,159 B when the poller landed, and 91 % of that is
three measured lines that had no estimate at all: the poller's 13,824 B task
arena, the 11,553 B crest pool and channel, and its 2,050 B socket. What it
replaced was mostly guesswork — the receive-buffer estimate lost the poller's
share (which turned out to be one 4 KB buffer, inside the arena) and the
core-0-arena estimate lost everything but tasks #12 and #15.

The projection rose by 34,014 B when the HTTP server landed, which is less than
it looks: 50,114 B of it is the server, measured, against the 40,960 B estimate
that was carrying the HTTP share all along, and the ring log's 28,812 B replaced
an 8,192 B guess that had assumed 200-character messages would cost less than
they do.

The projection *fell* by 42,520 B when Phase 3's networking landed, because two
of SPEC §11's three network guesses were high. The stack and its AP-mode
sockets measured 12,868 B against a 49,152 B estimate — the estimate was
counting TCP socket buffers for the poller, the HTTP server and OTA. Two of the
three have now landed and cost 2,050 B and 8,912 B respectively.

`SnapshotChannel`'s 8,552 B, `Store`'s 2,888 B and `Slate`'s 4,596 B are now
symbols in a real device ELF, and all three came out byte-for-byte as the host
predicted. That is the `u16` length prefix on every bounded string doing its
job: the layout is identical on the host and on `thumbv8m.main-none-eabihf`, so
`scoreboard-model`'s own budget test is a real check and not a coincidence.

**Measured today: 337,712 B** (329.8 KiB, 36.6 % headroom) — the whole of
`scoreboard-app`, **`link-boot-integrated`**, which is now the binary of record
because it is the profile the production unit runs: `.data` 11,296 + `.bss`
322,320 + `.uninit` 4,096. The `.uninit` is dev-only defmt/RTT and leaves with
the probe build; the breadcrumb cell used to be in there too and is now outside
the linker map entirely.

Two things about that figure before it is compared with anything above it.
**It is a different profile** from the 284,260 B this line used to report, which
was `link-standalone` — boot-integrated links `embassy-boot-rp`, `sha2` and
`ed25519-dalek`, which is 73,744 B of *flash* and very little RAM, so the two
are close but not the same measurement, and the standalone figure has not been
re-taken. And **it is measured from the artifact left in the tree**, built
2026-08-16 at the four-connection change (108bc85), rather than from a fresh
`--locked` CI build. The symbol-by-symbol breakdown below reconciles to it, so
it is not a stray build; re-run the reproduce block at the next size-relevant PR.

The rise from 284,260 B is 53,452 B. The single largest contributor is the HTTP
server going from two connections to four — **50,498 B**, of which 44,352 B is
arena and 6,146 B is scratch — and the rest is spread across everything Phase 4
and task #20 added between the two measurements: the mDNS responder's 4,026 B,
the fused pack tables (~2,056 B in core 1's arena, 562 B in `.data`), the socket
table's 752 B for `StackResources<10>`, less the 240 B breadcrumb cell that left
the map and whatever the profile change is worth. Those do not sum to the delta
and are not meant to — the two measurements are different profiles a week apart.
The four-connection line is the one that is measured as a clean before/after,
and it is the one that matters.

**The 32-slot crest pool and its warmer are 29,832 B of that**, measured as a
clean before/after in a detached worktree so no other in-flight work was in the
tree:

| Symbol | 8 slots | 32 slots | Δ |
|---|---:|---:|---:|
| `scoreboard_app::CRESTS` | 9,217 | 36,865 | +27,648 |
| `scoreboard_app::poller::run::POOL` | 13,864 | 16,048 | +2,184 |
| `scoreboard_app::logos::UPDATES` | 2,336 | 2,336 | 0 |
| `.data` + `.bss` + `.uninit` | 254,428 | 284,260 | **+29,832** |

The two symbol deltas account for the image delta to the byte, which is the
check that nothing else moved: the pool array, the directory's keys and its LRU
all size from one `SLOTS` constant, `LogoRef` is a `u8` either way, and
`scoreboard-render` takes the pool as a slice it indexes with `get` — so no
renderer, no snapshot field and no test knew the number was eight. The channel
did not move because it carries one slot's pixels at a time.

(The 249,800 B this line read before is not a comparable baseline: it predates
work that is not this change. 254,428 B is the commit immediately before it,
measured the same way.)

**Storage, inputs, brightness and supervision added 1,588 B**, which is the
smallest any Phase 3 task has cost and worth one sentence on why: the two things
that could have been large are not statics. The map's 3 KB scratch is a stack
local of a plain `fn` — `storage`'s functions are blocking and are called from
async handlers, so the buffer lives on core 0's stack for one call instead of
inside a picoserve handler future, where the nested-router generics would
instantiate it once per layer (`http::scratch` measured that multiplier at 22×).
And the button driver's state is two 24 B folds, because the PIO holds the
timing and the CPU holds only an anchor.

**Flash: 1,096,104 B**, 69.7 % of the 1,536 KB active partition — the raw `.bin`
of the boot-integrated image, measured 2026-08-16. It read 993,760 B (63.1 %)
after task #12, on the standalone profile and before the OTA path linked; the
profile change alone is 73,744 B of it, per the three-image table below. The
bulk of task #12's own increase was `sequential-storage`'s map (its item
iteration, page-state machine, CRC and auto-repair paths all instantiate against
one concrete flash type) and `DeviceConfig::serialize`, which is the *write*
half of the serde pair whose read half was already the image's largest function.
Two single items still account for most of the total and neither costs any
RAM:
**232,803 B is the CYW43's own firmware** (`43439A0.bin` 231,077 + CLM 984 +
board NVRAM 742) and **54,528 B is the embedded settings SPA**, both
`include_bytes!` into `.rodata`. Together that is 287 KB that every OTA image
carries and every OTA transfer moves, which is worth knowing before Phase 4
sizes its download budget.

The largest single *function* in the image is 35,582 B: the visitor
`serde_derive` generates for `ConfigPatch::deserialize`. That is the price of
parsing `PUT /api/config` into bounded types with no allocator, and it scales
with the number of config keys — worth a glance whenever the configuration
grows a section.

### Correction to SPEC §4 — the handoff needs three buffers, not two

SPEC §4 specifies `static SNAPSHOTS: [ScoreboardSnapshot; 2]`: core 0 fills the
inactive buffer and publishes by storing its index. That is one buffer short of
correct. Core 1 latches an index at the top of a frame and reads from it for the
whole 50 ms frame, so the buffer it latched is still live *after* core 0 has
published a newer one — and core 0's next publish targets exactly that buffer.
Two publishes inside one frame is not a corner case: every live commit is
followed immediately by the play-flash commit, microseconds later.

Three slots is provably sufficient for one writer and one reader — one
published, one possibly latched, the writer takes the third. That is what the
MicroPython `TripleBufferedState` does (with a lock around the index
bookkeeping); `scoreboard-model`'s `SnapshotChannel` reaches the same guarantee
with one atomic swap per side and no lock, so core 0 never waits on core 1.

The cost is one extra 2,848 B slot over the spec's shape. The `Store` line is a
fourth copy: core 0 keeps the authoritative snapshot and publishes clones of it.
Folding those together — mutating the publisher's back buffer in place and
carrying forward from the just-published slot — would recover 2,848 B at the
price of making the whole state machine borrow the channel. If RAM ever gets
tight, that is the lever. **RAM has since got tight** — 36.6 % headroom, under
the ≥ 40 % target — so this is now a live option rather than a note, and it is
the cheapest 2,848 B in the file. It is still coupling the whole state machine
to the channel, so it should be taken deliberately and not as a reflex.

### Thirty-two crest slots, and what the last twenty-four bought

The pool was `display.py`'s `LogoPool(size=8)` — two crests per game, so eight
covered the game on screen, the one before it, and two more. That is enough to
rotate without thrashing and **not enough to ever stop fetching**. A full MLB
day is 15 games and 30 teams: an eight-slot pool evicts a crest it will want
again one lap later, every lap, for the whole evening.

| Slate | Teams | 8 slots | 32 slots |
|---|---:|---|---|
| Full MLB day | 30 | evicts every lap, forever | converges after one lap, 2 spare |
| NFL Sunday | 32 | evicts every lap, forever | converges after one lap, exact fit |
| College-football Saturday (~70 games) | ~140 | thrashes | fills once, then stops trying |

Thirty-two costs **27,648 B** of core-1 RAM (24 × 1,152) and 2,184 B in the
poller's arena for the directory that indexes it, and it is the single largest
deliberate spend in this file after the framebuffers. What it buys is a
property rather than a speedup: **after one lap of a normal slate the board
fetches no logos at all**, so a rotation, a skip, a league-skip and a mashed
button all paint immediately, and the `logo: evicting slot` line — which is the
only direct evidence a pool is too small — goes quiet for the evening. That line
staying quiet across a real MLB night is the acceptance test for this number.

The idle warmer (`poller::warm_crests`) is what reaches the property without
waiting for the rotation to walk the whole slate, and it is deliberately unable
to make this worse: it fills **free slots only** and never evicts, so on the
college-football row above it fills the pool once and then goes silent instead
of two fetchers taking turns evicting each other's work. Its own memory — 16
game records, sized to the most games a 32-slot pool can hold — is inside the
poller arena line.

**The steady state is zero traffic, not just zero logo traffic**, and that took
a second thing to remember. A crest is keyed by `{league key}/{abbreviation}`,
and the games list carries a state and an id only, so the warmer cannot know
which crest a game needs without fetching that game's detail. Left there it
would pay for one such probe per idle window forever. So the abbreviations are
remembered per game (`scoreboard_model::prefetch::WarmIndex`), and the poll
loop's own commits fill that index for free — a game the rotation has shown is
never probed at all. The convergence cost is therefore **one extra detail fetch
per game that the board has not yet displayed, once**, and after convergence an
idle scoreboard makes no crest requests and no probe requests:

| | Requests per idle window |
|---|---|
| Cold slate, games the board has not shown | ≤ 6 (a probe, then crests, per game) |
| Converged | 0 |
| Pool full (slate larger than 32 crests) | 0 |

Six is a *convergence* number, not a latency one — the warmer checks the command
channel between every fetch, so a press waits for one 1.1 KB logo whatever the
budget is. At six a 15-game MLB slate (15 probes, 30 crests) is warm four to six
minutes from boot, inside the first sitting; at two it was slower than the
rotation warms the pool by simply visiting games, which made the warmer nearly
pointless on a board being watched. The warmer also stops at the window's
deadline, so it cannot push the next poll late however short
`poll_interval_seconds` is set.

The 40 % floor is what bounds this. When the pool grew, thirty-two slots left
44.8 % headroom and sixty-four would have cost another 36,864 B and landed at
37.9 %, under the target — so this was the last doubling that fits, and it is
worth writing down that it was checked rather than assumed.

**Drill day then spent the margin somewhere else.** The four-connection HTTP
pool put the whole budget at 36.6 %, so the floor this section was checked
against no longer holds and the pool is now the largest single reversible item
in the file: dropping back to sixteen slots returns 18,432 B and puts the budget
back over 40 %. That is a real trade, and the table above is what prices it: a
full MLB day is 30 teams and an NFL Sunday is 32, so sixteen slots evict every
lap on both — which is exactly the property the last twenty-four slots were
bought to remove. It is written here so that whoever needs
RAM next finds the price already worked out instead of re-deriving it.

### The crest pool's split across the cores is not the cheap arrangement, because the cheap one is undefined behaviour

`display.py`'s `LogoPool` was one pool of buffers: Core 0 filled them, Core 1
drew from them, and evicting a slot the displayed state referenced tore a crest
for a frame. In Python that is a cosmetic race. In Rust a `&[LogoSlot]` handed
to a renderer *asserts* those bytes do not change while the borrow lives, so the
same arrangement is not a race but undefined behaviour, and no amount of care on
core 0 makes it sound while core 0 can write them.

Three ways out, priced at the pool's 32 slots:

| | Bytes | |
|---|---:|---|
| Crest bytes in the snapshot | 18,432 | Four more copies (three channel slots + the `Store`) at 2,304 B each, plus 2,304 B copied on **every publish**. Does not scale with the pool at all — it holds the *current* game's two crests — but it makes every publish 2,304 B more expensive and caches nothing, so a rotation re-fetches both crests every time. The `SnapshotChannel` docs reject it for exactly this reason. |
| One shared pool, `UnsafeCell` per slot | 36,864 | Cheapest, and it does not work: forming the `&[LogoSlot]` the renderer wants covers a cell core 0 may be writing, whatever the eviction rule says. |
| **Pixels on core 1, directory on core 0** | **39,201** | `CRESTS` 36,865 B, owned outright by the render loop, plus a 2,336 B channel. Core 0 keeps keys and LRU only. |

The third is what is built. It costs 2,337 B over the unsound option — a
*constant*, not a per-slot cost, which is why growing the pool to 32 did not
widen the gap — and one 1,152 B copy per crest *fetched*, on a path that runs
once per team per boot. In exchange the renderer's borrow is exclusive, which is
the only way it is sound at all.

It also closed the tear on purpose rather than by luck: eviction never chooses a
slot the published snapshot references, so the pixels behind a handle core 1 is
drawing cannot be replaced. **That rule did not have to change for 32 slots.**
It is stated over the two slots the published snapshot names, not over the pool,
so a bigger pool only widens the gap between the held slots and the eviction
candidates — and the idle warmer, the one new writer, does not evict at all.

### The receive buffer is 4,096 B, and `api_client.py` derived it from the wrong body

The constant is MicroPython's and it is right; the comment justifying it
(`api_client.py:22-27`) is not. It sizes 4 KB as "~3.5× the largest body (a
1,152 B logo)". The largest body is a **games list**, and it is the only one
that scales with anything:

| | Bytes |
|---|---:|
| List body at the wire format's ceiling: `2 + 255 × (2 + 9)` | 2,807 |
| Response header block, measured against the deployed backend | 386 |
| Worst case | 3,193 |
| Buffer | 4,096 |
| Spare | 903 |

255 is the format's own limit (the count is a `u8`); 9 is the game-id length
across every corpus fixture, asserted in `scoreboard-model::poll`'s tests so a
backend that changed it fails a host test rather than a device. The header block
counts because reqwless parses headers and body in the *same* caller-owned
buffer, so the peak is their sum.

One buffer, not two: a list refresh and a detail poll never overlap, so the list
gets all 4,096 B, and the detail phase — which is the only time a decoded game
and a crest are alive at once — splits it 2,048/2,048 with `split_at_mut`. The
2,048 B detail half holds a corpus maximum of 148 B, a computed worst case near
800 B, and the headers. Overflow is loud: reqwless answers `BufferTooSmall` and
the panel shows `Network error / response too large`.

### The render tables are flash, and that is checked rather than assumed

SPEC §11 budgeted "Glyph/font tables: 0 RAM — `&'static` in flash". True, but a
`static` only lands in `.rodata` while nothing can write to it, and a single
`static mut` or a `Cell` in a palette would move kilobytes into `.bss` without a
compiler complaint. `scoreboard-render` forbids `unsafe`, mutates no static at
all (the mutation-contract table in its crate docs says why), and the result is
visible in the symbol table:

| Table | Bytes | Section |
|---|---:|---|
| `unscii_16` heap + index | 3,888 | `.rodata` |
| `unscii_8` heap + index | 2,360 | `.rodata` |
| `spleen_5x8` heap + index | 1,410 | `.rodata` |
| 10 compiled sprites (pixels + palettes) | 1,880 | `.rodata` |
| **Total** | **9,538** | **flash, 0 B RAM** |

The 19 Aseprite slices cost nothing at all: they are `const` rectangles, folded
into the code that reads them.

Reproduce (needs `binutils-arm-none-eabi`):

```sh
cargo build -p scoreboard-render --release --target thumbv8m.main-none-eabihf
RLIB=target/thumbv8m.main-none-eabihf/release/libscoreboard_render.rlib
# Every generated table, by section. All 'R' (read-only) is the claim.
arm-none-eabi-nm -S -td "$RLIB" | grep scoreboard_render9generated
# Writable statics from this crate. Zero is the claim.
arm-none-eabi-nm -S -td "$RLIB" | awk '$3 ~ /^[bBdD]$/' | grep scoreboard_render
```

The QR encoder is the crate's only sizeable working memory and it is **stack**,
not static: two 211 B buffers live inside `QrBitmap::encode` for the duration of
one call. The 343 B bitmap it fills is a field of the app's `PreparedView`,
which lands wherever the render loop's locals do — a core-1 stack line, not a
static.

### Correction to SPEC §11

The spec's framebuffer line read "~48 KB · 64×32×8 planes ×2 buffers". Both
halves were wrong: the shipping panel is 128×64, not 64×32, and ~48 KB did not
follow from the 64×32 geometry it cited either (that pairing works out to
16 KiB). The measured figure for the real panel is 65,536 B of bitplane buffers
— four times what the seed's own stated geometry implies, and the largest single
item in the budget.

### Correction to SPEC §11 — the core-1 loop state is arena, not stack

SPEC §11 budgets a "Core-1 stack, 8 KB, render loop only" and says nothing about
where the loop's own state lives. On embassy it is not the stack: a task's
future is a `static` in the task's pool, so `LoopState` — the frame rail, the
`PreparedView` with its 343 B QR bitmap, the skip memo, the frame probe — plus
the `Hub75Display` and the channel reader all sit in a 1,392 B arena that
`arm-none-eabi-nm` reports as `render_loop::POOL`.

That is why 8 KB of stack turns out to be generous: with the cross-frame state
elsewhere, the stack only carries renderer call frames and the one deep
transient, `QrBitmap::encode`'s two 211 B Reed-Solomon buffers. Measured
high-water is **2,480 B**, stable to within 4 B across every scenario.

**The QR gap is now closed.** That figure was recorded with a caveat: the setup
screen is the only caller of `QrBitmap::encode` and it was not in the probe's
rotation. Phase 3's networking put it there — an un-provisioned device boots
straight into it — and the setup screen measures **2,476 B**, four bytes *under*
the deepest demo scenario. The QR encoder is not the deepest frame after all;
the live game screens are. The 8 KB stays as sized regardless.

---

## Core 1: measured frame times

**The Phase 3 acceptance measurement.** `scoreboard-render` dropped
MicroPython's strip pre-rendering on the thesis that a compiled glyph blit is
cheap enough to do inline every frame. On silicon (Pico 2 W, 150 MHz, release
profile, `firmware-rs/app`'s frame probe, 100 frames per scenario), it is:

| Scenario | frame mean | frame max | render mean | render max | rebuild |
|---|---:|---:|---:|---:|---:|
| Idle — static screen, every frame skipped | 0.07 ms | 5.92 ms | 0.48 ms | 0.48 ms | 15 µs |
| Startup — static screen, 6 commits in 5 s | 0.39 ms | 6.36 ms | 0.79 ms | 0.89 ms | 14 µs |
| **Final line score, three rows scrolling** | 6.46 ms | 6.84 ms | 1.17 ms | 1.31 ms | 15 µs |
| Final + sticky spinner toast (full-frame dim) | 6.81 ms | 7.30 ms | 1.51 ms | 1.81 ms | 15 µs |
| Menu — 5 rows, marquee every frame | 6.85 ms | 7.19 ms | 1.57 ms | 1.69 ms | 14 µs |
| **MLB live + 255-byte play flash scrolling** | 6.90 ms | 7.41 ms | 1.59 ms | 1.96 ms | 116 µs |

**Worst frame measured: 7.41 ms of the 50 ms budget — 15 %.** Zero overruns in
any scenario; core 1 held 20 FPS exactly (200 ticks per 10 s window) through a
5-minute soak, 58 consecutive scenario reports.

> **These numbers were taken at 20 FPS and are still the frame costs.** Task
> #17 moved the loop to 60 FPS, which changes the budget and therefore the duty,
> not the cost of drawing a frame — nothing in the render path or the packer
> depends on how often it is called. Against the 16.67 ms budget the same 7.41
> ms worst frame is **44 %**, with 9.26 ms of margin, and the busiest scenario's
> mean of 6.90 ms is a **41 % duty** on core 1 where it was 14 %. The one
> measured number the change invalidated was the gamma hook, and that was fixed
> rather than re-measured — see below. The frame probe now reports every 1,800
> ticks (still 30 s) and the liveness line reads 600 ticks per 10 s.
>
> **Then task #20 rebuilt the pack and the duty question went away**: `show`
> measures 2,241 µs where this table's frames were paying 5,250, so the busiest
> scenario's ~41 % duty is ~20 % (BACKLOG 63, closed on device 2026-08-09, zero
> overruns across 14,400+ drawn frames at 60 FPS). **A full scenario-by-scenario
> re-take at 60 FPS has still not been done** — drill day spent its bench time on
> the OTA path, and the numbers it did produce are in the "Drill day" section
> below. What exists is the pack measurement and the overrun count, which is
> what the acceptance question actually turned on.

The thesis holds by a wide margin. The two cases MicroPython could not draw
inline are the two in bold: three line-score rows measured ~41 ms there and
**1.17 ms** here; a long play line measured over 50 ms there and **1.96 ms**
here (worst case, at the wire format's 255-byte ceiling). The strip pool, its
capacity invariants and its per-glyph fallback bought about 35× of something
this port does not need.

The `frame max` column for the two static scenarios is not a slow frame — it is
the one frame in a hundred that actually drew, sitting next to ninety-nine that
skipped. Read the `render` columns for what drawing costs.

### Re-measured with the network stack up

The numbers above were taken with core 0 running three placeholder tasks. The
question Phase 3's networking has to answer is whether core 0's real load —
cyw43's SPI runner, smoltcp's poll loop, DHCP, a TCP connection to the backend
— starves core 1's executor. It does not:

| Scenario | frame mean | frame max | render max | Δ frame max |
|---|---:|---:|---:|---:|
| Idle | 0.08 ms | 5.56 ms | 0.53 ms | −0.36 ms |
| Startup | 0.39 ms | 6.03 ms | 0.93 ms | −0.33 ms |
| Final line score | 6.35 ms | 6.72 ms | 1.48 ms | −0.12 ms |
| Final + spinner toast | 6.68 ms | 7.20 ms | 2.18 ms | −0.10 ms |
| Menu | 6.70 ms | 7.18 ms | 1.73 ms | −0.01 ms |
| MLB live + 255-byte play flash | 6.72 ms | 7.15 ms | 1.98 ms | −0.26 ms |

Every window reported exactly **200 ticks per 10 s — 20.0 FPS — with zero
overruns**, across the station bench run, the AP-fallback run, and the
un-provisioned run. (At 60 FPS the same line reads 600 ticks per 10 s; the
question this section answers — whether core 0's real load starves core 1's
executor — is one the higher rate asks three times as often, so it is on the
drill-day list.) The deltas are all *negative* and all within the build noise
the section below warns about (flash placement moves the XIP cache under
hub75's packing loop); the honest reading is not "networking made rendering
faster" but "networking costs core 1 nothing that this instrument can see".

That is the expected result rather than a lucky one. The two cores share no
lock, no allocator and no data path except the snapshot channel's single atomic
swap and the brightness atomic; the network lives entirely on core 0's
executor, and core 1's frame is dominated (~76 %) by `show`, which is DMA setup
and a pure-CPU repack that touches neither.

### Where a frame actually goes

`show` — `load_rgb565` + `flip`, repacking 8,192 RGB565 pixels into eight BCM
bitplanes — costs **5.25 ms** and is flat across every scenario, because it does
not depend on what was drawn. That is **~76 % of a drawn frame**, and it is
`crates/hub75`, not the render path. Two consequences:

- The render budget question is settled and needs no further work. If frame time
  ever has to come down, the driver's packer is where the time is, not the
  glyphs (BACKLOG item 63). 7.41 ms against 50 ms was not a problem to solve;
  what made it one was 60 FPS, which left the pack unchanged at 5.25 ms per
  drawn frame but ran it three times as often — **31 % of core 1's wall time**
  on a screen that draws every frame, against 10 % before. **That is what task
  #20 then fixed**: the pack measures 2,241 µs today and the duty is ~20 %, so
  this bullet's advice was taken and the item is closed. The glyph path was
  never the place to look, and still is not.
- A *skipped* frame costs 0.07 ms, two orders of magnitude less than a drawn
  one. The static-screen skip is worth every line it costs.

> **Pack rebuilt by task #20 and measured on device — BACKLOG 63, closed
> 2026-08-09.** The repack now goes through fused gamma+bitspread tables
> (`hub75::packing::FusedTables` — 1 KiB in the driver, rebuilt by `set_gamma`
> itself) and the loop runs from RAM instead of XIP; BACKLOG 63 carries the
> design record and the declined alternatives. thumbv8m codegen went from ~165
> instructions per pixel pair to 78, and the projection of ~2.1–2.6 ms landed:
> **`show` mean 2,241 µs against 5,250, a 2.34× speedup**, stable across eight
> 30 s windows, worst lap 2,257 µs, **zero overruns in 14,400+ drawn frames at
> 60 FPS**. Core 1's duty on a screen that draws every frame went from ~44 % to
> **~20 %**. The predicted RAM deltas were right too, and are now in the table
> above: `render_loop::POOL` 1,384 → 3,448 B and 562 B of `.data` for the
> RAM-resident loop.
>
> **So every 5.25 ms-derived number below is superseded**, and the tables have
> deliberately not been rewritten around 2.241 ms: they are the measurement that
> justified the rebuild, and re-deriving them by arithmetic would turn measured
> rows into computed ones. What changed is the conclusion, not the history —
> `show` is no longer ~76 % of a drawn frame, and the headroom question the
> 60 FPS move opened is closed with ~13 ms of margin per frame.

`rebuild` (the prepared view, on commit frames only) is 14–16 µs for everything
except the 255-byte play line, where measuring 255 glyphs to size the flash
window takes **116 µs** — still a rounding error, and it happens once per
commit, not once per frame.

`brightness` — core 0 moving the atomic, core 1 re-deriving the driver's OE
timing stream — costs **112–138 µs mean, 190 µs worst**, and only on the frames
where the value actually changed (25 of every 100 ticks under the demo's
deliberately brisk sweep; a real light sensor moves far slower). It is not a
per-frame cost and it is not hiding in the render number.

One caution on precision: `show` moved from 5.07 to 5.25 ms across two builds
that differed only by a reordered statement and 120 B of code. Flash placement
moves the XIP cache under the hot packing loop, so treat the third significant
figure as build noise and compare orders of magnitude, not percentages.

Reproduce: flash `firmware-rs/app` and capture a full 30 s scenario cycle —

```sh
cd firmware-rs/app
cargo build --release --locked --target thumbv8m.main-none-eabihf
probe-rs download --chip RP235x target/thumbv8m.main-none-eabihf/release/scoreboard-app
timeout 80 probe-rs run --chip RP235x target/thumbv8m.main-none-eabihf/release/scoreboard-app > session.log 2>&1
```

The demo task that drove those scenarios was scaffolding for this measurement
and left with the real poller, as planned — `demo::feed::POOL` is gone from the
symbol table below. Re-running the scenario cycle now means driving the real
poller against fixtures, which is why the re-take at 60 FPS is still outstanding.

---

## Measured breakdown — `scoreboard-app`, release, `thumbv8m.main-none-eabihf`

**`link-boot-integrated`, with flip-link, measured 2026-08-17** after the
flat-dispatch router and the picoserve select-fix pin landed (previous
measures: 337,712 B at 108bc85 — the image the production unit runs — and
330,352 B with the flat dispatch alone). **1,091,776 B flash** as a raw
`.bin` (69.4 % of the 1,536 KB active partition), **11,296 B `.data`**,
**289,552 B `.bss`**, **4,096 B `.uninit`** → **304,944 B of RAM statics**
(**42.7 % headroom**), with core-0 stack below them (flip-link keeps
`_stack_start` at the statics' floor).

The previous version of this table was the task-#10 snapshot — standalone
profile, two HTTP connections, the demo feed still in it, and no poller, crest
pool, mDNS or OTA path. It is not annotated here because almost every row moved;
what follows replaces it.

| Symbol | Bytes | Owner |
|---|---:|---|
| `http::serve::POOL` | 55,552 | the four HTTP connection tasks. **13,888 B each, of which 8,552 B is buffers** — see the multiplier note below, and its 2026-08-17 addendum for the two cuts that took each task from 22,080 B |
| `hub75::driver::FRAMEBUFFERS` | 65,536 | hub75 — the two BCM bitplane buffers |
| `scoreboard_app::CRESTS` | 36,865 | the 32-slot crest pool, owned outright by core 1 |
| `ringlog::RING` | 28,812 | the deployed log: 200 × 144 B slots. `.bss`, deliberately — see below |
| `scoreboard_app::FRAME` | 16,385 | app — RGB565 drawing surface (16,384 + 1 B `ConstStaticCell` flag) |
| `poller::run::POOL` | 16,096 | the poll loop's arena: `Slate`, the 4,096 B receive buffer, the crest directory, the warm index, and a whole tick's futures |
| `net::CYW43_STATE` | 12,696 | cyw43's ioctl state and its 4-deep packet channel in each direction |
| `http::scratch::SLOTS` | 12,292 | four 3,072 B response buffers and their claim flags, pooled rather than held in handler futures |
| `scoreboard_app::CHANNEL` | 8,552 | the three-slot snapshot handoff. **In `.data`, not `.bss`** — see below |
| `scoreboard_app::CORE1_STACK` | 8,192 | core 1's stack; 3,348 B high-water |
| `net::STACK_RESOURCES` | 5,336 | `StackResources<10>` — smoltcp's `SocketSet` storage |
| `net::cyw43_runner::POOL` | 5,232 | the radio runner's arena — its SPI scratch dominates |
| `defmt_rtt::BUFFER` | 4,096 | defmt ring, `.uninit`. **Dev-only** |
| `display_core1::render_loop::POOL` | 3,448 | core 1's task arena — the loop state, the display, and the driver's 1 KiB fused pack tables |
| `scoreboard_app::STORE` | 2,888 | core 0's authoritative snapshot |
| `net::dhcp_server::serve::POOL` | 2,752 | the DHCP task's two 1 KiB packet scratch buffers and its lease table |
| `logos::UPDATES` | 2,336 | the two-deep channel core 0 ships new crest pixels over |
| `net::api_client::TCP_STATE` | 2,050 | the poller's one connection: 1,536 B receive + 512 B send |
| `net::mdns::serve::POOL` | 1,776 | the mDNS task's arena |
| `net::bringup::POOL` | 1,656 | provisioning's arena: the scan's BSSID table and the credentials |
| `net::mdns::{RX,TX}_BUFFER` | 2,050 | 1,537 + 513 — the mDNS socket's payload buffers |
| `net::captive_dns::serve::POOL` | 1,168 | the DNS task's 512 B query and 528 B response scratch |
| `net::{dhcp_server,captive_dns}::{RX,TX}_BUFFER` | 4,100 | 4 × 1,025 — the four UDP socket payload buffers |
| `config::CONFIG` | 960 | **`.data`** — the running configuration behind one lock |
| `net::…::{RX,TX}_META` | 600 | UDP packet metadata across all three responders |
| `hub75::packing::pack_rgb565` | 562 | **`.data`** — the pack loop itself, copied to RAM so it does not run from XIP |
| `inputs::run::POOL` + `brightness::auto_brightness::POOL` | 528 | 264 each |
| `.L_MergedGlobals` ×4 | 404 | hub75 driver statics, `net::hosts::HOSTS`, the stack-watermark atomics, embassy/defmt/probe singletons |
| `supervise::watchdog::POOL` | 360 | the feeder |
| `settings::DISPLAY` | 312 | **`.data`** — the live display settings core 1 applies at a frame boundary |
| `embassy_rp::gpio::BANK0_WAKERS` | 240 | embassy-rp |
| `supervise::PREVIOUS` | 228 | the decoded breadcrumb `/api/logs/previous` serves |
| `net::net_runner::POOL` | 136 | smoltcp's driver arena |
| `net::watch_link::POOL` + `supervise::reboot_on_request::POOL` | 136 | |
| `embassy_rp::dma::CHANNEL_WAKERS` | 128 | embassy-rp, for CH0 |
| `supervise::liveness::POOL` | 104 | |
| `net::status::STATUS` | 100 | what provisioning decided, for `/api/status` |
| `embassy_rp::pio::…::WAKERS` | 96 | embassy-rp, for PIO2 |
| `flash::ram_helpers::write_flash_inner` | 68 | **`.data`** — embassy-rp's XIP-disabled programmer, likewise RAM-resident |
| `hub75::driver::TIMING_BUFFER` | 64 | the OE/BCM timing stream |
| `_SEGGER_RTT` + `defmt_rtt::NAME` | 54 | `.data`, RTT control block. Dev-only |
| everything else (wakers, flags, padding) | ~440 | |

**The breadcrumb cell is not in this table and that is the point.** It is 256 B
at `scoreboard_layout::BREADCRUMB_BASE`, above the top of every profile's RAM
region, so no linker allocated it and no stack can reach it. See the caveat at
the end of this file.

### A buffer in a picoserve handler costs its size times the router's depth

The single most surprising number in this table is `http::serve::POOL`: 22,080 B
per connection, of which the actual buffers — 1,536 B TCP receive, 2,920 B TCP
send, 4,096 B request — are 8,552 B. The other 13,528 B is the future, and it is
paid four times over.

picoserve's router is a **type**, not a dispatch table. Each `.route()` produces
`Route<Path, Handler, Fallback>` wrapping the previous router as its fallback,
so nine routes are nine layers of nested generics, and the future for "handle
one request" contains, at every layer, that layer's handler future *and* the
whole fallback chain beneath it. A `heapless::Vec` local to a handler is
therefore instantiated once per layer it appears under.

Measured directly, by building the same code twice:

| Response buffer | Log chunk | `http::serve::POOL` |
|---:|---:|---:|
| 256 B | 256 B | 60,336 B |
| 3,072 B | 2,048 B | 263,088 B |

4,608 B of extra buffer, 202,752 B of extra arena — a **22× multiplier**. Those
two builds were made at two connections, and the multiplier is *per connection*:
at four, the same buffer change costs twice as much arena again. So response
buffers live in
`http::scratch`, a pool of one slot per connection, and a handler's future holds
an eight-byte `Lease`. The pool is the 12,292 B line above; it grows with the
connection count and not when a route is added.

**Before adding a route or a response type, check this number again.** The rule
of thumb: anything larger than a pointer that lives across an `await` inside a
handler should be in the pool, not in the handler.

*Addendum (2026-08-17, BACKLOG 94):* the `-Zprint-type-sizes` walk the item
prescribed named every byte. Each `.route()` wrapper cost a flat 120 B of
future, and the whole "handle one request" future was materialised **three
times per connection** — picoserve's internal `select`/`select_either` were
`async fn`s taking futures by value, and a generator never overlaps its
argument slots with the `pin!`-ed locals they move into. Both cuts are in:
the route chain is one flat `PathRouterService` (`routes.rs::Dispatch`,
−7,360 B), and the select duplication is fixed by named `Select` future
structs on fork `dgrantpete/picoserve` branch `perf/select-by-ref`
(−25,408 B more; the app pins it by rev — drop the patch when an upstream
release carries it, and an upstream PR is held pending the owner's outward
contact, the picojson pattern). HTTP behavior was held byte-identical under
a 16-probe matrix against the seated unit through both changes. The 22×
table above still holds for anything a handler keeps across an `await` —
these cuts changed who multiplies, not that multiplication happens.

### The ring log is in `.bss`, and that was not free

`Ring::new()` is 28,812 B of mostly zeros. Two non-zero fields would have put
the whole thing in `.data` — where it costs its size *again* in flash, for an
initializer image that is almost entirely zeros. Both were removed: the
sequence counter tracks the last-used sequence (0 at boot) rather than the next
one (1), and the level filter is stored as `2 - level` so that the default,
`Debug`, is the zero value. `.data` fell from 38,992 B to 10,184 B when they
were, which is 28,808 B of flash and of every OTA transfer.

### A gamma change used to cost 27.5 ms on core 1, and no longer runs there

`PUT /api/config` sends display settings to core 1, which applies them at the
top of a frame. Measured on hardware at 20 FPS, per hook:

| Hook | Cost |
|---|---:|
| gamma (rebuilds the 256-entry LUT, 256 × `libm::pow`) | 27,562 µs |
| data clock + refresh rate + blanking, together | included above |
| render settings only (variants, dividers, scroll speed) | 10 µs |
| boot, every hook at once | 2,017 µs |

27.5 ms was over half a 50 ms frame. It fit — the frame itself is ~7 ms, no
overrun was recorded, and the loop held 20.0 FPS across the whole config
exercise — but it was the largest single thing that happened inside a frame,
and it is why the settings message carries the *set of hooks to run* rather
than just the values: a `PUT` that changes an SSID must not pay it. The boot
figure is lower because the LUT it builds is sRGB, which is a `const` table copy
rather than 256 calls to `pow`.

**The 60 FPS move turned that from a large cost into an impossible one** — 27.5
against a 16.7 ms budget is a guaranteed overrun — so it was fixed rather than
re-measured (BACKLOG 68, task #17). `hub75::gamma::GammaTable` carries the
finished 256 bytes, `DisplayUpdate` is built on core 0 where the request lands,
and core 1's share of a gamma change is a `copy_from_slice`. The row above is
kept because it is the measurement that motivated the change, and because it is
the only number in this file that a frame-rate change invalidated outright.
**Re-measure the gamma hook at drill day**: it should now be indistinguishable
from the render-settings row.

**The flash number is mostly not code.** `.rodata` is 269,660 B, and 232,803 B
of that is the CYW43's firmware, CLM and board NVRAM — `include_bytes!`
(`firmware-rs/app/cyw43-firmware/`). The radio has no flash of its own, so the
host uploads the image over SPI at every boot; there is no way to not carry it.
Phase 4 should note that every OTA image and every OTA transfer includes those
232 KB.

**`CHANNEL` is initialized data, so it costs its 8,552 B twice** — once in RAM
and once in flash, plus a boot-time copy. `ScoreboardSnapshot::new()` sets
`UiColors` to white, so the initializer is not all-zero and the linker cannot
put it in `.bss`. Harmless at 0.5 % of the active partition, and noted here so
the flash number is not a surprise later.

Reproduce (needs `binutils-arm-none-eabi`; CI runs exactly this):

```sh
cd firmware-rs/app
cargo build --release --locked --target thumbv8m.main-none-eabihf
ELF=target/thumbv8m.main-none-eabihf/release/scoreboard-app
arm-none-eabi-size -B -d "$ELF"
arm-none-eabi-nm -C --size-sort -r -S -td "$ELF" | awk '$3 ~ /^[bBdD]$/'
```

---

## Measured breakdown — `hub75-diag`, release, `thumbv8m.main-none-eabihf`

The panel-driver reference point, kept so the hub75 numbers stay comparable
across commits. Links plain (no flip-link), which is why its symbol addresses
are not comparable with the app's.
Re-measured 2026-08-09, after task #20's repack. **32,348 B flash** (`.text` +
`.rodata` + `.data` + vector/boot blocks), **616 B `.data`**, **85,096 B
`.bss`**, **1,024 B `.uninit`** → **86,736 B of RAM statics**, leaving 427.3 KiB
of its 512 KiB map unspent.

| Symbol | Bytes | Owner |
|---|---:|---|
| `hub75::driver::FRAMEBUFFERS` | 65,536 | hub75 — the two BCM bitplane buffers |
| `hub75_diag::FRAME` | 16,385 | app — RGB565 drawing surface (16,384 + 1 B `ConstStaticCell` flag) |
| `hub75_diag::__embassy_main::POOL` | 2,816 | embassy task arena — holds the `Hub75Display`, hence the gamma LUT and the fused pack tables. It was 768 B before task #20 |
| `defmt_rtt::BUFFER` | 1,024 | defmt-rtt ring, `.uninit`. **Dev-only**; not in the deployed budget |
| `embassy_rp::gpio::BANK0_WAKERS` | 240 | embassy-rp |
| `.L_MergedGlobals` | 104 | embassy/defmt singletons and driver flags |
| `hub75::driver::TIMING_BUFFER` | 64 | the OE/BCM timing stream |
| `hub75::packing::pack_rgb565` | 562 | **`.data`** — the RAM-resident pack loop, which is why `.data` went from 56 B to 616 B |
| `_SEGGER_RTT` + `defmt_rtt::NAME` | 54 | `.data`, RTT control block. Dev-only |
| `embassy_rp::time_driver::DRIVER` + clocks cache + PAC flag | 21 | |

Reproduce (needs `binutils-arm-none-eabi`; CI runs exactly this and echoes it
into the job summary):

```sh
cd firmware-rs/hub75-diag
cargo build --release --locked --target thumbv8m.main-none-eabihf
ELF=target/thumbv8m.main-none-eabihf/release/hub75-diag
arm-none-eabi-size -B -d "$ELF"   # headline text/data/bss
arm-none-eabi-size -A -d "$ELF"   # per-section; ignore .debug_* and its Total
arm-none-eabi-nm -C --size-sort -r -S -td "$ELF" | awk '$3 ~ /^[bBdD]$/'
```

### What a flash write costs the panel — measured

A flash program on the RP2350 runs from RAM with XIP disabled, and embassy-rp
arranges that by parking core 1 through the multicore FIFO. Core 1 executes the
render loop from XIP, so **every config save stops the display**. The question is
only how long, and the frame probe answers it directly.

Measured on the bench unit, `PUT /api/config` with a 942 B document, against
`Mode(final)` reports either side of it:

| | frame mean | frame max | render max | show max | overruns |
|---|---:|---:|---:|---:|---:|
| baseline (600 ticks) | 6,642 µs | 7,259 µs | 1,331 µs | 6,341 µs | 0 |
| baseline (600 ticks) | 6,654 µs | 9,657 µs | 4,130 µs | 7,152 µs | 0 |
| **containing the save** | 6,669 µs | **14,544 µs** | 7,639 µs | 7,022 µs | **0** |

So one save costs the frame it lands in about **5–7 ms**, taking it to 14.5 ms
against the 50 ms budget, and **drops nothing**. The mean does not move, because
one frame in six hundred is not a rate. The hitch shows up in `render` or in
`show` depending on where core 1 happened to be when it was parked, which is why
both maxima rise.

**This is the measurement 60 FPS pressed hardest on, and it is still the one to
re-take first.** 14.5 ms was 29 % of a 50 ms budget and is 87 % of a 16.67 ms
one. It still fits, but the margin was 2.1 ms where every other number in this
file has at least 9 — and the baseline frames either side of the save ranged 7.3
to 9.7 ms, so a save landing on the unluckier of those would cross the deadline.

Task #20's repack has since taken roughly 3 ms out of every drawn frame, which
moves both the baseline and the save frame down by about that much and is the
one thing that would materially change this row. **It has not been re-measured**,
so the 14.5 ms stands as the number of record and the margin should be read as
"wider than 2.1 ms, by an amount nobody has put a probe on".
What that costs is bounded and already handled: the loop counts an overrun,
re-anchors, and carries on one frame late; it does not drop or fast-forward
anything. A config save producing an occasional counted overrun is the expected
behaviour at 60 FPS, not a fault, and the probe's `overrun` column is where it
will show up.

That is an *append*, not an erase. `sequential-storage` only erases when a page
fills and the region wraps: at ~942 B per save into 4 KB pages across a 980 KB
region, the first erase is about a thousand saves away. An erase is roughly
30 ms per sector — under a 50 ms frame and comfortably over a 16.67 ms one, so
at 60 FPS it is about two frames of visible hitch rather than a long frame. It
is not on the normal path, and the one place it *is* deliberate — a storage
region that does not read as a map at all, erased once and only ever once — runs
at boot before core 1 starts, where parking core 1 is a no-op and the cost is
zero frames.

The same reasoning is why every boot-time flash read happens before
`spawn_core1`.

---

## Phase 4: the three images, and the RAM the OTA path did not cost

Measured 2026-08-08, `--release`, `thumbv8m.main-none-eabihf`, raw `.bin` via
`rust-objcopy -O binary` (what actually occupies flash, not the ELF).

| Image | Bytes | Of its partition | Notes |
|---|---:|---:|---|
| `scoreboard-boot` | 14,208 | 43.4% of 32 KB | `opt-level = "s"`, defmt + `embassy_boot=trace`. The spike's was 12.2 KB; the difference is logging kept on purpose — those trace lines are the only direct evidence of what a given boot decided, and drill day reads them. |
| `scoreboard-app`, `link-boot-integrated` | 1,088,880 | 69.1% of 1536 KB | What `publish-fw` ships. **1,096,104 B / 69.7 % as of 108bc85** — the four-connection change added 7,224 B of code. |
| `scoreboard-app`, `link-standalone` | 1,015,136 | — | The bench image. Not re-measured since. |

**The boot-integrated image is 73,744 B larger**, and all of it is the OTA
path: `ed25519-dalek` + `curve25519-dalek`, `sha2`, and `embassy-boot`. That is
the price of the device being able to refuse an image, and it is paid only by
the profile that can install one.

**476,760 B of headroom** before an image stops fitting the active partition.
Worth watching rather than worrying about: the DFU partition is one erase page
larger than active by construction, so the ceiling is a single number and the
const asserts in `firmware-rs/layout` fail the build rather than the swap.

### The download costs zero RAM, which was not a given

SPEC §11's table budgeted an OTA chunk buffer and hedged it as "unioned where
phases can't overlap (OTA vs. poll)". The union turned out to be exact. Because
an update is a *phase of the poll loop* rather than a task of its own
(`app/src/ota`'s module docs argue why), the poller's 4,096 B receive buffer is
idle for the whole download — so it splits into a 2,048 B header half and a
2,048 B chunk half and nothing new is allocated. The updater's own state buffer
is **one byte** (embassy-rp's `NorFlash::WRITE_SIZE` is 1).

The same argument returned a socket. §7's budget reserved one for OTA; the
poller's connection is free while an update runs, so the total is unchanged
even though Phase 4 also added an mDNS responder — which took the slot.

| Phase 4 addition | RAM | Where |
|---|---:|---|
| OTA chunk + header buffers | **0** | Split from the poller's existing 4,096 B |
| `BlockingFirmwareUpdater` aligned buffer | 1 B | Stack, per call |
| mDNS socket buffers | 2,050 B | 1,537 rx + 513 tx, `net::mdns` statics |
| mDNS packet metadata | 200 B | 12 slots: 8 rx + 4 tx |
| mDNS task arena | 1,776 B | `net::mdns::serve::POOL`, including the `Responder` |
| One socket slot | 0 | It took the one §7.1 had reserved for OTA |

### Drill day 2026-08-16 — the numbers, measured

The section this replaces existed to be deleted; here is what the hardware
said. All figures from the seated production unit over real Wi-Fi, captured
by SWD snapshots of the defmt ring plus `/api/status` polling.

- **The DFU hash: 10,635 ms at 1,094,232 B (~103 KB/s).** PAST the 8 s
  watchdog ceiling — the estimate's "spans both sides of the limit" resolved
  on the wrong side, and the single blocking call reset the device at the end
  of every install, three times, until the attempt record blocked the
  version. Survivable only because the verify now feeds and yields between
  4 KB chunks (`ota/install.rs`); the walk's duration is no longer
  correctness-relevant. For scale: embassy-boot's own two-byte path at ~30×
  would be five-plus minutes.
- **The swap: ~29 s** (41 s of dark panel reset-to-HTTP, of which ~12 s is
  boot + Wi-Fi rejoin). Inside the 35–70 s budget. The revert cycle — swap
  in, 8 s wedge starvation, revert swap, boot — measured **~86 s of dark**.
- **The download: 1.09 MB in ~35 s** with smooth 10 %-steps, core 1 dipping
  to ~53 FPS during chunk writes (one 9 ms overrun frame observed; benign).
- **Download-to-confirmed, end to end: ~132 s** (35 download + 11 verify +
  5 countdown + 41 dark + ~30 trial-to-confirm).

## Caveats to close before the numbers can be trusted end to end

- **Stacks are still not in `size`'s output**, but both are now measured
  another way. `arm-none-eabi-size` reports statics only. Core 1's stack is an
  8,192 B static painted with 0xAA before the core starts, and
  `supervise::liveness` reports the deepest byte touched every 10 s — 3,348 B
  against real backend data. Core 0's stack is painted and probed the same way,
  and its **high-water was 25,816 B when the stack was 266,536 B**. The stack is
  now 186,304 B, because it is the remainder below the statics and the statics
  grew; the *depth* should not have moved, but that has not been confirmed since
  drill day and the ratio it is reported as certainly has. The watermark is worth
  watching, because the two 3 KB buffers that live on it (the storage scratch
  and a config save) are the deepest transients in the firmware.
  `hub75-diag` still has neither.
- **`flip-link` is wired up for `firmware-rs/app`**, and the numbers above are
  measured with it. Core 0's stack sits below `.bss`/`.data`
  (`_stack_start = 0x2002_d7c0`, `_stack_end = 0x2000_0000`) and
  `install_core0_stack_guard()` arms MSPLIM at the bottom, so overflow faults
  rather than eating `FRAMEBUFFERS`. Core 1 gets the same protection by a
  different route: `spawn_core1` arms MSPLIM at the bottom of the static stack
  it is handed. `hub75-diag` still links plain — a bench binary with one shallow
  task and 427.3 KiB of slack, where the guard buys little (BACKLOG 64).
- **512 KiB vs. 520 KiB, minus one cell.** `memory.x` declares
  `RAM : LENGTH = 0x7ff00` — **524,032 B**, the contiguous striped banks less the
  top 256 B the layout crate withholds for the crash breadcrumb (drill day: the
  bootloader's stack shredded a cell that lived inside the app's map; see
  `scoreboard_layout::BREADCRUMB_BASE`). The RP2350's other 8 KiB (two
  non-striped 4 KiB banks) is likewise not in the linker's map, so neither
  can be spent without a deliberate placement. The headroom target is stated
  against 520 KiB per the spec; the linked figure is what is actually
  reachable today, and **neither clears 40 % any more** — 36.6 % and 35.6 %
  respectively. That sentence used to end "and both clear 40 %".
- **defmt/RTT is dev-only.** 4,150 B in the app (a 4 KB ring, raised from the
  default because two cores log into it, plus the control block) and 1,078 B in
  `hub75-diag`. Both leave the release image; the deployed build spends its
  logging RAM on the ring buffer in the "Ring log + misc statics" line instead.
  Worth restating now that headroom is tight: the deployed figure is ~4 KB below
  the measured one, and that is the whole of the slack this caveat buys.
- **The measurements above come from the artifact in the tree, not from CI.**
  Both ELFs were built by hand on the days they are dated. The symbol tables
  reconcile to their section totals, which is the check that they are not stray
  builds, but the next size-relevant PR should re-run the reproduce blocks under
  `--locked` and replace these rather than adding to them.

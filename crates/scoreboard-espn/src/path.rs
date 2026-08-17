//! The streaming path-matcher engine.
//!
//! Sport-agnostic: a const table of path patterns is matched against the
//! structural path of a JSON document as picojson's `PushParser` walks it,
//! and a [`Sink`] receives exactly the matched values and container
//! boundaries. Unmatched subtrees cost a bitset check and nothing else —
//! skip-unknown is the default by construction (DESIGN.md).
//!
//! Matching state is one `u64` bitset per open container plus per-frame
//! counters: pattern `p` is alive at depth `d` iff its first `d` segments
//! match the current path. A pattern's cursor position always equals the
//! depth, because every segment consumes exactly one path level — that is
//! what lets the whole matcher be bitsets instead of a trie. Incoming keys
//! are compared against alive patterns' next segments at the `Key` event;
//! no key text is ever buffered.

use picojson::{
    DefaultConfig, Event, ParseError, PushParseError, PushParser, PushParserHandler,
};

/// Hard cap on patterns per table: the per-depth alive set is a `u64`.
pub const MAX_PATTERNS: usize = 64;

/// Maximum nesting depth, matching picojson `DefaultConfig`'s 32-level
/// bitstack. ESPN scoreboard bodies nest ~15 deep (asserted with headroom
/// in the tests).
pub const MAX_DEPTH: usize = 32;

/// One segment of a path pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seg {
    /// Match an object member by (unescaped) key.
    Key(&'static str),
    /// Match exactly this array index.
    Index(u16),
    /// Match every array index; the concrete index is reported to the sink.
    AnyIndex,
}

/// A pattern is a path from the document root, one segment per level.
/// An empty pattern designates the root value itself.
pub type Pattern = &'static [Seg];

/// A matched scalar, valid only for the duration of the sink call.
/// Numbers carry their raw JSON text so no precision or format is lost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    Str(&'a str),
    Num(&'a str),
    Bool(bool),
    Null,
}

/// The sink's answer to any callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Directive {
    Continue,
    /// Fast-forward to the close of the innermost open **array element**
    /// (the whole document remainder if no array is open). No further
    /// values or `enter`s are delivered from inside the skipped region, but
    /// `leave` still fires for containers already `enter`ed — every `enter`
    /// is always paired with a `leave`.
    SkipElement,
}

/// What kind of container an [`Sink::enter`] callback opened. Exists because
/// `{"events":{…}}` and `{"events":[]}` are otherwise indistinguishable to a
/// sink (an object's members arrive as engine-internal key events), and the
/// sport tables must 502-shape the former while the latter is a legal
/// no-games day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Object,
    Array,
}

/// Receives matches. `pattern` is the index into the table; `indices` holds
/// the concrete array index bound to each [`Seg::AnyIndex`] of that pattern,
/// outermost first (`u16`: a college-Saturday slate can exceed a `u8`).
///
/// When several patterns match one node, callbacks fire in ascending
/// pattern-index order.
pub trait Sink {
    /// A scalar matched a pattern designating it.
    fn value(&mut self, pattern: usize, indices: &[u16], value: Value<'_>) -> Directive;

    /// A container (object or array) matched a pattern designating it.
    fn enter(&mut self, _pattern: usize, _indices: &[u16], _kind: ContainerKind) -> Directive {
        Directive::Continue
    }

    /// The matched container closed.
    fn leave(&mut self, _pattern: usize, _indices: &[u16]) -> Directive {
        Directive::Continue
    }
}

#[derive(Debug, PartialEq)]
pub enum Error {
    /// The tokenizer or push parser rejected the input.
    Parse(ParseError),
    /// The pattern table exceeds [`MAX_PATTERNS`].
    TableTooLarge,
    /// A pattern exceeds [`MAX_DEPTH`] segments.
    PatternTooDeep,
    /// The document nests deeper than [`MAX_DEPTH`].
    DepthOverflow,
}

impl From<ParseError> for Error {
    fn from(e: ParseError) -> Self {
        Error::Parse(e)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Frame {
    /// Patterns whose first `depth` segments matched the path to this
    /// container (`depth` = this frame's stack position).
    active: u64,
    /// Objects: the alive set for the upcoming member slot, computed at the
    /// `Key` event and consumed by the member's value.
    pending: u64,
    is_array: bool,
    /// Arrays: index of the next element slot.
    next: u16,
    /// Arrays: index of the element currently in progress — what descendants
    /// read when binding an `AnyIndex` at this level.
    cur: u16,
}

/// The matcher: engine state over one pattern table.
struct Engine<'t> {
    table: &'t [Pattern],
    frames: [Frame; MAX_DEPTH],
    depth: usize,
}

impl<'t> Engine<'t> {
    fn new(table: &'t [Pattern]) -> Result<Self, Error> {
        if table.len() > MAX_PATTERNS {
            return Err(Error::TableTooLarge);
        }
        if table.iter().any(|p| p.len() > MAX_DEPTH) {
            return Err(Error::PatternTooDeep);
        }
        Ok(Self {
            table,
            frames: [Frame::default(); MAX_DEPTH],
            depth: 0,
        })
    }

    fn full_mask(&self) -> u64 {
        if self.table.len() == 64 {
            u64::MAX
        } else {
            (1u64 << self.table.len()) - 1
        }
    }

    /// Alive set among `parent_active` for a child reached by key `k`.
    fn advance_key(&self, parent_active: u64, parent_depth: usize, k: &str) -> u64 {
        let mut out = 0u64;
        let mut bits = parent_active;
        while bits != 0 {
            let p = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let segs = self.table[p];
            if segs.len() > parent_depth {
                if let Seg::Key(want) = segs[parent_depth] {
                    if want == k {
                        out |= 1 << p;
                    }
                }
            }
        }
        out
    }

    /// Alive set among `parent_active` for a child at array index `i`.
    fn advance_index(&self, parent_active: u64, parent_depth: usize, i: u16) -> u64 {
        let mut out = 0u64;
        let mut bits = parent_active;
        while bits != 0 {
            let p = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let segs = self.table[p];
            if segs.len() > parent_depth {
                match segs[parent_depth] {
                    Seg::AnyIndex => out |= 1 << p,
                    Seg::Index(want) if want == i => out |= 1 << p,
                    _ => {}
                }
            }
        }
        out
    }

    /// Patterns in `alive` that designate the node at `slot_depth` itself.
    fn full_matches(&self, alive: u64, slot_depth: usize) -> u64 {
        let mut out = 0u64;
        let mut bits = alive;
        while bits != 0 {
            let p = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            if self.table[p].len() == slot_depth {
                out |= 1 << p;
            }
        }
        out
    }

    /// Bind pattern `p`'s `AnyIndex` levels from the open array frames.
    /// Every `AnyIndex` level of a matched pattern is an array frame, or the
    /// pattern could not have matched.
    fn bindings<'b>(&self, p: usize, slot_depth: usize, buf: &'b mut [u16; MAX_DEPTH]) -> &'b [u16] {
        let mut n = 0;
        for (k, seg) in self.table[p][..slot_depth].iter().enumerate() {
            if *seg == Seg::AnyIndex {
                buf[n] = self.frames[k].cur;
                n += 1;
            }
        }
        &buf[..n]
    }

    /// Compute the alive set for the slot a value/container is about to
    /// occupy, updating the parent array counters as a side effect.
    fn slot_alive(&mut self) -> u64 {
        if self.depth == 0 {
            return self.full_mask();
        }
        let d = self.depth - 1;
        if self.frames[d].is_array {
            let i = self.frames[d].next;
            self.frames[d].cur = i;
            self.frames[d].next = i.saturating_add(1);
            let active = self.frames[d].active;
            self.advance_index(active, d, i)
        } else {
            self.frames[d].pending
        }
    }

    /// `SkipElement`: mask every frame above the innermost array frame (or
    /// every frame, when no array is open) down to the patterns that
    /// designate that frame's own container — descendants stop matching,
    /// while already-entered containers keep their pending `leave`.
    fn apply_skip(&mut self) {
        let floor = (0..self.depth).rev().find(|&k| self.frames[k].is_array);
        let start = match floor {
            Some(a) => a + 1,
            None => 0,
        };
        for k in start..self.depth {
            let own = self.full_matches(self.frames[k].active, k);
            self.frames[k].active = own;
            self.frames[k].pending = 0;
        }
    }

    fn fire_values<S: Sink>(&mut self, alive: u64, slot_depth: usize, v: Value<'_>, sink: &mut S) {
        let mut fires = self.full_matches(alive, slot_depth);
        let mut skip = false;
        let mut buf = [0u16; MAX_DEPTH];
        while fires != 0 {
            let p = fires.trailing_zeros() as usize;
            fires &= fires - 1;
            let idx = self.bindings(p, slot_depth, &mut buf);
            if sink.value(p, idx, v) == Directive::SkipElement {
                skip = true;
            }
        }
        if skip {
            self.apply_skip();
        }
    }

    fn on_event<S: Sink>(&mut self, event: Event<'_, '_>, sink: &mut S) -> Result<(), Error> {
        match event {
            Event::Key(k) => {
                let d = self.depth - 1;
                let active = self.frames[d].active;
                self.frames[d].pending = if active == 0 {
                    0
                } else {
                    self.advance_key(active, d, k.as_str())
                };
            }
            Event::StartObject | Event::StartArray => {
                let is_array = matches!(event, Event::StartArray);
                let kind = if is_array {
                    ContainerKind::Array
                } else {
                    ContainerKind::Object
                };
                let alive = self.slot_alive();
                let slot_depth = self.depth;
                if self.depth == MAX_DEPTH {
                    return Err(Error::DepthOverflow);
                }
                self.frames[self.depth] = Frame {
                    active: alive,
                    pending: 0,
                    is_array,
                    next: 0,
                    cur: 0,
                };
                self.depth += 1;
                let mut fires = self.full_matches(alive, slot_depth);
                let mut skip = false;
                let mut buf = [0u16; MAX_DEPTH];
                while fires != 0 {
                    let p = fires.trailing_zeros() as usize;
                    fires &= fires - 1;
                    let idx = self.bindings(p, slot_depth, &mut buf);
                    if sink.enter(p, idx, kind) == Directive::SkipElement {
                        skip = true;
                    }
                }
                if skip {
                    self.apply_skip();
                }
            }
            Event::EndObject | Event::EndArray => {
                let d = self.depth - 1;
                let mut fires = self.full_matches(self.frames[d].active, d);
                let mut skip = false;
                let mut buf = [0u16; MAX_DEPTH];
                while fires != 0 {
                    let p = fires.trailing_zeros() as usize;
                    fires &= fires - 1;
                    let idx = self.bindings(p, d, &mut buf);
                    if sink.leave(p, idx) == Directive::SkipElement {
                        skip = true;
                    }
                }
                self.depth = d;
                if skip {
                    self.apply_skip();
                }
            }
            Event::String(s) => {
                let alive = self.slot_alive();
                let slot_depth = self.depth;
                self.fire_values(alive, slot_depth, Value::Str(s.as_str()), sink);
            }
            Event::Number(n) => {
                let alive = self.slot_alive();
                let slot_depth = self.depth;
                self.fire_values(alive, slot_depth, Value::Num(n.as_str()), sink);
            }
            Event::Bool(b) => {
                let alive = self.slot_alive();
                let slot_depth = self.depth;
                self.fire_values(alive, slot_depth, Value::Bool(b), sink);
            }
            Event::Null => {
                let alive = self.slot_alive();
                let slot_depth = self.depth;
                self.fire_values(alive, slot_depth, Value::Null, sink);
            }
            Event::EndDocument => {}
        }
        Ok(())
    }
}

/// The `PushParserHandler` gluing the engine to picojson. Retrieve the sink
/// back with [`StreamMatcher::finish`].
pub struct PathEvents<'t, S: Sink> {
    engine: Engine<'t>,
    sink: S,
}

impl<'a, 'b, 't, S: Sink> PushParserHandler<'a, 'b, Error> for PathEvents<'t, S> {
    fn handle_event(&mut self, event: Event<'a, 'b>) -> Result<(), Error> {
        self.engine.on_event(event, &mut self.sink)
    }
}

/// The crate's entry point: a pattern table + a sink + a caller-owned
/// scratch buffer, fed network chunks (from a reused receive buffer if the
/// caller likes — the chunk is only borrowed per `write` call).
pub struct StreamMatcher<'t, 'scratch, S: Sink> {
    parser: PushParser<'scratch, PathEvents<'t, S>, DefaultConfig>,
}

impl<'t, 'scratch, S: Sink> StreamMatcher<'t, 'scratch, S> {
    /// `scratch` must hold the longest contiguous string or number token in
    /// the input (sport lanes size it from the corpus bounds).
    pub fn new(table: &'t [Pattern], sink: S, scratch: &'scratch mut [u8]) -> Result<Self, Error> {
        let engine = Engine::new(table)?;
        Ok(Self {
            parser: PushParser::new(PathEvents { engine, sink }, scratch),
        })
    }

    pub fn write(&mut self, chunk: &[u8]) -> Result<(), Error> {
        self.parser.write::<Error>(chunk).map_err(flatten)
    }

    /// Finish the document and hand the sink back.
    pub fn finish(self) -> Result<S, Error> {
        self.parser.finish::<Error>().map(|h| h.sink).map_err(flatten)
    }
}

fn flatten(e: PushParseError<Error>) -> Error {
    match e {
        PushParseError::Parse(pe) => Error::Parse(pe),
        PushParseError::Handler(err) => err,
    }
}

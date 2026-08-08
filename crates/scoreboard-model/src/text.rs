//! Bounded owned text, and the fold that gets wire strings into the display
//! fonts' repertoire.
//!
//! Every string that reaches [`crate::ScoreboardSnapshot`] passes through
//! [`push_folded`], which is the Rust equivalent of `textfold.fold_text`
//! applied at `wire.read_str` — the single ingest point in the MicroPython
//! firmware. Folding at copy-out rather than at decode keeps
//! `scoreboard-wire` a pure zero-copy decoder and puts the cost exactly where
//! the bytes are already being moved.

use core::fmt::Write as _;

/// The length prefix is a `u16`, not the platform `usize`, so a snapshot has
/// the same layout on the host and on `thumbv8m` — the RAM budget can be
/// measured by a host test.
pub type Text<const N: usize> = heapless::String<N, u16>;

/// One line of `spleen_5x8` across the full 128 px panel.
const LINE_MAX_CHARS: usize = 25;

/// Latin Extended-A (complete, in codepoint order) followed by the Latin
/// Extended-B / General Punctuation strays worth having: Romanian comma-below
/// (what ESPN sends for Romanian names), the hyphen/dash forms, curly quotes,
/// prime marks, and the bullet. Multi-char expansions live in [`FOLD_MULTI`].
const FOLD_SRC: &str = concat!(
    "ĀāĂăĄąĆćĈĉĊċČčĎďĐđ",
    "ĒēĔĕĖėĘęĚěĜĝĞğĠġĢģ",
    "ĤĥĦħĨĩĪīĬĭĮįİı",
    "ĴĵĶķĸĹĺĻļĽľĿŀŁł",
    "ŃńŅņŇňŊŋŌōŎŏŐő",
    "ŔŕŖŗŘřŚśŜŝŞşŠš",
    "ŢţŤťŦŧŨũŪūŬŭŮůŰűŲų",
    "ŴŵŶŷŸŹźŻżŽžſ",
    "ȘșȚț",
    "‐‑‒–—―",
    "‘’‚“”„",
    "•′″⁄",
);

const FOLD_DST: &str = concat!(
    "AaAaAaCcCcCcCcDdDd",
    "EeEeEeEeEeGgGgGgGg",
    "HhHhIiIiIiIiIi",
    "JjKkkLlLlLlLlLl",
    "NnNnNnNnOoOoOo",
    "RrRrRrSsSsSsSs",
    "TtTtTtUuUuUuUuUuUu",
    "WwYyYZzZzZzs",
    "SsTt",
    "------",
    "'',\"\"\"",
    "\u{b7}'\"/",
);

/// The folds that widen: ligatures and the ellipsis.
const FOLD_MULTI: &[(char, &str)] = &[
    ('Ĳ', "IJ"),
    ('ĳ', "ij"),
    ('ŉ', "'n"),
    ('Œ', "OE"),
    ('œ', "oe"),
    ('…', "..."),
];

const fn char_count(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut chars = 0;
    while index < bytes.len() {
        // Count lead bytes; continuation bytes are 0b10xxxxxx.
        if bytes[index] & 0xC0 != 0x80 {
            chars += 1;
        }
        index += 1;
    }
    chars
}

// The two halves are data, not code: a length drift would silently misfold
// everything past the drift point, so it fails the build instead.
const _: () = assert!(char_count(FOLD_SRC) == char_count(FOLD_DST));

/// The in-repertoire ceiling: the fonts cover 32..=255, so Latin-1 names
/// (Suárez, Peña) render natively and only what is above this folds.
const FOLD_FLOOR: char = '\u{100}';

/// Fold one codepoint to its closest in-repertoire equivalent.
///
/// Returns [`Fold::Keep`] for anything already renderable and for unmapped
/// high codepoints, which pass through and draw as the fonts' `'?'` glyph.
fn fold(c: char) -> Fold {
    if c < FOLD_FLOOR {
        return Fold::Keep;
    }
    if let Some(replacement) = FOLD_SRC
        .chars()
        .zip(FOLD_DST.chars())
        .find_map(|(src, dst)| (src == c).then_some(dst))
    {
        return Fold::One(replacement);
    }
    match FOLD_MULTI.iter().find(|(src, _)| *src == c) {
        Some((_, replacement)) => Fold::Many(replacement),
        None => Fold::Keep,
    }
}

enum Fold {
    Keep,
    One(char),
    Many(&'static str),
}

/// Replace `dst` with `src`, folded, truncated at a char boundary if it does
/// not fit.
///
/// Truncation is never an error: the wire caps strings at 255 bytes and every
/// bound here is sized from the corpus with margin, so overflow means the
/// upstream data is anomalous — dropping the tail beats refusing to render.
pub fn set_folded<const N: usize>(dst: &mut Text<N>, src: &str) {
    dst.clear();
    push_folded(dst, src);
}

/// Append `src`, folded, stopping cleanly at capacity.
pub fn push_folded<const N: usize>(dst: &mut Text<N>, src: &str) {
    // A pure-ASCII string — the overwhelming majority — needs no inspection
    // past this check.
    if src.is_ascii() {
        let room = N - dst.len();
        let _ = dst.push_str(&src[..src.len().min(room)]);
        return;
    }
    for c in src.chars() {
        let pushed = match fold(c) {
            Fold::Keep => dst.push(c),
            Fold::One(replacement) => dst.push(replacement),
            Fold::Many(replacement) => dst.push_str(replacement),
        };
        if pushed.is_err() {
            return;
        }
    }
}

/// Replace `dst` with `src` verbatim — for text this crate built itself, which
/// is in-repertoire by construction.
pub fn set_plain<const N: usize>(dst: &mut Text<N>, src: &str) {
    dst.clear();
    set_plain_append(dst, src);
}

/// Append `src` verbatim, stopping at capacity.
pub fn set_plain_append<const N: usize>(dst: &mut Text<N>, src: &str) {
    let room = N - dst.len();
    let end = src
        .char_indices()
        .map(|(index, _)| index)
        .chain(core::iter::once(src.len()))
        .take_while(|index| *index <= room)
        .last()
        .unwrap_or(0);
    let _ = dst.push_str(&src[..end]);
}

/// Replace `dst` with `src` capped at one panel line, marking a truncation
/// with a trailing dot (`state._truncate_line`). The cap counts *glyphs*, so
/// the byte bound is the second, wider limit.
pub fn set_line<const N: usize>(dst: &mut Text<N>, src: &str) {
    if src.chars().count() <= LINE_MAX_CHARS {
        set_folded(dst, src);
        return;
    }
    set_capped(dst, src, LINE_MAX_CHARS - 1);
    let _ = dst.push('.');
}

/// Replace `dst` with at most `max_chars` glyphs of `src`, folded.
pub fn set_capped<const N: usize>(dst: &mut Text<N>, src: &str, max_chars: usize) {
    dst.clear();
    for c in src.chars().take(max_chars) {
        if push_one_folded(dst, c).is_err() {
            return;
        }
    }
}

fn push_one_folded<const N: usize>(dst: &mut Text<N>, c: char) -> Result<(), ()> {
    let pushed = match fold(c) {
        Fold::Keep => dst.push(c),
        Fold::One(replacement) => dst.push(replacement),
        Fold::Many(replacement) => dst.push_str(replacement),
    };
    pushed.map_err(|_| ())
}

/// Append `src` folded and upper-cased.
///
/// The fold runs after the case change, not before: upper-casing Latin-1 can
/// produce codepoints above the font repertoire (ÿ → Ÿ, U+0178), which the
/// MicroPython path — folding at ingest, upper-casing at commit — lets through.
pub fn push_folded_upper<const N: usize>(dst: &mut Text<N>, src: &str) {
    for upper in src.chars().flat_map(char::to_uppercase) {
        if push_one_folded(dst, upper).is_err() {
            return;
        }
    }
}

/// `write!` into a bounded string, discarding the overflow error.
///
/// Formatting only ever builds text this crate controls the shape of (times,
/// ordinals, score columns), so a bound is either right or a bug — never a
/// runtime condition worth propagating.
macro_rules! write_text {
    ($dst:expr, $($arg:tt)*) => {{
        let dst = &mut *$dst;
        dst.clear();
        $crate::text::write_args(dst, ::core::format_args!($($arg)*));
    }};
}
pub(crate) use write_text;

/// Append formatted text, discarding the overflow error. See [`write_text`].
pub(crate) fn write_args<const N: usize>(dst: &mut Text<N>, args: core::fmt::Arguments<'_>) {
    let _ = dst.write_fmt(args);
}

//! Engine semantics + the S0 chunk-split methodology applied one level up:
//! whatever the chunking (including feeding through a reused buffer, the
//! firmware's shape), a pattern table must deliver the identical sink
//! sequence.

use scoreboard_espn::path::MAX_DEPTH;
use scoreboard_espn::{Directive, Error, Pattern, Seg, Sink, StreamMatcher, Value};
use std::fs;
use std::path::PathBuf;

use Seg::{AnyIndex, Index, Key};

#[derive(Debug, Clone, PartialEq)]
enum Rec {
    Enter(usize, Vec<u16>),
    Leave(usize, Vec<u16>),
    Val(usize, Vec<u16>, String),
}

fn render(v: &Value<'_>) -> String {
    match v {
        Value::Str(s) => format!("s:{s}"),
        Value::Num(n) => format!("n:{n}"),
        Value::Bool(b) => format!("b:{b}"),
        Value::Null => "null".to_string(),
    }
}

type SkipPred = Box<dyn FnMut(usize, &str) -> bool>;

/// Records everything; optionally answers SkipElement when a predicate
/// fires on a value.
struct RecSink {
    recs: Vec<Rec>,
    skip_when: Option<SkipPred>,
}

impl RecSink {
    fn new() -> Self {
        Self { recs: Vec::new(), skip_when: None }
    }
    fn skipping(pred: impl FnMut(usize, &str) -> bool + 'static) -> Self {
        Self { recs: Vec::new(), skip_when: Some(Box::new(pred)) }
    }
}

impl Sink for RecSink {
    fn value(&mut self, pattern: usize, indices: &[u16], value: Value<'_>) -> Directive {
        let rendered = render(&value);
        self.recs.push(Rec::Val(pattern, indices.to_vec(), rendered.clone()));
        if let Some(pred) = self.skip_when.as_mut() {
            if pred(pattern, &rendered) {
                return Directive::SkipElement;
            }
        }
        Directive::Continue
    }
    fn enter(&mut self, pattern: usize, indices: &[u16]) -> Directive {
        self.recs.push(Rec::Enter(pattern, indices.to_vec()));
        Directive::Continue
    }
    fn leave(&mut self, pattern: usize, indices: &[u16]) -> Directive {
        self.recs.push(Rec::Leave(pattern, indices.to_vec()));
        Directive::Continue
    }
}

fn run_chunked(table: &'static [Pattern], input: &[u8], sink: RecSink, sizes: &[usize]) -> Result<Vec<Rec>, Error> {
    let mut scratch = vec![0u8; 16 * 1024];
    let mut m = StreamMatcher::new(table, sink, &mut scratch)?;
    let mut pos = 0;
    let mut i = 0;
    while pos < input.len() {
        let n = sizes[i % sizes.len()].max(1);
        let end = (pos + n).min(input.len());
        m.write(&input[pos..end])?;
        pos = end;
        i += 1;
    }
    m.finish().map(|s| s.recs)
}

fn run(table: &'static [Pattern], json: &str) -> Vec<Rec> {
    run_chunked(table, json.as_bytes(), RecSink::new(), &[json.len().max(1)]).unwrap()
}

/// Feed through one fixed reused buffer — the firmware's socket-loop shape.
fn run_reused(table: &'static [Pattern], input: &[u8], chunk: usize) -> Vec<Rec> {
    let mut scratch = vec![0u8; 16 * 1024];
    let mut m = StreamMatcher::new(table, RecSink::new(), &mut scratch).unwrap();
    let mut recv = [0u8; 4096];
    for piece in input.chunks(chunk.clamp(1, 4096)) {
        recv[..piece.len()].copy_from_slice(piece);
        for b in recv[piece.len()..].iter_mut() {
            *b = b'!';
        }
        m.write(&recv[..piece.len()]).unwrap();
    }
    m.finish().unwrap().recs
}

// ---------------------------------------------------------------- semantics

#[test]
fn index_vs_anyindex() {
    static T: &[Pattern] = &[
        &[Key("a"), Index(0), Key("x")],
        &[Key("a"), AnyIndex, Key("x")],
    ];
    let recs = run(T, r#"{"a":[{"x":1},{"x":2}]}"#);
    assert_eq!(
        recs,
        vec![
            Rec::Val(0, vec![], "n:1".into()),
            Rec::Val(1, vec![0], "n:1".into()),
            Rec::Val(1, vec![1], "n:2".into()),
        ]
    );
}

#[test]
fn sibling_patterns_and_container_designation() {
    static T: &[Pattern] = &[
        &[Key("a")],
        &[Key("a"), Key("b")],
        &[Key("a"), Key("c")],
    ];
    let recs = run(T, r#"{"a":{"b":1,"c":"z"},"d":9}"#);
    assert_eq!(
        recs,
        vec![
            Rec::Enter(0, vec![]),
            Rec::Val(1, vec![], "n:1".into()),
            Rec::Val(2, vec![], "s:z".into()),
            Rec::Leave(0, vec![]),
        ]
    );
}

#[test]
fn same_pattern_scalar_vs_container() {
    static T: &[Pattern] = &[&[Key("a")]];
    assert_eq!(run(T, r#"{"a":7}"#), vec![Rec::Val(0, vec![], "n:7".into())]);
    assert_eq!(
        run(T, r#"{"a":{"k":1}}"#),
        vec![Rec::Enter(0, vec![]), Rec::Leave(0, vec![])]
    );
    assert_eq!(
        run(T, r#"{"a":[1]}"#),
        vec![Rec::Enter(0, vec![]), Rec::Leave(0, vec![])]
    );
}

#[test]
fn root_pattern_and_nested_arrays() {
    static T: &[Pattern] = &[
        &[],
        &[Key("m"), AnyIndex, AnyIndex],
    ];
    let recs = run(T, r#"{"m":[[10,20],[30]]}"#);
    assert_eq!(
        recs,
        vec![
            Rec::Enter(0, vec![]),
            Rec::Val(1, vec![0, 0], "n:10".into()),
            Rec::Val(1, vec![0, 1], "n:20".into()),
            Rec::Val(1, vec![1, 0], "n:30".into()),
            Rec::Leave(0, vec![]),
        ]
    );
}

#[test]
fn escaped_keys_match_unescaped() {
    static T: &[Pattern] = &[&[Key("a\nb"), Key("café")]];
    let recs = run(T, r#"{"a\nb":{"café":true}}"#);
    assert_eq!(recs, vec![Rec::Val(0, vec![], "b:true".into())]);
}

#[test]
fn raw_number_text_is_preserved() {
    static T: &[Pattern] = &[&[Key("n")]];
    let recs = run(T, r#"{"n":-12.5e2}"#);
    assert_eq!(recs, vec![Rec::Val(0, vec![], "n:-12.5e2".into())]);
}

#[test]
fn null_and_bool_values() {
    static T: &[Pattern] = &[&[Key("x")], &[Key("y")]];
    let recs = run(T, r#"{"x":null,"y":false}"#);
    assert_eq!(
        recs,
        vec![Rec::Val(0, vec![], "null".into()), Rec::Val(1, vec![], "b:false".into())]
    );
}

#[test]
fn deep_nesting_headroom_and_overflow() {
    // 20 levels: ESPN's ~15 plus margin, must work.
    static T20: &[Pattern] = &[&[Key("k"); 20]];
    let mut json = String::new();
    for _ in 0..20 {
        json.push_str(r#"{"k":"#);
    }
    json.push('1');
    json.push_str(&"}".repeat(20));
    let recs = run(T20, &json);
    assert_eq!(recs, vec![Rec::Val(0, vec![], "n:1".into())]);

    // Past MAX_DEPTH must be a clean error (ours or the tokenizer's).
    static TE: &[Pattern] = &[&[Key("z")]];
    let deep = MAX_DEPTH + 2;
    let mut too_deep = String::new();
    for _ in 0..deep {
        too_deep.push_str(r#"{"k":"#);
    }
    too_deep.push('1');
    too_deep.push_str(&"}".repeat(deep));
    let r = run_chunked(TE, too_deep.as_bytes(), RecSink::new(), &[too_deep.len()]);
    assert!(r.is_err(), "expected depth error, got {r:?}");
}

// ------------------------------------------------------------------- errors

#[test]
fn table_validation() {
    static ONE: Pattern = &[Key("a")];
    let big: Vec<Pattern> = vec![ONE; 65];
    let mut scratch = [0u8; 64];
    let err = StreamMatcher::new(big.leak(), RecSink::new(), &mut scratch).err();
    assert!(matches!(err, Some(Error::TableTooLarge)));

    static DEEP: &[Pattern] = &[&[Key("a"); MAX_DEPTH + 1]];
    let mut scratch = [0u8; 64];
    let err = StreamMatcher::new(DEEP, RecSink::new(), &mut scratch).err();
    assert!(matches!(err, Some(Error::PatternTooDeep)));
}

// --------------------------------------------------------------------- skip

const EVENTS_JSON: &str = r#"{"events":[
    {"id":"1","status":{"name":"pre"},"z":"a1"},
    {"id":"2","status":{"name":"live"},"z":"a2"},
    {"id":"3","status":{"name":"post"},"z":"a3"}
]}"#;

#[test]
fn skip_element_from_direct_child() {
    static T: &[Pattern] = &[
        &[Key("events"), AnyIndex, Key("id")],
        &[Key("events"), AnyIndex, Key("z")],
    ];
    // Keep only event with id "2": skip every other element at its id.
    let sink = RecSink::skipping(|p, v| p == 0 && v != "s:2");
    let recs = run_chunked(T, EVENTS_JSON.as_bytes(), sink, &[EVENTS_JSON.len()]).unwrap();
    assert_eq!(
        recs,
        vec![
            Rec::Val(0, vec![0], "s:1".into()),
            Rec::Val(0, vec![1], "s:2".into()),
            Rec::Val(1, vec![1], "s:a2".into()),
            Rec::Val(0, vec![2], "s:3".into()),
        ]
    );
}

#[test]
fn skip_element_from_nested_object_suppresses_rest_of_element() {
    static T: &[Pattern] = &[
        &[Key("events"), AnyIndex, Key("status"), Key("name")],
        &[Key("events"), AnyIndex, Key("z")],
    ];
    // Skip the whole event when its (nested) status.name is "live".
    let sink = RecSink::skipping(|p, v| p == 0 && v == "s:live");
    let recs = run_chunked(T, EVENTS_JSON.as_bytes(), sink, &[EVENTS_JSON.len()]).unwrap();
    assert_eq!(
        recs,
        vec![
            Rec::Val(0, vec![0], "s:pre".into()),
            Rec::Val(1, vec![0], "s:a1".into()),
            Rec::Val(0, vec![1], "s:live".into()),
            // z of event 1 suppressed; event 2 resumes normally
            Rec::Val(0, vec![2], "s:post".into()),
            Rec::Val(1, vec![2], "s:a3".into()),
        ]
    );
}

#[test]
fn skip_preserves_enter_leave_pairing() {
    static T: &[Pattern] = &[
        &[Key("events"), AnyIndex],
        &[Key("events"), AnyIndex, Key("id")],
        &[Key("events"), AnyIndex, Key("z")],
    ];
    let sink = RecSink::skipping(|p, v| p == 1 && v == "s:1");
    let recs = run_chunked(T, EVENTS_JSON.as_bytes(), sink, &[EVENTS_JSON.len()]).unwrap();
    assert_eq!(
        recs,
        vec![
            Rec::Enter(0, vec![0]),
            Rec::Val(1, vec![0], "s:1".into()),
            Rec::Leave(0, vec![0]), // paired despite the skip
            Rec::Enter(0, vec![1]),
            Rec::Val(1, vec![1], "s:2".into()),
            Rec::Val(2, vec![1], "s:a2".into()),
            Rec::Leave(0, vec![1]),
            Rec::Enter(0, vec![2]),
            Rec::Val(1, vec![2], "s:3".into()),
            Rec::Val(2, vec![2], "s:a3".into()),
            Rec::Leave(0, vec![2]),
        ]
    );
}

#[test]
fn skip_with_no_open_array_suppresses_document_remainder() {
    static T: &[Pattern] = &[&[Key("id")], &[Key("later")]];
    let sink = RecSink::skipping(|p, _| p == 0);
    let json = r#"{"id":"x","later":1}"#;
    let recs = run_chunked(T, json.as_bytes(), sink, &[json.len()]).unwrap();
    assert_eq!(recs, vec![Rec::Val(0, vec![], "s:x".into())]);
}

// -------------------------------------------------- chunk-split invariance

fn corpus_sample() -> Vec<PathBuf> {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../backend/testdata"));
    let mut files = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        if dir.file_name().is_some_and(|n| n == "wire") {
            continue;
        }
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "json") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(files.len() >= 10, "corpus missing? found {}", files.len());
    // Every ~3rd file keeps the runtime sane while covering all sports.
    files.into_iter().step_by(3).collect()
}

/// Paths present in real event fixtures across sports; boundaries included.
static CORPUS_TABLE: &[Pattern] = &[
    &[Key("id")],
    &[Key("status"), Key("type"), Key("name")],
    &[Key("competitions"), Index(0), Key("competitors"), AnyIndex],
    &[Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("score")],
    &[Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("homeAway")],
    &[Key("competitions"), Index(0), Key("competitors"), AnyIndex, Key("team"), Key("abbreviation")],
    &[Key("competitions"), Index(0), Key("status"), Key("type"), Key("state")],
];

#[test]
fn corpus_chunk_split_invariance() {
    for path in corpus_sample() {
        let bytes = fs::read(&path).unwrap();
        let reference =
            run_chunked(CORPUS_TABLE, &bytes, RecSink::new(), &[bytes.len()]).unwrap();
        assert!(
            !reference.is_empty(),
            "table matched nothing in {path:?} — fixture shape changed?"
        );

        let one_byte = run_chunked(CORPUS_TABLE, &bytes, RecSink::new(), &[1]).unwrap();
        assert_eq!(one_byte, reference, "1-byte feed diverged on {path:?}");

        for seed in [3usize, 17, 1379] {
            let sizes: Vec<usize> = (0..64)
                .map(|i| 1 + (seed.wrapping_mul(31).wrapping_add(i * 7)) % 97)
                .collect();
            let chunked = run_chunked(CORPUS_TABLE, &bytes, RecSink::new(), &sizes).unwrap();
            assert_eq!(chunked, reference, "seed {seed} diverged on {path:?}");
        }

        let reused = run_reused(CORPUS_TABLE, &bytes, 1379);
        assert_eq!(reused, reference, "reused-buffer feed diverged on {path:?}");
    }
}

//! Shared, deterministic helpers for the behavioural property tests.
//!
//! Everything here is test-side code: it builds controlled Unicode
//! corpora, measures rendered output the way a real terminal would
//! (per-character cell columns, ANSI sequences taking no cells), and
//! records/shrinks counterexamples with a fixed seed.

#![allow(dead_code)]

use std::{fmt::Write as _, fs, path::PathBuf};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const FIXED_SEED: u64 = 0x5EED_1234_ABCD_0001;

/// A tiny, dependency free, deterministic PRNG (splitmix64) so that
/// counterexamples are reproducible regardless of the host/toolchain.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        }
    }
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
    pub fn chance(&mut self, numerator: u32, denominator: u32) -> bool {
        (self.next_u64() % u64::from(denominator)) < u64::from(numerator)
    }
    pub fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        &slice[self.below(slice.len())]
    }
}

// ---------------------------------------------------------------------------
// Terminal-side measurement
// ---------------------------------------------------------------------------

/// Remove CSI/OSC escape sequences. These encode style only and occupy
/// no cells on screen.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for c2 in chars.by_ref() {
                        if (0x40..=0x7e).contains(&(c2 as u32)) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut prev = '\u{1b}';
                    for c2 in chars.by_ref() {
                        if c2 == '\u{7}' || (prev == '\u{1b}' && c2 == '\\') {
                            break;
                        }
                        prev = c2;
                    }
                }
                Some(_) => {}
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn has_ansi(s: &str) -> bool {
    s.contains('\u{1b}')
}

/// Number of terminal cells taken by a character, following the
/// per-character East-Asian width model that termimad's pipeline uses
/// (`UnicodeWidthChar::width`). This is deliberately *not*
/// `str::width()`: the layout accounts columns char by char, which is
/// exactly where a ZWJ/skin-tone grapheme would otherwise hide an
/// off-by-one-column error.
pub fn char_cells(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Per-character cell width of a (possibly styled) rendered line.
pub fn line_cells(rendered: &str) -> usize {
    strip_ansi(rendered).chars().map(char_cells).sum()
}

/// Visible cells of a plain source fragment.
pub fn visible_cells(s: &str) -> usize {
    s.chars().map(char_cells).sum()
}

pub fn starts_with_nonspacing(rendered: &str) -> bool {
    strip_ansi(rendered)
        .chars()
        .next()
        .map(|c| char_cells(c) == 0)
        .unwrap_or(false)
}

/// String level cell width. This is the model used by the composite /
/// wrapping / table layers, which cache `compound.src.width()`: ZWJ
/// emoji ligatures and skin-tone sequences are measured with the
/// Unicode tables as a whole (2 cells), exactly like a conforming
/// terminal renders them.
pub fn screen_width(plain: &str) -> usize {
    strip_ansi(plain).width()
}

/// Positions (cell columns) where visible characters start.
pub fn cell_columns(rendered: &str) -> Vec<usize> {
    let mut cols = Vec::new();
    let mut col = 0usize;
    for c in strip_ansi(rendered).chars() {
        cols.push(col);
        col += char_cells(c);
    }
    cols
}

// ---------------------------------------------------------------------------
// Graphemes
// ---------------------------------------------------------------------------

pub fn graphemes(s: &str) -> Vec<&str> {
    UnicodeSegmentation::graphemes(s, true).collect()
}

/// A zero width character left without a wide base would be rendered
/// as an isolated combining mark/joiner after cropping.
pub fn is_zero_width_char(c: char) -> bool {
    char_cells(c) == 0
}

pub fn grapheme_cells(g: &str) -> usize {
    g.chars().map(char_cells).sum()
}

/// Whether a grapheme contains a ZWJ / emoji-modifier sequence whose
/// per-character cell sum differs from the string level width lookup
/// tables. Layout decisions are taken per character, so such graphemes
/// are kept for the primitive cropping suite but excluded from the
/// composite viewport suite.
pub fn grapheme_is_zwj_like(g: &str) -> bool {
    g.contains('\u{200d}') || g.chars().any(|c| (0x1F3FB..=0x1F3FF).contains(&(c as u32)))
}

/// Concatenated visible content of a collection of source fragments,
/// in grapheme order.
pub fn join_graphemes<I, S>(parts: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut all = String::new();
    for p in parts {
        all.push_str(p.as_ref());
    }
    graphemes(&all).into_iter().map(str::to_string).collect()
}

// ---------------------------------------------------------------------------
// Counterexample reporting / shrinking
// ---------------------------------------------------------------------------

/// Build a human readable failure report.
///
/// `source` is the plain source text, `expected` describes the required
/// columns and `actual` the observed cell sequence.
pub fn failure_report(
    title: &str,
    source: &str,
    width: Option<usize>,
    expected: &str,
    actual: &str,
) -> String {
    let mut report = String::new();
    let _ = writeln!(report, "property violated: {title}");
    if let Some(w) = width {
        let _ = writeln!(report, "viewport width: {w}");
    }
    let _ = writeln!(report, "source graphemes ({}):", graphemes(source).len());
    for (i, g) in graphemes(source).iter().enumerate() {
        let _ = writeln!(
            report,
            "  [{i:>3}] {g:?} cells={} chars={}",
            grapheme_cells(g),
            g.chars().count(),
        );
    }
    let _ = writeln!(report, "expected: {expected}");
    let _ = writeln!(report, "actual:   {actual}");
    report
}

/// Directory in which the minimal failing sample is persisted.
/// It lives under `target/` so it is never mistaken for source data.
pub fn artifact_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(&dir).expect("create artifact dir");
    dir
}

/// Persist the minimal failing sample (deterministic filename) and
/// return its path.
pub fn save_sample(name: &str, content: &str) -> PathBuf {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = artifact_dir().join(format!("prop-failure-{safe}.txt"));
    fs::write(&path, content).expect("write minimal failing sample");
    path
}

/// Bounded, one-shot greedy shrinker.
///
/// `T` is the candidate input. Each iteration tries a few atom
/// deletions; whenever a candidate still fails it is kept. A hard cap on
/// the number of iterations guarantees shrinking terminates, and a
/// minimal candidate (empty atom list / single unsplittable atom) ends
/// immediately, so a singleton can never be shrunk forever.
pub fn shrink<T, F>(initial: T, mut candidates: impl FnMut(&T) -> Vec<T>, mut fails: F) -> T
where
    T: Clone,
    F: FnMut(&T) -> bool,
{
    const MAX_ITERATIONS: usize = 256;
    let mut current = initial;
    for _ in 0..MAX_ITERATIONS {
        let options = candidates(&current);
        if options.is_empty() {
            break; // singleton/irreducible: stop immediately
        }
        let before = options.len();
        let mut shrunk = false;
        for candidate in options {
            if fails(&candidate) {
                current = candidate;
                shrunk = true;
                break;
            }
        }
        if !shrunk {
            break;
        }
        debug_assert!(before >= 1);
    }
    current
}

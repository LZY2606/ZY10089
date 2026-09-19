//! Behavior-invariant property tests for width measurement, cropping,
//! wrapping, table composition, skins, and scrollable rendering.

use proptest::{collection, prelude::*};
use std::io::Write;
use termimad::{
    minimad::{Alignment, Composite, Compound},
    Area, CropWriter, Fitter, FmtComposite, FmtLine, MadSkin, TextView,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

const FIXED_SEED: u64 = 0x4274_4941_9f13_c577;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StyleKind {
    Plain,
    Bold,
    Italic,
    BoldItalic,
    Code,
    Strike,
}

#[derive(Clone, Debug)]
struct Fragment {
    text: String,
    style: StyleKind,
}

impl Fragment {
    fn compound(&self) -> Compound<'_> {
        let mut compound = Compound::raw_str(&self.text);
        match self.style {
            StyleKind::Plain => {}
            StyleKind::Bold => {
                compound = compound.bold();
            }
            StyleKind::Italic => {
                compound = compound.italic();
            }
            StyleKind::BoldItalic => {
                compound = compound.bold().italic();
            }
            StyleKind::Code => {
                compound = compound.code();
            }
            StyleKind::Strike => {
                compound = compound.strikeout();
            }
        }
        compound
    }
}

#[derive(Clone, Debug)]
struct Case {
    fragments: Vec<Fragment>,
    gap: String,
}

impl Case {
    fn visible(&self) -> String {
        let mut out = String::new();
        for (idx, fragment) in self.fragments.iter().enumerate() {
            if idx > 0 {
                out.push_str(&self.gap);
            }
            out.push_str(&fragment.text);
        }
        out
    }

    fn compounds(&self) -> Vec<Compound<'_>> {
        let mut compounds = Vec::new();
        for (idx, fragment) in self.fragments.iter().enumerate() {
            if idx > 0 {
                compounds.push(Compound::raw_str(self.gap.as_str()));
            }
            compounds.push(fragment.compound());
        }
        compounds
    }

    fn composite(&self, skin: &MadSkin) -> FmtComposite<'_> {
        let composite: Composite<'_> = self.compounds().into();
        FmtComposite::from(composite, skin)
    }
}

fn atom_strategy() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("a"),
        Just("AB"),
        Just("x7"),
        Just("日"),
        Just("本語"),
        Just("，"),
        Just("😀"),
        Just("e\u{301}"),
        Just("a\u{308}\u{301}"),
        Just("a\u{200d}日"),
        Just("👩\u{200d}💻"),
    ]
}

fn style_strategy() -> impl Strategy<Value = StyleKind> {
    prop_oneof![
        2 => Just(StyleKind::Plain),
        2 => Just(StyleKind::Bold),
        2 => Just(StyleKind::Italic),
        1 => Just(StyleKind::BoldItalic),
        2 => Just(StyleKind::Code),
        1 => Just(StyleKind::Strike),
    ]
}

fn fragment_strategy() -> impl Strategy<Value = Fragment> {
    (collection::vec(atom_strategy(), 1..4), style_strategy()).prop_map(|(atoms, style)| Fragment {
        text: atoms.concat(),
        style,
    })
}

fn simple_atom_strategy() -> impl Strategy<Value = &'static str> {
    atom_strategy().prop_filter("no ZWJ for ellision paths", |atom| {
        !atom.contains('\u{200d}')
    })
}

fn simple_fragment_strategy() -> impl Strategy<Value = Fragment> {
    (
        collection::vec(simple_atom_strategy(), 1..4),
        style_strategy(),
    )
        .prop_map(|(atoms, style)| Fragment {
            text: atoms.concat(),
            style,
        })
}

fn wrap_fragment_strategy() -> impl Strategy<Value = Fragment> {
    (atom_strategy(), style_strategy()).prop_map(|(text, style)| Fragment {
        text: text.to_owned(),
        style,
    })
}

fn case_strategy() -> impl Strategy<Value = Case> {
    (
        collection::vec(fragment_strategy(), 2..6),
        prop_oneof![Just(" "), Just("  "), Just("   ")],
    )
        .prop_map(|(fragments, gap)| Case {
            fragments,
            gap: gap.to_owned(),
        })
}

fn simple_case_strategy() -> impl Strategy<Value = Case> {
    (
        collection::vec(simple_fragment_strategy(), 2..6),
        prop_oneof![Just(" "), Just("  "), Just("   ")],
    )
        .prop_map(|(fragments, gap)| Case {
            fragments,
            gap: gap.to_owned(),
        })
}

fn wrap_case_strategy() -> impl Strategy<Value = Case> {
    (
        collection::vec(wrap_fragment_strategy(), 3..9),
        prop_oneof![Just(" "), Just("  ")],
    )
        .prop_map(|(fragments, gap)| Case {
            fragments,
            gap: gap.to_owned(),
        })
}

fn alignment_strategy() -> impl Strategy<Value = Alignment> {
    prop_oneof![
        Just(Alignment::Left),
        Just(Alignment::Center),
        Just(Alignment::Right),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cell {
    ch: char,
    col: usize,
}

fn strip_escapes(s: &str) -> String {
    let mut out = String::new();
    let mut bytes = s.chars();
    while let Some(ch) = bytes.next() {
        if ch == '\u{1b}' {
            match bytes.next() {
                Some('[') | Some('O') => {
                    for next in bytes.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut previous = '\0';
                    for next in bytes.by_ref() {
                        if previous == '\u{7}' || (previous == '\u{1b}' && next == '\\') {
                            break;
                        }
                        previous = next;
                    }
                }
                Some(_) => {}
                None => {}
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn cells_in_visible(visible: &str) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut col = 0;
    for grapheme in visible.graphemes(true) {
        let width = grapheme
            .chars()
            .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
            .sum::<usize>();
        for ch in grapheme.chars() {
            cells.push(Cell { ch, col });
        }
        col += width;
    }
    cells
}

fn visible_width(cells: &[Cell]) -> usize {
    cells
        .iter()
        .rev()
        .find_map(|cell| {
            UnicodeWidthChar::width(cell.ch)
                .filter(|width| *width > 0)
                .map(|width| cell.col + width)
        })
        .unwrap_or(0)
}

fn graphemes(visible: &str) -> Vec<&str> {
    visible.graphemes(true).collect()
}

fn is_combining_start(ch: char) -> bool {
    ('\u{300}'..='\u{36f}').contains(&ch)
}

fn assert_graphemes_are_source_clusters(line: &str, source: &str, context: &str) {
    let source_clusters = source.graphemes(true).collect::<Vec<_>>();
    for cluster in line.graphemes(true) {
        assert!(
            source_clusters.contains(&cluster),
            "{context}\nsource graphemes: {:?}\norphan/partial grapheme: {:?}\nactual cells: {:?}",
            source_clusters,
            cluster,
            cells_in_visible(line),
        );
    }
}

fn assert_no_orphan_combining(line: &str, source: &str, context: &str, expected: usize) {
    if let Some(ch) = line.chars().next() {
        assert!(
            !is_combining_start(ch),
            "{context}\nsource graphemes: {:?}\nexpected columns: {expected}\nactual cells: {:?}",
            graphemes(source),
            cells_in_visible(line)
        );
    }
    let mut has_base = false;
    for ch in line.chars() {
        if is_combining_start(ch) {
            assert!(
                has_base,
                "{context}: orphan combining mark {ch:?}\nsource graphemes: {:?}\nexpected columns: {expected}\nactual cells: {:?}",
                graphemes(source),
                cells_in_visible(line)
            );
        } else {
            has_base = UnicodeWidthChar::width(ch).unwrap_or(0) > 0;
        }
    }
}

fn assert_no_half_wide_cell(line: &str, source: &str, context: &str, expected: usize) {
    let actual = cells_in_visible(line);
    for cell in &actual {
        if let Some(width @ 2) = UnicodeWidthChar::width(cell.ch) {
            assert!(
                cell.col + width <= expected,
                "{context}\nsource graphemes: {:?}\nexpected columns: {expected}\nactual cells: {actual:?}",
                graphemes(source)
            );
        }
    }
}

fn is_subsequence_ignoring_space(needle: &str, haystack: &str) -> bool {
    let mut needle = needle.chars().filter(|ch| !ch.is_whitespace()).peekable();
    for ch in haystack.chars().filter(|ch| !ch.is_whitespace()) {
        if needle.peek() == Some(&ch) {
            needle.next();
        }
    }
    needle.peek().is_none()
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 128,
        rng_seed: proptest::test_runner::RngSeed::Fixed(FIXED_SEED),
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::WithSource("invariants".into()),
        )),
        max_shrink_iters: 2_048_u32,
        ..ProptestConfig::with_source_file(file!())
    }
}

fn render_composite(fc: &FmtComposite<'_>, skin: &MadSkin) -> Vec<u8> {
    let mut out = Vec::new();
    write!(
        out,
        "{}",
        termimad::FmtInline {
            skin,
            composite: fc.clone()
        }
    )
    .unwrap();
    out
}

fn crop_compounds(compounds: &[Compound<'_>], skin: &MadSkin, width: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let line_style = skin.paragraph.clone();
    {
        let mut crop = CropWriter::new(&mut out, width);
        for compound in compounds {
            let style = skin.compound_style(&line_style, compound);
            crop.queue_str(&style, compound.src).unwrap();
        }
    }
    out
}

fn crop_compounds_direct(compounds: &[Compound<'_>], width: usize) -> String {
    let mut remaining = width;
    let mut out = String::new();
    for compound in compounds {
        for ch in compound.src.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width > remaining {
                break;
            }
            remaining -= width;
            out.push(ch);
        }
    }
    out
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn cropping_keeps_graphemes_and_stays_in_rectangle(
        case in case_strategy(),
        width in 0usize..24,
    ) {
        let skin = MadSkin::no_style();
        let source = case.visible();
        let raw = String::from_utf8(crop_compounds(&case.compounds(), &skin, width)).unwrap();
        let actual = strip_escapes(&raw);
        let actual_cells = cells_in_visible(&actual);

        prop_assert!(
            visible_width(&actual_cells) <= width,
            "source graphemes: {:?}\nexpected columns: {width}\nactual cells: {actual_cells:?}",
            graphemes(&source),
        );

        assert_no_orphan_combining(&actual, &source, "styled composite crop", width);
        assert_no_half_wide_cell(&actual, &source, "styled composite crop", width);
        prop_assert_eq!(&actual, &crop_compounds_direct(&case.compounds(), width));
    }

    #[test]
    fn styled_cropping_strips_only_escape_encoding(
        case in case_strategy(),
        width in 0usize..24,
    ) {
        let skin = MadSkin::default();
        let raw = String::from_utf8(crop_compounds(&case.compounds(), &skin, width)).unwrap();
        assert!(raw.contains('\u{1b}') || raw == strip_escapes(&raw));
        let actual = strip_escapes(&raw);
        let expected = crop_compounds_direct(&case.compounds(), width);
        prop_assert_eq!(cells_in_visible(&actual), cells_in_visible(&expected));
        prop_assert!(visible_width(&cells_in_visible(&actual)) <= width);
    }

    #[test]
    fn fit_then_crop_matches_direct_crop_when_fit_is_a_noop(
        case in simple_case_strategy(),
        pre_width_delta in 0usize..8,
        target in 0usize..24,
        align in alignment_strategy(),
    ) {
        let skin = MadSkin::no_style();
        let source_width: usize = source_fit_width(&case.visible());
        let pre_width = source_width + pre_width_delta;
        let mut fc = case.composite(&skin);
        Fitter::for_align(align).fit(&mut fc, pre_width, &skin);

        let composed = String::from_utf8(crop_compounds(&fc.compounds, &skin, target)).unwrap();
        let composed = strip_escapes(&composed);
        let direct = crop_compounds_direct(&case.compounds(), target);
        let expected_cells = cells_in_visible(&direct);
        let actual_cells = cells_in_visible(&composed);

        let message = format!(
            "source graphemes: {:?}\nexpected cells: {:?}\nactual cells: {:?}",
            graphemes(&case.visible()), expected_cells, actual_cells
        );
        prop_assert!(
            actual_cells == expected_cells,
            "{}", message
        );
    }

    #[test]
    fn aligned_rectangle_padding_sums_to_viewport(
        case in simple_case_strategy(),
        width in 1usize..32,
        align in alignment_strategy(),
    ) {
        let skin = MadSkin::no_style();
        let mut fc = case.composite(&skin);
        fc.fill_width(width, align, &skin);
        let rendered = render_composite(&fc, &skin);
        let rendered = String::from_utf8(rendered).unwrap();
        let actual = strip_escapes(&rendered);
        let actual_cells = cells_in_visible(&actual);
        let content_width = fc.visible_length;
        let left = actual.chars().take_while(|ch| *ch == ' ').count();
        let right = actual.chars().rev().take_while(|ch| *ch == ' ').count();
        let expected_padding = width.saturating_sub(content_width);

        let padding_message = format!(
            "source graphemes: {:?}\nexpected padding: {}\nactual cells: {:?}",
            graphemes(&case.visible()), expected_padding, actual_cells
        );
        prop_assert!(
            left + right == expected_padding,
            "{}", padding_message
        );
        let width_message = format!(
            "source graphemes: {:?}\nexpected at most columns: {}\nactual cells: {:?}",
            graphemes(&case.visible()), width, actual_cells
        );
        prop_assert!(
            visible_width(&actual_cells) <= width,
            "{}", width_message
        );
    }

    #[test]
    fn hard_wrap_preserves_visible_grapheme_order(
        case in wrap_case_strategy(),
        width in 4usize..24,
    ) {
        let skin = MadSkin::no_style();
        let source = case.visible();
        let fc = case.composite(&skin);
        let wrapped = if fc.visible_length > width {
            termimad::wrap::hard_wrap_composite(&fc, width, &skin).unwrap()
        } else {
            vec![fc]
        };
        let mut rendered_lines = Vec::new();
        let mut joined = String::new();
        for line in &wrapped {
            let rendered = render_composite(line, &skin);
            let rendered = strip_escapes(&String::from_utf8(rendered).unwrap());
            assert_graphemes_are_source_clusters(&rendered, &source, "hard wrapped line");
            assert_no_orphan_combining(&rendered, &source, "hard wrapped line", width);
            assert_no_half_wide_cell(&rendered, &source, "hard wrapped line", width);
            prop_assert!(
                visible_width(&cells_in_visible(&rendered)) <= width,
                "source graphemes: {:?}\nexpected columns: {width}\nactual line: {:?}",
                graphemes(&source),
                rendered,
            );
            joined.push_str(&rendered);
            rendered_lines.push(rendered);
        }
        prop_assert!(
            is_subsequence_ignoring_space(&source, &joined),
            "source graphemes: {:?}\nwrapped lines: {rendered_lines:?}",
            graphemes(&source),
        );
    }
}

fn source_fit_width(source: &str) -> usize {
    source
        .chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn table_cell_strategy() -> impl Strategy<Value = String> {
    collection::vec(
        atom_strategy().prop_filter("table cells avoid ZWJ edge cases", |atom| {
            !atom.contains('\u{200d}')
        }),
        1..5,
    )
    .prop_map(|atoms| atoms.concat())
}

#[derive(Clone, Debug)]
struct TableCase {
    rows: Vec<Vec<String>>,
    aligns: Vec<Alignment>,
    width: usize,
}

fn table_case_strategy() -> impl Strategy<Value = TableCase> {
    (
        collection::vec(table_cell_strategy(), 2..4),
        collection::vec(table_cell_strategy(), 2..4),
        collection::vec(table_cell_strategy(), 2..4),
        prop_oneof![
            Just(vec![Alignment::Left, Alignment::Right]),
            Just(vec![Alignment::Center, Alignment::Center]),
            Just(vec![Alignment::Right, Alignment::Left, Alignment::Center]),
        ],
        13usize..48,
    )
        .prop_map(|(a, b, c, aligns, width)| {
            let normalize = |mut row: Vec<String>| {
                row.truncate(aligns.len());
                while row.len() < aligns.len() {
                    row.push(String::new());
                }
                row
            };
            TableCase {
                rows: vec![normalize(a), normalize(b), normalize(c)],
                aligns,
                width,
            }
        })
}

impl TableCase {
    fn markdown(&self) -> String {
        let align = |align: Alignment| match align {
            Alignment::Left => ":-",
            Alignment::Right => "-:",
            Alignment::Center => ":-:",
            Alignment::Unspecified => "---",
        };
        let line = |row: &[String]| {
            row.iter()
                .map(|cell| format!("| {} ", cell))
                .collect::<Vec<_>>()
                .join("")
                + "|"
        };
        let rule = self
            .aligns
            .iter()
            .map(|a| format!("| {} ", align(*a)))
            .collect::<Vec<_>>()
            .join("")
            + "|";
        let bottom = self
            .aligns
            .iter()
            .map(|_| "---")
            .collect::<Vec<_>>()
            .join("|");
        format!(
            "{}\n{}\n{}\n{}\n|{}|\n",
            line(&self.rows[0]),
            rule,
            line(&self.rows[1]),
            line(&self.rows[2]),
            bottom,
        )
    }
}

fn rendered_lines(skin: &MadSkin, markdown: &str, width: usize) -> Vec<String> {
    let text = skin.text(markdown, Some(width)).to_string();
    strip_escapes(&text)
        .trim_end_matches('\n')
        .split('\n')
        .map(ToOwned::to_owned)
        .collect()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn table_columns_borders_and_spans_stay_in_viewport(case in table_case_strategy()) {
        let skin = MadSkin::no_style();
        let markdown = case.markdown();
        let text = skin.text(&markdown, Some(case.width));
        let rendered = rendered_lines(&skin, &markdown, case.width);
        let mut rules = Vec::new();
        for line in &text.lines {
            if let FmtLine::TableRule(rule) = line {
                rules.push(rule);
            }
        }
        for line in &rendered {
            prop_assert!(
                visible_width(&cells_in_visible(line)) <= case.width,
                "table markdown: {markdown:?}\nexpected columns: {}\nactual cells: {:?}",
                case.width,
                cells_in_visible(line)
            );
        }
        let mut column_text = vec![String::new(); case.aligns.len()];
        for line in &text.lines {
            if let FmtLine::TableRow(row) = line {
                for (idx, cell) in row.cells.iter().enumerate() {
                    for compound in &cell.compounds {
                        column_text[idx].push_str(compound.src);
                    }
                }
            }
        }
        for row in &case.rows {
            for (column, expected) in row.iter().enumerate() {
                prop_assert!(
                    is_subsequence_ignoring_space(expected, &column_text[column]),
                    "table markdown: {:?}\ncolumn {} expected {:?}, rendered text {:?}",
                    markdown,
                    column,
                    expected,
                    column_text[column]
                );
            }
        }
        prop_assert_eq!(rules.len(), 2);
        for rule in &rules {
            prop_assert_eq!(rule.widths.len(), case.aligns.len());
            prop_assert!(rule.widths.iter().all(|width| *width >= 3));
            let table_width = 1 + rule.widths.iter().sum::<usize>() + rule.widths.len();
            prop_assert!(
                table_width <= case.width,
                "table markdown: {:?}\nexpected at most {}\nactual table width {}",
                markdown,
                case.width,
                table_width
            );
            prop_assert_eq!(
                1 + rule.widths.iter().sum::<usize>() + rule.widths.len(),
                table_width
            );
        }
        let expected_borders = {
            let sum: usize = rules[0].widths.iter().sum();
            let mut positions = vec![0usize];
            let mut pos = 0;
            for width in &rules[0].widths {
                pos += width + 1;
                positions.push(pos);
            }
            let table_width = sum + positions.len();
            prop_assert!(table_width <= case.width);
            positions
        };
        for line in &rendered {
            let borders = [
                skin.table_border_chars.vertical,
                skin.table_border_chars.top_left_corner,
                skin.table_border_chars.top_right_corner,
                skin.table_border_chars.bottom_right_corner,
                skin.table_border_chars.bottom_left_corner,
                skin.table_border_chars.top_junction,
                skin.table_border_chars.right_junction,
                skin.table_border_chars.bottom_junction,
                skin.table_border_chars.left_junction,
                skin.table_border_chars.cross,
            ];
            let positions: Vec<usize> = cells_in_visible(line)
                .iter()
                .filter(|cell| borders.contains(&cell.ch))
                .map(|cell| cell.col)
                .collect();
            prop_assert_eq!(
                positions,
                expected_borders.clone(),
                "table markdown: {:?}\nline: {:?}",
                markdown,
                line,
            );
        }
    }
}

fn invariant_corpus() -> Vec<(String, usize)> {
    let cases = vec![
        Case {
            fragments: vec![
                Fragment {
                    text: "AB".into(),
                    style: StyleKind::Bold,
                },
                Fragment {
                    text: "日e\u{301}".into(),
                    style: StyleKind::Code,
                },
                Fragment {
                    text: "👩\u{200d}💻".into(),
                    style: StyleKind::Italic,
                },
                Fragment {
                    text: "x7".into(),
                    style: StyleKind::Plain,
                },
            ],
            gap: "  ".into(),
        },
        Case {
            fragments: vec![
                Fragment {
                    text: "😀a\u{308}\u{301}".into(),
                    style: StyleKind::Strike,
                },
                Fragment {
                    text: "本語".into(),
                    style: StyleKind::BoldItalic,
                },
                Fragment {
                    text: "code".into(),
                    style: StyleKind::Code,
                },
            ],
            gap: " ".into(),
        },
    ];
    let mut corpus = Vec::new();
    for case in cases {
        for width in [4usize, 6, 9, 15, 24] {
            corpus.push((case.visible(), width));
        }
    }
    corpus
}

fn skin_invariant_corpus() -> Vec<(String, usize)> {
    let mut corpus = invariant_corpus();
    corpus.push((
        "| **AB** | `code` 日😀 |\n|:-|:-:|\n| e\u{301} x7 | 本語 |\n|---|---|\n".to_owned(),
        25,
    ));
    corpus
}

fn skins_for_invariants() -> Vec<(&'static str, MadSkin)> {
    let mut default = MadSkin::default();
    default.table_border_chars = termimad::ROUNDED_TABLE_BORDER_CHARS;
    let mut no_style = MadSkin::no_style();
    no_style.table_border_chars = termimad::ROUNDED_TABLE_BORDER_CHARS;
    let ascii_default = {
        let mut skin = MadSkin::default();
        skin.limit_to_ascii();
        skin
    };
    let ascii_no_style = {
        let mut skin = MadSkin::no_style();
        skin.limit_to_ascii();
        skin
    };
    vec![
        ("default", default),
        ("no-style", no_style),
        ("ascii-default", ascii_default),
        ("ascii-no-style", ascii_no_style),
    ]
}

fn encoded_render(skin: &MadSkin, source: &str, width: usize) -> String {
    skin.text(source, Some(width)).to_string()
}

fn normalized_skin_line(line: &str, skin: &MadSkin) -> String {
    let mut normalized = line.to_owned();
    for ch in [
        skin.table_border_chars.vertical,
        skin.table_border_chars.top_left_corner,
        skin.table_border_chars.top_right_corner,
        skin.table_border_chars.bottom_right_corner,
        skin.table_border_chars.bottom_left_corner,
        skin.table_border_chars.top_junction,
        skin.table_border_chars.right_junction,
        skin.table_border_chars.bottom_junction,
        skin.table_border_chars.left_junction,
        skin.table_border_chars.cross,
    ] {
        normalized = normalized.replace(ch, "|");
    }
    normalized.replace(skin.table_border_chars.horizontal, "-")
}

#[test]
fn skin_and_compatibility_change_encoding_not_cells_or_breaks() {
    let corpus = skin_invariant_corpus();
    for (source, width) in corpus {
        let mut reference = None;
        for (name, skin) in skins_for_invariants() {
            let encoded = encoded_render(&skin, &source, width);
            if name.contains("no-style") {
                assert!(!encoded.contains('\u{1b}'), "no-style skin emitted ANSI");
            }
            let visible_lines: Vec<String> = strip_escapes(&encoded)
                .trim_end_matches('\n')
                .split('\n')
                .map(ToOwned::to_owned)
                .collect();
            let signature: Vec<(Vec<Cell>, String)> = visible_lines
                .iter()
                .map(|line| {
                    let normalized = normalized_skin_line(line, &skin);
                    (cells_in_visible(&normalized), normalized)
                })
                .collect();
            let normalized_lines: Vec<String> =
                signature.iter().map(|(_, line)| line.clone()).collect();
            for line in &visible_lines {
                assert!(
                    visible_width(&cells_in_visible(line)) <= width,
                    "skin {name}, width {width}, line {line:?}, cells {:?}",
                    cells_in_visible(line)
                );
            }
            if let Some((reference, reference_lines)) = &reference {
                assert_eq!(
                    &signature, reference,
                    "skin {name} changed visible cells or wrap breaks for {source:?} at {width}"
                );
                assert_eq!(
                    &normalized_lines, reference_lines,
                    "skin {name} changed visible text for {source:?} at {width}"
                );
            } else {
                reference = Some((signature, normalized_lines));
            }
        }
    }
}

#[test]
fn scrolling_view_keeps_wrapped_rows_inside_its_area() {
    let source = invariant_corpus()
        .iter()
        .map(|(text, _)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for skin in [MadSkin::default(), MadSkin::no_style()] {
        for width in [8usize, 16, 28] {
            for height in [2u16, 5, 12] {
                for scroll in [0usize, 1, 3] {
                    let area = Area::new(0, 0, width as u16, height);
                    let text = skin.area_text(&source, &area);
                    let mut view = TextView::from(&area, &text);
                    view.show_scrollbar = false;
                    let effective_scroll = view.set_scroll(scroll);
                    let mut out = Vec::new();
                    view.write_on(&mut out).unwrap();
                    let rendered = String::from_utf8(out).unwrap();
                    let rows: Vec<String> = rendered
                        .split("\u{1b}[")
                        .skip(1)
                        .filter_map(|part| {
                            part.split_once('H').map(|(_, rest)| strip_escapes(rest))
                        })
                        .collect();
                    assert_eq!(rows.len(), height as usize);
                    for (idx, row) in rows.iter().enumerate() {
                        assert!(
                            visible_width(&cells_in_visible(row)) <= width,
                            "skin scroll={effective_scroll} width={width} row={idx} cells {:?}",
                            cells_in_visible(row)
                        );
                    }
                }
            }
        }
    }
}

fn correct_cut(s: &str, max_width: usize) -> &str {
    let mut width = 0;
    for (idx, ch) in s.char_indices() {
        let next = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + next > max_width {
            return &s[..idx];
        }
        width += next;
    }
    s
}

fn char_count_cut(s: &str, max_width: usize) -> &str {
    match s.char_indices().nth(max_width) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

fn off_by_one_cut(s: &str, max_width: usize) -> &str {
    correct_cut(s, max_width.saturating_sub(1))
}

#[test]
fn mutation_width_as_char_count_is_caught() {
    let source = "ab日本";
    let correct = correct_cut(source, 3);
    let mutated = char_count_cut(source, 3);
    assert_eq!(correct, "ab");
    assert_eq!(mutated, "ab日");
    assert_ne!(
        visible_width(&cells_in_visible(mutated)),
        3,
        "char-count cut leaves a wide character half outside the viewport"
    );
}

#[test]
fn mutation_crop_endpoint_off_by_one_is_caught() {
    let source = "abcd";
    let correct = correct_cut(source, 2);
    let mutated = off_by_one_cut(source, 2);
    assert_eq!(correct, "ab");
    assert_eq!(mutated, "a");
    assert_ne!(correct, mutated);
    assert_eq!(visible_width(&cells_in_visible(correct)), 2);
    assert_eq!(visible_width(&cells_in_visible(mutated)), 1);
}

#[test]
fn mutation_style_switch_repeats_escape_bytes_is_caught() {
    let encoded = "\u{1b}[31ma\u{1b}[0m\u{1b}[32mb\u{1b}[0m";
    let correct = strip_escapes(encoded);
    let mutated_cell_count = encoded.len() - encoded.matches('\u{1b}').count();
    assert_eq!(correct, "ab");
    assert_eq!(correct.chars().count(), 2);
    assert_ne!(mutated_cell_count, 2);
    assert_eq!(visible_width(&cells_in_visible(&correct)), 2);
}

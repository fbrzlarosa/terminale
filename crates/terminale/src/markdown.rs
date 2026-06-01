//! Streaming-tolerant markdown renderer for AI assistant replies.
//!
//! Designed to work with egui 0.29. The renderer is invoked every frame with
//! the full accumulated assistant text, parses it in O(n) into a `Vec<Block>`,
//! then emits egui widgets.
//!
//! # Streaming safety
//!
//! A fenced code block that has not yet received its closing ``` is emitted as
//! `CodeBlock { closed: false }` and rendered identically to a closed block so
//! the UI never flashes literal backtick characters during streaming.
//!
//! # Interactive elements in paragraphs
//!
//! egui's `LayoutJob` cannot host interactive elements (hyperlinks). Paragraphs
//! that contain a `Link` or inline `Code` span use `ui.horizontal_wrapped` with
//! one widget per inline run — this sacrifices ideal kerning at run boundaries
//! but is the only path that keeps links clickable.

use egui::{Color32, FontFamily, FontId, RichText};

// ────────────────────────────────────────────────────────────────────────────
// Public surface (crate-local only)
// ────────────────────────────────────────────────────────────────────────────

/// A top-level document block produced by the parser.
#[derive(Debug, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum Block {
    /// A regular paragraph of inline-styled text.
    Paragraph(Vec<Inline>),
    /// A fenced code block.
    CodeBlock {
        /// Optional language info-string (first word after the opening fence).
        lang: Option<String>,
        /// The raw body text inside the fence.
        body: String,
        /// `false` while the closing fence has not yet arrived (streaming).
        closed: bool,
    },
    /// A heading (`#` through `####`).
    Heading {
        /// 1 = `#`, 2 = `##`, 3 = `###`, 4+ = `####`.
        level: u8,
        /// Inline spans inside the heading line.
        content: Vec<Inline>,
    },
    /// An unordered list (`-`, `*`, or `+` bullets).
    BulletList(Vec<Vec<Inline>>),
    /// An ordered list (`1.`, `2.`, …).
    OrderedList(Vec<Vec<Inline>>),
}

/// An inline span inside a block.
#[derive(Debug, PartialEq)]
pub(crate) enum Inline {
    /// Plain text.
    Text(String),
    /// Inline code (backtick-delimited).
    Code(String),
    /// Bold emphasis.
    Bold(Vec<Inline>),
    /// Italic emphasis.
    Italic(Vec<Inline>),
    /// A hyperlink `[label](url)`.
    Link {
        /// Display label.
        label: String,
        /// Target URL.
        url: String,
    },
}

// ────────────────────────────────────────────────────────────────────────────
// Entry-point
// ────────────────────────────────────────────────────────────────────────────

/// Render `text` inside `ui`.
///
/// When `enabled` is `false`, fall back to a plain-text `ui.label` exactly
/// matching the pre-markdown baseline behaviour so disabling the toggle is a
/// true escape hatch with no visual change.
///
/// Applies a 64 KiB guard: messages larger than that are rendered as raw
/// monospace to avoid pathological re-parses (defensive; never hit in practice).
pub(crate) fn render(ui: &mut egui::Ui, text: &str, enabled: bool) {
    const MAX_BYTES: usize = 64 * 1024;
    if !enabled || text.len() > MAX_BYTES {
        ui.label(RichText::new(text).color(TEXT_COLOR));
        return;
    }
    let blocks = parse(text);
    for block in &blocks {
        render_block(ui, block);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Colour palette
// ────────────────────────────────────────────────────────────────────────────

/// Default body text colour — matches the pre-markdown `ui.label` colour.
const TEXT_COLOR: Color32 = Color32::from_rgb(220, 226, 240);
/// Dim accent for secondary labels (lang tag, etc.).
const DIM_COLOR: Color32 = Color32::from_rgb(100, 110, 140);
/// Monospace code body colour.
const CODE_BODY_COLOR: Color32 = Color32::from_rgb(220, 226, 240);
/// Inline-code background.
const INLINE_CODE_BG: Color32 = Color32::from_rgb(28, 32, 42);

// ────────────────────────────────────────────────────────────────────────────
// Parser
// ────────────────────────────────────────────────────────────────────────────

/// Parse `src` into a sequence of [`Block`]s.
///
/// The algorithm is line-based and stateful, designed so that every prefix of
/// a valid document produces a structurally sound `Vec<Block>` — unclosed
/// fences produce `CodeBlock { closed: false }` rather than leaking raw
/// backticks into prose.
pub(crate) fn parse(src: &str) -> Vec<Block> {
    let lines: Vec<&str> = src.split('\n').collect();
    let mut blocks: Vec<Block> = Vec::new();
    let mut i = 0;
    // Buffer for paragraph lines accumulated before a block-level break.
    let mut para_buf: Vec<&str> = Vec::new();

    /// Flush accumulated paragraph lines as a `Block::Paragraph`.
    fn flush_para(buf: &mut Vec<&str>, blocks: &mut Vec<Block>) {
        if buf.is_empty() {
            return;
        }
        let joined = buf.join(" ");
        buf.clear();
        let inlines = parse_inline(&joined);
        if !inlines.is_empty() {
            blocks.push(Block::Paragraph(inlines));
        }
    }

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // ── Fenced code block ──────────────────────────────────────────────
        if let Some(fence_info) = detect_fence_open(trimmed) {
            flush_para(&mut para_buf, &mut blocks);
            let (fence_char, fence_len, lang) = fence_info;
            i += 1;
            let mut body_lines: Vec<&str> = Vec::new();
            let mut closed = false;
            while i < lines.len() {
                let bl = lines[i];
                if is_fence_close(bl.trim_start(), fence_char, fence_len) {
                    closed = true;
                    i += 1;
                    break;
                }
                body_lines.push(bl);
                i += 1;
            }
            blocks.push(Block::CodeBlock {
                lang,
                body: body_lines.join("\n"),
                closed,
            });
            continue;
        }

        // ── Heading ────────────────────────────────────────────────────────
        if let Some((level, content)) = detect_heading(trimmed) {
            flush_para(&mut para_buf, &mut blocks);
            blocks.push(Block::Heading {
                level,
                content: parse_inline(content),
            });
            i += 1;
            continue;
        }

        // ── Bullet list ────────────────────────────────────────────────────
        if is_bullet_item(trimmed) {
            flush_para(&mut para_buf, &mut blocks);
            let mut items: Vec<Vec<Inline>> = Vec::new();
            while i < lines.len() && is_bullet_item(lines[i].trim_start()) {
                let rest = bullet_item_content(lines[i].trim_start());
                items.push(parse_inline(rest));
                i += 1;
            }
            blocks.push(Block::BulletList(items));
            continue;
        }

        // ── Ordered list ───────────────────────────────────────────────────
        if is_ordered_item(trimmed) {
            flush_para(&mut para_buf, &mut blocks);
            let mut items: Vec<Vec<Inline>> = Vec::new();
            while i < lines.len() && is_ordered_item(lines[i].trim_start()) {
                let rest = ordered_item_content(lines[i].trim_start());
                items.push(parse_inline(rest));
                i += 1;
            }
            blocks.push(Block::OrderedList(items));
            continue;
        }

        // ── Blank line — paragraph break ───────────────────────────────────
        if trimmed.is_empty() {
            flush_para(&mut para_buf, &mut blocks);
            i += 1;
            continue;
        }

        // ── Regular paragraph line ─────────────────────────────────────────
        para_buf.push(line);
        i += 1;
    }

    flush_para(&mut para_buf, &mut blocks);
    blocks
}

// ────────────────────────────────────────────────────────────────────────────
// Line-level detection helpers
// ────────────────────────────────────────────────────────────────────────────

/// Detect an opening code fence on `trimmed`.
///
/// Returns `(fence_char, fence_len, lang_tag)` when a fence is found.
/// The lang tag is the first whitespace-delimited token of the info-string
/// (CommonMark §4.5) with extra tokens discarded.
fn detect_fence_open(trimmed: &str) -> Option<(char, usize, Option<String>)> {
    let fence_char = if trimmed.starts_with('`') {
        '`'
    } else if trimmed.starts_with('~') {
        '~'
    } else {
        return None;
    };
    let fence_len = trimmed.chars().take_while(|&c| c == fence_char).count();
    if fence_len < 3 {
        return None;
    }
    let rest = &trimmed[fence_len..];
    let lang = rest
        .split_whitespace()
        .next()
        .filter(|s| !s.is_empty())
        .map(String::from);
    Some((fence_char, fence_len, lang))
}

/// Return `true` when `trimmed` is a valid closing fence for an opening fence
/// of `fence_char` with length `open_len` (CommonMark: ≥ open_len same chars,
/// no non-space after the run).
fn is_fence_close(trimmed: &str, fence_char: char, open_len: usize) -> bool {
    let run = trimmed.chars().take_while(|&c| c == fence_char).count();
    if run < open_len {
        return false;
    }
    trimmed[run..].trim().is_empty()
}

/// Detect a heading. Returns `(level, rest_of_line)` or `None`.
fn detect_heading(trimmed: &str) -> Option<(u8, &str)> {
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if hashes > 4 {
        return None;
    }
    let rest = &trimmed[hashes..];
    // Must be followed by a space (CommonMark §4.2).
    if !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, rest.trim_start()))
}

/// `true` if `trimmed` begins a bullet list item (`- `, `* `, `+ `).
fn is_bullet_item(trimmed: &str) -> bool {
    matches!(trimmed.chars().next(), Some('-') | Some('*') | Some('+'))
        && trimmed.len() >= 2
        && trimmed.as_bytes().get(1).copied() == Some(b' ')
}

/// Strip the bullet marker and return the content.
fn bullet_item_content(trimmed: &str) -> &str {
    trimmed[2..].trim_start()
}

/// `true` if `trimmed` begins an ordered list item (`N. `).
fn is_ordered_item(trimmed: &str) -> bool {
    let digits: usize = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let rest = &trimmed[digits..];
    rest.starts_with(". ")
}

/// Strip the ordered-list marker and return the content.
fn ordered_item_content(trimmed: &str) -> &str {
    let digits: usize = trimmed.chars().take_while(char::is_ascii_digit).count();
    trimmed[digits + 2..].trim_start()
}

// ────────────────────────────────────────────────────────────────────────────
// Inline parser
// ────────────────────────────────────────────────────────────────────────────

/// Parse a string into a sequence of [`Inline`] spans.
///
/// Streaming safety: any unclosed `**bold`, `*italic*, or `` `code `` run at
/// end-of-string is emitted as plain `Inline::Text` prefixed by the literal
/// opening marker so no characters are ever swallowed.
pub(crate) fn parse_inline(src: &str) -> Vec<Inline> {
    let bytes = src.as_bytes();
    let mut spans: Vec<Inline> = Vec::new();
    let mut text_start = 0usize;
    let mut i = 0usize;

    /// Flush any accumulated text bytes `bytes[text_start..i]` as `Inline::Text`.
    macro_rules! flush_text {
        ($spans:expr, $bytes:expr, $start:expr, $end:expr) => {
            if $start < $end {
                let s = std::str::from_utf8(&$bytes[$start..$end]).unwrap_or_default();
                if !s.is_empty() {
                    $spans.push(Inline::Text(s.to_string()));
                }
            }
        };
    }

    while i < bytes.len() {
        // ── Inline code: backtick ──────────────────────────────────────────
        if bytes[i] == b'`' {
            flush_text!(spans, bytes, text_start, i);
            i += 1; // skip opening `
            let code_start = i;
            while i < bytes.len() && bytes[i] != b'`' {
                i += 1;
            }
            if i < bytes.len() {
                // Found closing backtick.
                let inner = std::str::from_utf8(&bytes[code_start..i]).unwrap_or_default();
                spans.push(Inline::Code(inner.to_string()));
                i += 1; // skip closing `
            } else {
                // Unclosed: emit literal ` + remainder as Text.
                let inner = std::str::from_utf8(&bytes[code_start..]).unwrap_or_default();
                spans.push(Inline::Text(format!("`{inner}")));
            }
            text_start = i;
            continue;
        }

        // ── Bold / italic: asterisk ────────────────────────────────────────
        if bytes[i] == b'*' {
            // Check for `**` (bold).
            if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                // Try to find a closing `**`.
                let inner_start = i + 2;
                if let Some(close) = find_closing_double_star(bytes, inner_start) {
                    flush_text!(spans, bytes, text_start, i);
                    let inner = std::str::from_utf8(&bytes[inner_start..close]).unwrap_or_default();
                    let inner_spans = parse_inline(inner);
                    spans.push(Inline::Bold(inner_spans));
                    i = close + 2;
                    text_start = i;
                    continue;
                }
                // No closing `**` found — emit `**` as text and advance past them.
                flush_text!(spans, bytes, text_start, i);
                spans.push(Inline::Text("**".to_string()));
                i += 2;
                text_start = i;
                continue;
            }

            // Single `*` italic: must be left-flanking (next char is not space).
            let next_is_space = i + 1 >= bytes.len() || bytes[i + 1] == b' ';
            if !next_is_space {
                if let Some(close) = find_closing_single_star(bytes, i + 1) {
                    flush_text!(spans, bytes, text_start, i);
                    let inner_start = i + 1;
                    let inner = std::str::from_utf8(&bytes[inner_start..close]).unwrap_or_default();
                    let inner_spans = parse_inline(inner);
                    spans.push(Inline::Italic(inner_spans));
                    i = close + 1;
                    text_start = i;
                    continue;
                }
            }
            // Not a valid emphasis opener — treat as text.
            i += 1;
            continue;
        }

        // ── Link: [label](url) ─────────────────────────────────────────────
        if bytes[i] == b'[' {
            if let Some((label, url, end)) = parse_link(bytes, i) {
                flush_text!(spans, bytes, text_start, i);
                spans.push(Inline::Link { label, url });
                i = end;
                text_start = i;
                continue;
            }
        }

        i += 1;
    }

    flush_text!(spans, bytes, text_start, i);
    spans
}

/// Find the first `**` closing marker after `start`, skipping escaped chars.
/// Returns the byte index of the first `*` of `**`.
fn find_closing_double_star(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Find the first single `*` closing marker after `start` that is not part of
/// a `**` run. Returns the byte index of the `*`.
fn find_closing_single_star(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'*' {
            // Not part of a `**` double run.
            let prev_star = i > 0 && bytes[i - 1] == b'*';
            let next_star = i + 1 < bytes.len() && bytes[i + 1] == b'*';
            if !prev_star && !next_star {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Parse `[label](url)` starting at byte index `start`.
/// Returns `(label, url, end_index)` or `None`.
fn parse_link(bytes: &[u8], start: usize) -> Option<(String, String, usize)> {
    debug_assert_eq!(bytes[start], b'[');
    // Find closing `]`.
    let bracket_close = bytes[start + 1..]
        .iter()
        .position(|&b| b == b']')
        .map(|p| start + 1 + p)?;
    // Must be followed by `(`.
    if bytes.get(bracket_close + 1).copied() != Some(b'(') {
        return None;
    }
    let paren_open = bracket_close + 1;
    let paren_close = bytes[paren_open + 1..]
        .iter()
        .position(|&b| b == b')')
        .map(|p| paren_open + 1 + p)?;

    let label = std::str::from_utf8(&bytes[start + 1..bracket_close])
        .ok()?
        .to_string();
    let url = std::str::from_utf8(&bytes[paren_open + 1..paren_close])
        .ok()?
        .to_string();
    Some((label, url, paren_close + 1))
}

// ────────────────────────────────────────────────────────────────────────────
// Renderer
// ────────────────────────────────────────────────────────────────────────────

fn render_block(ui: &mut egui::Ui, block: &Block) {
    match block {
        Block::Paragraph(inlines) => {
            render_inlines(ui, inlines);
        }
        Block::CodeBlock { lang, body, .. } => {
            render_code_block(ui, lang.as_deref(), body);
        }
        Block::Heading { level, content } => {
            ui.add_space(4.0);
            render_heading(ui, *level, content);
        }
        Block::BulletList(items) => {
            for item in items {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(RichText::new("•").color(DIM_COLOR));
                    ui.add_space(4.0);
                    render_inlines(ui, item);
                });
            }
        }
        Block::OrderedList(items) => {
            for (idx, item) in items.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(RichText::new(format!("{}.", idx + 1)).color(DIM_COLOR));
                    ui.add_space(4.0);
                    render_inlines(ui, item);
                });
            }
        }
    }
}

fn render_heading(ui: &mut egui::Ui, level: u8, content: &[Inline]) {
    // Collect text for simple headings (just Text spans).
    let plain: String = content.iter().map(inline_plain_text).collect();
    let rt = match level {
        1 => RichText::new(&plain).heading().color(TEXT_COLOR),
        2 => RichText::new(&plain).size(16.0).strong().color(TEXT_COLOR),
        _ => RichText::new(&plain).size(14.0).strong().color(TEXT_COLOR),
    };
    ui.label(rt);
}

/// Extract plain-text content from an inline span (used for headings).
fn inline_plain_text(inline: &Inline) -> String {
    match inline {
        Inline::Text(s) | Inline::Code(s) => s.clone(),
        Inline::Bold(inner) | Inline::Italic(inner) => {
            inner.iter().map(inline_plain_text).collect()
        }
        Inline::Link { label, .. } => label.clone(),
    }
}

fn render_code_block(ui: &mut egui::Ui, lang: Option<&str>, body: &str) {
    let frame = egui::Frame::default()
        .fill(Color32::from_rgb(20, 23, 32))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(45, 50, 65)))
        .rounding(6.0)
        .inner_margin(egui::Margin::symmetric(10.0, 8.0));

    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        // Header: lang label on left, Copy button on right.
        ui.horizontal(|ui| {
            if let Some(lang) = lang {
                ui.label(RichText::new(lang).small().color(DIM_COLOR));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Copy").clicked() {
                    ui.output_mut(|o| o.copied_text = body.to_string());
                }
            });
        });
        ui.label(RichText::new(body).monospace().color(CODE_BODY_COLOR));
    });
}

/// Render a sequence of inline spans.
///
/// If the sequence contains any `Link` or `Code` spans, uses
/// `ui.horizontal_wrapped` with one widget per run (the only approach that
/// keeps links interactive). Pure text/bold/italic paragraphs use a single
/// `ui.label` for lower overhead.
fn render_inlines(ui: &mut egui::Ui, inlines: &[Inline]) {
    if inlines.is_empty() {
        return;
    }

    let has_interactive = inlines
        .iter()
        .any(|s| matches!(s, Inline::Code(_) | Inline::Link { .. }) || has_interactive_nested(s));

    if has_interactive {
        ui.horizontal_wrapped(|ui| {
            for span in inlines {
                render_inline_span(ui, span);
            }
        });
    } else {
        // Cheap single-label path for plain text paragraphs.
        // Build a LayoutJob so bold and italic render correctly.
        let job = build_layout_job(inlines);
        ui.label(job);
    }
}

/// Whether a nested inline contains an interactive span.
fn has_interactive_nested(inline: &Inline) -> bool {
    match inline {
        Inline::Bold(inner) | Inline::Italic(inner) => inner.iter().any(|s| {
            matches!(s, Inline::Code(_) | Inline::Link { .. }) || has_interactive_nested(s)
        }),
        _ => false,
    }
}

/// Render a single inline span as an egui widget.
fn render_inline_span(ui: &mut egui::Ui, inline: &Inline) {
    match inline {
        Inline::Text(s) => {
            ui.label(RichText::new(s).color(TEXT_COLOR));
        }
        Inline::Code(s) => {
            // Inline code: monospace in a lightly tinted background.
            egui::Frame::default()
                .fill(INLINE_CODE_BG)
                .rounding(3.0)
                .inner_margin(egui::Margin::symmetric(3.0, 1.0))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(s)
                            .monospace()
                            .size(12.5)
                            .color(CODE_BODY_COLOR),
                    );
                });
        }
        Inline::Bold(inner) => {
            // Flatten bold spans into their content, rendered strong.
            for span in inner {
                match span {
                    Inline::Text(s) => {
                        ui.label(RichText::new(s).strong().color(Color32::WHITE));
                    }
                    _ => render_inline_span(ui, span),
                }
            }
        }
        Inline::Italic(inner) => {
            for span in inner {
                match span {
                    Inline::Text(s) => {
                        ui.label(RichText::new(s).italics().color(TEXT_COLOR));
                    }
                    _ => render_inline_span(ui, span),
                }
            }
        }
        Inline::Link { label, url } => {
            ui.hyperlink_to(label, url);
        }
    }
}

/// Build an `egui::text::LayoutJob` for a sequence of non-interactive inline
/// spans (Text, Bold, Italic only). Used for the cheap single-label path.
fn build_layout_job(inlines: &[Inline]) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_rows: usize::MAX,
            break_anywhere: false,
            ..Default::default()
        },
        ..Default::default()
    };
    for inline in inlines {
        append_inline_to_job(inline, &mut job, false, false);
    }
    job
}

/// Recursively append an inline span to a `LayoutJob`.
fn append_inline_to_job(
    inline: &Inline,
    job: &mut egui::text::LayoutJob,
    bold: bool,
    italic: bool,
) {
    match inline {
        Inline::Text(s) => {
            job.append(
                s,
                0.0,
                egui::text::TextFormat {
                    font_id: FontId::new(13.0, FontFamily::Proportional),
                    color: if bold { Color32::WHITE } else { TEXT_COLOR },
                    italics: italic,
                    ..Default::default()
                },
            );
        }
        Inline::Bold(inner) => {
            for span in inner {
                append_inline_to_job(span, job, true, italic);
            }
        }
        Inline::Italic(inner) => {
            for span in inner {
                append_inline_to_job(span, job, bold, true);
            }
        }
        // Code and Link should not reach here (handled by the interactive path).
        Inline::Code(s) => {
            job.append(
                s,
                0.0,
                egui::text::TextFormat {
                    font_id: FontId::monospace(12.5),
                    color: CODE_BODY_COLOR,
                    background: INLINE_CODE_BG,
                    ..Default::default()
                },
            );
        }
        Inline::Link { label, .. } => {
            job.append(
                label,
                0.0,
                egui::text::TextFormat {
                    font_id: FontId::new(13.0, FontFamily::Proportional),
                    color: Color32::from_rgb(100, 160, 255),
                    ..Default::default()
                },
            );
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Block-level parsing ────────────────────────────────────────────────

    #[test]
    fn parses_fenced_code_block_closed() {
        let src = "Here:\n```rust\nfn main() {}\n```\nAfter.";
        let blocks = parse(src);
        let code = blocks
            .iter()
            .find(|b| matches!(b, Block::CodeBlock { .. }))
            .expect("should have a CodeBlock");
        match code {
            Block::CodeBlock { lang, body, closed } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert_eq!(body, "fn main() {}");
                assert!(*closed, "block should be closed");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn parses_fenced_code_block_unclosed() {
        // Streaming case: closing fence never arrives.
        let src = "Here:\n```rust\nfn main() {";
        let blocks = parse(src);
        let code = blocks
            .iter()
            .find(|b| matches!(b, Block::CodeBlock { .. }))
            .expect("should have a CodeBlock");
        match code {
            Block::CodeBlock { lang, body, closed } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert_eq!(body, "fn main() {");
                assert!(!*closed, "block should be open (streaming)");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn parses_inline_code() {
        let spans = parse_inline("use `cargo test` to run");
        assert!(
            spans.contains(&Inline::Code("cargo test".to_string())),
            "should contain inline code span"
        );
    }

    #[test]
    fn parses_bold() {
        let spans = parse_inline("**hello world**");
        assert_eq!(
            spans,
            vec![Inline::Bold(vec![Inline::Text("hello world".to_string())])]
        );
    }

    #[test]
    fn parses_italic() {
        let spans = parse_inline("*hello*");
        assert_eq!(
            spans,
            vec![Inline::Italic(vec![Inline::Text("hello".to_string())])]
        );
    }

    #[test]
    fn parses_nested_bold_italic() {
        let spans = parse_inline("**bold *and italic***");
        // After bold opener, inner text contains `*and italic*` — nested italic.
        // Just verify top-level is Bold.
        assert!(matches!(spans[0], Inline::Bold(_)));
    }

    #[test]
    fn parses_heading_level1() {
        let blocks = parse("# Hello World");
        assert_eq!(
            blocks,
            vec![Block::Heading {
                level: 1,
                content: vec![Inline::Text("Hello World".to_string())]
            }]
        );
    }

    #[test]
    fn parses_heading_levels() {
        for (src, expected_level) in [("# H1", 1u8), ("## H2", 2), ("### H3", 3), ("#### H4", 4)] {
            let blocks = parse(src);
            match &blocks[0] {
                Block::Heading { level, .. } => assert_eq!(*level, expected_level),
                _ => panic!("expected Heading for {src}"),
            }
        }
    }

    #[test]
    fn heading_requires_space_after_hash() {
        // `#tag` is NOT a heading.
        let blocks = parse("#notaheading");
        assert!(
            !blocks.iter().any(|b| matches!(b, Block::Heading { .. })),
            "#notaheading must not produce a Heading"
        );
    }

    #[test]
    fn parses_bullet_list() {
        let src = "- alpha\n- beta\n- gamma";
        let blocks = parse(src);
        match &blocks[0] {
            Block::BulletList(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], vec![Inline::Text("alpha".to_string())]);
            }
            _ => panic!("expected BulletList"),
        }
    }

    #[test]
    fn parses_ordered_list() {
        let src = "1. one\n2. two\n3. three";
        let blocks = parse(src);
        match &blocks[0] {
            Block::OrderedList(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], vec![Inline::Text("one".to_string())]);
            }
            _ => panic!("expected OrderedList"),
        }
    }

    #[test]
    fn parses_link() {
        let spans = parse_inline("[GitHub](https://github.com)");
        assert_eq!(
            spans,
            vec![Inline::Link {
                label: "GitHub".to_string(),
                url: "https://github.com".to_string()
            }]
        );
    }

    #[test]
    fn single_star_not_flanked_is_literal() {
        // A lone `*` surrounded by spaces is NOT italic.
        let spans = parse_inline("a * b");
        // The `*` should end up as plain Text (the star is space-flanked).
        let joined: String = spans
            .iter()
            .map(|s| match s {
                Inline::Text(t) => t.as_str(),
                _ => "",
            })
            .collect();
        assert!(
            !spans.iter().any(|s| matches!(s, Inline::Italic(_))),
            "space-flanked `*` must not produce Italic; got: {joined}"
        );
    }

    // ── Streaming property test ────────────────────────────────────────────

    #[test]
    fn streaming_prefix_stability() {
        let sample = "Intro paragraph.\n\n```rust\nfn hello() {\n    println!(\"hi\");\n}\n```\n\n**Bold** and *italic* and `code`.\n\n- item one\n- item two\n\n[Link](https://example.com)\n";

        let mut prev_fence_body: Option<String> = None;

        for end in 1..=sample.len() {
            // Only slice on valid UTF-8 boundaries.
            if !sample.is_char_boundary(end) {
                continue;
            }
            let prefix = &sample[..end];
            // Must not panic.
            let blocks = parse(prefix);

            // Verify: open code block body is a prefix of the final body.
            for block in &blocks {
                if let Block::CodeBlock { body, closed, .. } = block {
                    if !closed {
                        if let Some(prev) = &prev_fence_body {
                            // The new body must extend or equal the previous one.
                            assert!(
                                body.starts_with(prev.as_str()) || prev.starts_with(body.as_str()),
                                "open code block body should be prefix-monotone at offset {end}"
                            );
                        }
                        prev_fence_body = Some(body.clone());
                    }
                }
            }
        }
    }

    // ── Regression: Inject button survives markdown rendering ─────────────

    #[test]
    fn inject_command_still_detectable_with_bold_outside_fence() {
        // The `extract_command` function (in ai_assistant_window.rs) must still
        // find the fenced command even when prose around it contains bold.
        // This test just verifies the parse doesn't swallow the fence.
        let src = "**Use this command:**\n\n```bash\nls -la\n```";
        let blocks = parse(src);
        let has_code = blocks.iter().any(|b| {
            matches!(b, Block::CodeBlock { lang, body, .. }
                if lang.as_deref() == Some("bash") && body.contains("ls -la"))
        });
        assert!(has_code, "fenced block must survive bold prose around it");
    }
}

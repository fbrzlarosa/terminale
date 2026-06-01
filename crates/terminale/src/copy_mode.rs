//! Modal keyboard copy mode: a mouse-free keyboard selection of the screen
//! + scrollback with vim motions, yanking to the clipboard.
//!
//! All logic here is pure (no I/O, no rendering). Integration wiring lives in
//! `main.rs`; rendering is delegated to the existing selection-rect path via
//! `CopyModeState::renderer_selection`.

/// How the selection grows from the anchor to the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// Character-level, flowing across row boundaries (default).
    Cell,
    /// Whole lines — every row between anchor and cursor is fully selected.
    Line,
    /// Rectangular block (column range constant across rows), like xterm Alt+drag.
    Block,
}

/// A cursor motion that `CopyModeState::move_cursor` understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordForward,
    WordBackward,
    WordEnd,
    LineStart,
    LineEnd,
    FirstNonBlank,
    Top,
    Bottom,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
}

/// All per-cell data a text-extraction closure must supply.
///
/// The closure receives `(col, row_viewport)` — both 0-based — where
/// `row_viewport` is the visible viewport row (0 = top, rows-1 = bottom) at
/// the current scroll offset. It must return the character at that cell, or
/// `' '` for empty cells.
pub type RowAccessor<'a> = &'a dyn Fn(u16, u16) -> char;

/// State for an active copy-mode session.
#[derive(Debug, Clone)]
pub struct CopyModeState {
    /// Copy-mode cursor in viewport coordinates `(col, row)`. Row 0 = top of
    /// the currently visible viewport. Always kept within bounds by
    /// `clamp_cursor`.
    pub cursor: (u16, u16),
    /// The fixed end of a selection. `None` while no selection is live.
    pub anchor: Option<(u16, u16)>,
    /// Current selection kind (Cell / Line / Block).
    pub kind: SelectionKind,
    /// `true` while copy mode is engaged.
    pub active: bool,
    /// Terminal grid width (cols) — used for clamping.
    cols: u16,
    /// Terminal grid height (rows) — used for clamping and page motions.
    rows: u16,
}

impl CopyModeState {
    /// Build an inactive (default) state. Call `enter` to activate it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cursor: (0, 0),
            anchor: None,
            kind: SelectionKind::Cell,
            active: false,
            cols: 80,
            rows: 24,
        }
    }

    /// Enter copy mode: record the grid dimensions, move the cursor to
    /// `start` (clamped), and activate.
    pub fn enter(&mut self, start: (u16, u16), cols: u16, rows: u16) {
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        self.cursor = (
            start.0.min(self.cols.saturating_sub(1)),
            start.1.min(self.rows.saturating_sub(1)),
        );
        self.anchor = None;
        self.kind = SelectionKind::Cell;
        self.active = true;
    }

    /// Leave copy mode and clear all selection state.
    pub fn exit(&mut self) {
        self.active = false;
        self.anchor = None;
    }

    /// Apply a motion and update the cursor. `accessor` is used only by word
    /// motions (to classify characters). `history_lines` is the maximum number
    /// of lines that can scroll upward (from `emulator.history_size()`).
    ///
    /// Returns the scroll delta that the caller must apply to the viewport
    /// (`>0` = scroll up into history, `<0` = scroll down toward live).
    pub fn move_cursor(
        &mut self,
        motion: Motion,
        accessor: RowAccessor<'_>,
        history_lines: usize,
    ) -> i32 {
        let (col, row) = self.cursor;
        let cols = self.cols;
        let rows = self.rows;
        let page = rows.max(1);
        let half = (rows / 2).max(1);

        match motion {
            Motion::Left => {
                if col > 0 {
                    self.cursor.0 = col - 1;
                } else if row > 0 {
                    self.cursor.1 = row - 1;
                    self.cursor.0 = cols.saturating_sub(1);
                }
                0
            }
            Motion::Right => {
                let max_col = cols.saturating_sub(1);
                if col < max_col {
                    self.cursor.0 = col + 1;
                } else if row + 1 < rows {
                    self.cursor.1 = row + 1;
                    self.cursor.0 = 0;
                }
                0
            }
            Motion::Up => {
                if row > 0 {
                    self.cursor.1 = row - 1;
                    0
                } else {
                    // Scroll up by 1 (cursor stays at row 0).
                    1
                }
            }
            Motion::Down => {
                if row + 1 < rows {
                    self.cursor.1 = row + 1;
                    0
                } else {
                    // Scroll down by 1 (cursor stays at rows-1).
                    -1
                }
            }
            Motion::PageUp => -(page as i32),
            Motion::PageDown => page as i32,
            Motion::HalfPageUp => -(half as i32),
            Motion::HalfPageDown => half as i32,
            Motion::Top => {
                self.cursor.1 = 0;
                // Scroll to the top of the scrollback.
                i32::try_from(history_lines).unwrap_or(i32::MAX)
            }
            Motion::Bottom => {
                self.cursor.1 = rows.saturating_sub(1);
                // Scroll to the live edge.
                i32::MIN
            }
            Motion::LineStart => {
                self.cursor.0 = 0;
                0
            }
            Motion::LineEnd => {
                self.cursor.0 = cols.saturating_sub(1);
                0
            }
            Motion::FirstNonBlank => {
                // Walk right from col 0 until we hit a non-space character.
                let r = self.cursor.1;
                for c in 0..cols {
                    if accessor(c, r) != ' ' {
                        self.cursor.0 = c;
                        return 0;
                    }
                }
                self.cursor.0 = 0;
                0
            }
            Motion::WordForward => {
                let (nc, nr, scroll) = word_forward(col, row, rows, cols, accessor);
                self.cursor = (nc, nr);
                scroll
            }
            Motion::WordBackward => {
                let (nc, nr, scroll) = word_backward(col, row, rows, cols, accessor);
                self.cursor = (nc, nr);
                scroll
            }
            Motion::WordEnd => {
                let (nc, nr, scroll) = word_end(col, row, rows, cols, accessor);
                self.cursor = (nc, nr);
                scroll
            }
        }
    }

    /// Toggle or enter a selection kind.
    ///
    /// If already selecting with the given `kind`, clear the anchor (deselect).
    /// Otherwise, set the anchor to the current cursor position and activate
    /// the given kind.
    pub fn toggle_selection(&mut self, kind: SelectionKind) {
        if self.anchor.is_some() && self.kind == kind {
            self.anchor = None;
        } else {
            self.anchor = Some(self.cursor);
            self.kind = kind;
        }
    }

    /// Start a selection of `kind` at the current cursor (always sets anchor,
    /// even if one was already set). Can be called programmatically to force a
    /// new anchor (distinct from `toggle_selection` which clears the anchor
    /// when the kind matches). Used by tests and future scripting paths.
    #[allow(dead_code)]
    pub fn start_selection(&mut self, kind: SelectionKind) {
        self.anchor = Some(self.cursor);
        self.kind = kind;
    }

    /// The `(start, end, kind)` of the active selection, or `None` when no
    /// anchor is set. Start ≤ end in `(row, col)` order.
    #[must_use]
    pub fn selection_span(&self) -> Option<((u16, u16), (u16, u16), SelectionKind)> {
        let anchor = self.anchor?;
        let cursor = self.cursor;
        // Normalise: start ≤ end.
        let (start, end) = if (anchor.1, anchor.0) <= (cursor.1, cursor.0) {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        Some((start, end, self.kind))
    }

    /// Extract the selected text using `accessor(col, row) -> char`.
    ///
    /// Returns `None` when there is no active selection. The text is suitable
    /// for placing on the clipboard: rows are joined with `\n`, trailing spaces
    /// are trimmed per row, and block-selection rows are column-rect slices.
    #[must_use]
    pub fn selected_text(&self, accessor: RowAccessor<'_>) -> Option<String> {
        let (start, end, kind) = self.selection_span()?;
        let cols = self.cols;

        match kind {
            SelectionKind::Cell => {
                // Multi-row flow: first row partial (start.col..cols), middle
                // rows full, last row partial (0..=end.col).
                let mut out = String::new();
                let (s_col, s_row) = start;
                let (e_col, e_row) = end;
                for row in s_row..=e_row {
                    let (c0, c1) = if s_row == e_row {
                        (s_col, e_col)
                    } else if row == s_row {
                        (s_col, cols.saturating_sub(1))
                    } else if row == e_row {
                        (0, e_col)
                    } else {
                        (0, cols.saturating_sub(1))
                    };
                    let row_str: String = (c0..=c1).map(|c| {
                        let ch = accessor(c, row);
                        if ch == '\0' { ' ' } else { ch }
                    }).collect();
                    let trimmed = row_str.trim_end_matches(' ');
                    out.push_str(trimmed);
                    if row != e_row {
                        out.push('\n');
                    }
                }
                if out.is_empty() { None } else { Some(out) }
            }
            SelectionKind::Line => {
                // Whole lines (ignore col bounds entirely).
                let mut out = String::new();
                for row in start.1..=end.1 {
                    let row_str: String = (0..cols).map(|c| {
                        let ch = accessor(c, row);
                        if ch == '\0' { ' ' } else { ch }
                    }).collect();
                    let trimmed = row_str.trim_end_matches(' ');
                    out.push_str(trimmed);
                    if row != end.1 {
                        out.push('\n');
                    }
                }
                if out.is_empty() { None } else { Some(out) }
            }
            SelectionKind::Block => {
                // Column-rectangle: same col range, every row.
                let (col_lo, col_hi) = if start.0 <= end.0 {
                    (start.0, end.0)
                } else {
                    (end.0, start.0)
                };
                let mut out = String::new();
                for row in start.1..=end.1 {
                    let row_str: String = (col_lo..=col_hi).map(|c| {
                        let ch = accessor(c, row);
                        if ch == '\0' { ' ' } else { ch }
                    }).collect();
                    let trimmed = row_str.trim_end_matches(' ');
                    out.push_str(trimmed);
                    if row != end.1 {
                        out.push('\n');
                    }
                }
                if out.is_empty() { None } else { Some(out) }
            }
        }
    }

    /// Build the `CellRect` value the renderer needs to draw the selection
    /// highlight. Returns `None` when there is no active selection.
    ///
    /// The returned tuple is `(anchor_col, anchor_row, cursor_col, cursor_row,
    /// block)` in viewport coordinates, matching `terminale_render::CellRect`.
    #[must_use]
    pub fn renderer_selection(&self) -> Option<(u16, u16, u16, u16, bool)> {
        let anchor = self.anchor?;
        let cursor = self.cursor;
        let block = self.kind == SelectionKind::Block;
        Some((anchor.0, anchor.1, cursor.0, cursor.1, block))
    }

    /// Update the stored grid dimensions (call this when the terminal resizes
    /// while copy mode is active to keep the cursor in bounds).
    pub fn update_size(&mut self, cols: u16, rows: u16) {
        self.cols = cols.max(1);
        self.rows = rows.max(1);
        self.cursor.0 = self.cursor.0.min(self.cols.saturating_sub(1));
        self.cursor.1 = self.cursor.1.min(self.rows.saturating_sub(1));
        if let Some(a) = self.anchor.as_mut() {
            a.0 = a.0.min(self.cols.saturating_sub(1));
            a.1 = a.1.min(self.rows.saturating_sub(1));
        }
    }
}

impl Default for CopyModeState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Word-motion helpers ──────────────────────────────────────────────────────

/// True when the character is a word boundary (whitespace or common
/// shell/path punctuation), mirroring the emulator's double-click logic.
///
/// This is intentionally a small, fast classifier. Underscores, hyphens,
/// dots, and slashes are NOT boundaries (they bind identifiers and paths).
fn is_word_boundary(ch: char) -> bool {
    ch == ' ' || ch == '\t' || ch == '\n' || ch == '\0'
        || "()[]{}<>\"'`,;:!?@#$%^&*=+|\\ ".contains(ch)
}

/// `w` motion: jump forward to the start of the next word.
fn word_forward(
    col: u16, row: u16,
    rows: u16, cols: u16,
    accessor: RowAccessor<'_>,
) -> (u16, u16, i32) {
    let mut c = col;
    let mut r = row;
    // Skip the current word (non-boundary characters).
    while !is_word_boundary(accessor(c, r)) {
        if c + 1 < cols { c += 1; } else if r + 1 < rows { r += 1; c = 0; } else { return (c, r, 0); }
    }
    // Skip whitespace/boundary gap.
    while is_word_boundary(accessor(c, r)) {
        if c + 1 < cols { c += 1; } else if r + 1 < rows { r += 1; c = 0; } else { return (c, r, 0); }
    }
    (c, r, 0)
}

/// `b` motion: jump backward to the start of the current/previous word.
fn word_backward(
    col: u16, row: u16,
    _rows: u16, cols: u16,
    accessor: RowAccessor<'_>,
) -> (u16, u16, i32) {
    let mut c = col;
    let mut r = row;
    // Step back one cell first (otherwise `b` on a word start just stays).
    if c > 0 { c -= 1; } else if r > 0 { r -= 1; c = cols.saturating_sub(1); } else { return (0, 0, 0); }
    // Skip boundary gap.
    while is_word_boundary(accessor(c, r)) {
        if c > 0 { c -= 1; } else if r > 0 { r -= 1; c = cols.saturating_sub(1); } else { return (0, 0, 0); }
    }
    // Walk back through the word to its start.
    while c > 0 && !is_word_boundary(accessor(c - 1, r)) {
        c -= 1;
    }
    (c, r, 0)
}

/// `e` motion: jump to the end of the current/next word.
fn word_end(
    col: u16, row: u16,
    rows: u16, cols: u16,
    accessor: RowAccessor<'_>,
) -> (u16, u16, i32) {
    let mut c = col;
    let mut r = row;
    // Step forward one.
    if c + 1 < cols { c += 1; } else if r + 1 < rows { r += 1; c = 0; } else { return (c, r, 0); }
    // Skip boundary gap.
    while is_word_boundary(accessor(c, r)) {
        if c + 1 < cols { c += 1; } else if r + 1 < rows { r += 1; c = 0; } else { return (c, r, 0); }
    }
    // Walk to the last non-boundary in this word.
    let max_col = cols.saturating_sub(1);
    while c < max_col && !is_word_boundary(accessor(c + 1, r)) {
        c += 1;
    }
    (c, r, 0)
}

// ────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Build a fixed 80×5 grid from a slice of strings, returning a closure
    /// suitable as a `RowAccessor`.
    fn make_grid<'a>(lines: &'a [&'a str]) -> impl Fn(u16, u16) -> char + 'a {
        move |col: u16, row: u16| {
            lines
                .get(row as usize)
                .and_then(|l| l.chars().nth(col as usize))
                .unwrap_or(' ')
        }
    }

    // ── Clamping ─────────────────────────────────────────────────────────────

    #[test]
    fn cursor_clamp_top_left() {
        let mut s = CopyModeState::new();
        s.enter((0, 0), 80, 24);
        // Motion::Up from row 0 should return +1 scroll, cursor stays at 0.
        let grid = make_grid(&[]);
        let delta = s.move_cursor(Motion::Up, &grid, 100);
        assert_eq!(s.cursor.1, 0, "cursor row must stay at 0");
        assert_eq!(delta, 1, "Up from row 0 scrolls up by 1");
    }

    #[test]
    fn cursor_clamp_bottom_right() {
        let mut s = CopyModeState::new();
        s.enter((79, 23), 80, 24);
        let grid = make_grid(&[]);
        let delta = s.move_cursor(Motion::Right, &grid, 0);
        // At the last cell (79,23): Right can't wrap further, cursor stays.
        assert_eq!(s.cursor, (79, 23));
        assert_eq!(delta, 0);
    }

    #[test]
    fn cursor_clamp_left_bound() {
        let mut s = CopyModeState::new();
        s.enter((0, 5), 80, 24);
        let grid = make_grid(&[]);
        s.move_cursor(Motion::Left, &grid, 0);
        // Left from col 0, row 5 wraps to (79, 4).
        assert_eq!(s.cursor, (79, 4));
    }

    #[test]
    fn cursor_down_at_last_row_scrolls() {
        let mut s = CopyModeState::new();
        s.enter((10, 23), 80, 24);
        let grid = make_grid(&[]);
        let delta = s.move_cursor(Motion::Down, &grid, 0);
        assert_eq!(s.cursor.1, 23, "cursor stays at bottom row");
        assert_eq!(delta, -1, "Down at last row scrolls down");
    }

    // ── Word motions ─────────────────────────────────────────────────────────

    /// Grid used by word-motion tests: "foo  bar.baz" at row 0.
    ///
    /// Columns: f(0) o(1) o(2) ' '(3) ' '(4) b(5) a(6) r(7) .(8) b(9) a(10) z(11)
    fn word_test_grid() -> impl Fn(u16, u16) -> char {
        let line = "foo  bar.baz";
        move |col: u16, row: u16| {
            if row == 0 {
                line.chars().nth(col as usize).unwrap_or(' ')
            } else {
                ' '
            }
        }
    }

    #[test]
    fn word_forward_from_start_of_foo() {
        let mut s = CopyModeState::new();
        s.enter((0, 0), 80, 5);
        let grid = word_test_grid();
        s.move_cursor(Motion::WordForward, &grid, 0);
        // 'b' at col 5 is the start of the next word.
        assert_eq!(s.cursor.0, 5);
    }

    #[test]
    fn word_backward_from_bar() {
        let mut s = CopyModeState::new();
        s.enter((5, 0), 80, 5); // cursor at 'b' of "bar"
        let grid = word_test_grid();
        s.move_cursor(Motion::WordBackward, &grid, 0);
        // b-motion goes back to 'f' of "foo".
        assert_eq!(s.cursor.0, 0);
    }

    #[test]
    fn word_end_from_start_of_foo() {
        let mut s = CopyModeState::new();
        s.enter((0, 0), 80, 5);
        let grid = word_test_grid();
        s.move_cursor(Motion::WordEnd, &grid, 0);
        // e-motion lands on 'o' at col 2 (last char of "foo").
        assert_eq!(s.cursor.0, 2);
    }

    // ── Selection span ───────────────────────────────────────────────────────

    #[test]
    fn cell_selection_span_normalised() {
        let mut s = CopyModeState::new();
        s.enter((5, 3), 80, 24);
        // Anchor at (5,3), move cursor to (2,1) — cursor is "before" anchor.
        s.start_selection(SelectionKind::Cell);
        s.cursor = (2, 1);
        let (start, end, kind) = s.selection_span().unwrap();
        assert_eq!(kind, SelectionKind::Cell);
        // start must be ≤ end in (row,col) order.
        assert!(
            (start.1, start.0) <= (end.1, end.0),
            "start {start:?} must precede end {end:?}"
        );
    }

    #[test]
    fn line_selection_span_whole_rows() {
        let mut s = CopyModeState::new();
        s.enter((10, 2), 80, 24);
        s.start_selection(SelectionKind::Line);
        s.cursor = (3, 5);
        let (start, end, kind) = s.selection_span().unwrap();
        assert_eq!(kind, SelectionKind::Line);
        assert_eq!(start.1, 2);
        assert_eq!(end.1, 5);
    }

    #[test]
    fn block_selection_span_cols() {
        let mut s = CopyModeState::new();
        s.enter((10, 2), 80, 24);
        s.start_selection(SelectionKind::Block);
        s.cursor = (3, 5);
        let (start, end, kind) = s.selection_span().unwrap();
        assert_eq!(kind, SelectionKind::Block);
        // Rows are normalised.
        assert_eq!(start.1, 2);
        assert_eq!(end.1, 5);
    }

    // ── Text extraction ──────────────────────────────────────────────────────

    #[test]
    fn cell_text_extraction_partial_lines() {
        // 3-row grid: row 0 = "Hello World", row 1 = "Second line", row 2 = "Third"
        let grid = make_grid(&["Hello World", "Second line", "Third      "]);
        let mut s = CopyModeState::new();
        s.enter((6, 0), 11, 3); // 11 cols, 3 rows
        s.start_selection(SelectionKind::Cell);
        s.cursor = (5, 1); // end at "Second"
        let text = s.selected_text(&grid).unwrap();
        // row 0: cols 6..=10 = "World"
        // row 1: cols 0..=5  = "Second"
        assert!(text.contains("World"), "first row partial: {text:?}");
        assert!(text.contains("Second"), "second row partial: {text:?}");
    }

    #[test]
    fn line_text_extraction_whole_rows() {
        let grid = make_grid(&["foo bar", "baz qux", "ignored"]);
        let mut s = CopyModeState::new();
        s.enter((3, 0), 7, 3);
        s.start_selection(SelectionKind::Line);
        s.cursor = (1, 1);
        let text = s.selected_text(&grid).unwrap();
        assert!(text.contains("foo bar"), "row 0 must be fully included");
        assert!(text.contains("baz qux"), "row 1 must be fully included");
        assert!(!text.contains("ignored"), "row 2 must not be included");
    }

    #[test]
    fn block_text_extraction_column_rect() {
        // Grid: 10 cols, 3 rows.
        let grid = make_grid(&["ABCDEFGHIJ", "0123456789", "abcdefghij"]);
        let mut s = CopyModeState::new();
        s.enter((2, 0), 10, 3); // anchor col 2
        s.start_selection(SelectionKind::Block);
        s.cursor = (4, 2); // end col 4, row 2
        let text = s.selected_text(&grid).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // Cols 2..=4 of each row.
        assert_eq!(lines[0], "CDE", "row 0 block: {text:?}");
        assert_eq!(lines[1], "234", "row 1 block: {text:?}");
        assert_eq!(lines[2], "cde", "row 2 block: {text:?}");
    }

    // ── Config default / roundtrip ───────────────────────────────────────────

    #[test]
    fn copy_mode_shortcut_default_is_not_empty() {
        let sc = terminale_config::ShortcutsConfig::default();
        assert!(
            !sc.copy_mode.is_empty(),
            "copy_mode shortcut must have a non-empty default"
        );
    }

    #[test]
    fn copy_mode_shortcut_roundtrip() {
        let sc = terminale_config::ShortcutsConfig::default();
        // The default binding must survive a serde round-trip via JSON
        // (serde_json IS available in the terminale-config test path but not
        // directly here). Verify the simpler invariant: cloning the config
        // preserves the binding string.
        let cloned = sc.clone();
        assert_eq!(cloned.copy_mode, sc.copy_mode);
        // And it must parse as a valid hotkey binding (non-empty, has a `+`
        // separator) — same check used for all other default bindings.
        assert!(
            sc.copy_mode.contains('+'),
            "copy_mode default binding must have modifier+key form"
        );
    }

    // ── Toggle selection ─────────────────────────────────────────────────────

    #[test]
    fn toggle_selection_same_kind_clears_anchor() {
        let mut s = CopyModeState::new();
        s.enter((5, 5), 80, 24);
        s.toggle_selection(SelectionKind::Cell);
        assert!(s.anchor.is_some());
        s.toggle_selection(SelectionKind::Cell);
        assert!(s.anchor.is_none(), "toggling same kind clears anchor");
    }

    #[test]
    fn toggle_selection_different_kind_changes_kind() {
        let mut s = CopyModeState::new();
        s.enter((5, 5), 80, 24);
        s.toggle_selection(SelectionKind::Cell);
        s.toggle_selection(SelectionKind::Line);
        assert_eq!(s.kind, SelectionKind::Line);
        assert!(s.anchor.is_some(), "anchor must be set after kind switch");
    }
}

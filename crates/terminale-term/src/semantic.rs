//! OSC 133 semantic prompt zones (shell integration).
//!
//! Tracks prompt boundaries emitted by the shell via OSC 133 (FinalTerm)
//! sequences and provides navigation helpers so the host can jump between
//! prompts in the scrollback.
//!
//! ## Sequence summary
//!
//! | OSC payload   | Meaning                                       |
//! |---------------|-----------------------------------------------|
//! | `133;A`       | Prompt start                                  |
//! | `133;B`       | Prompt end / user-input start                 |
//! | `133;C`       | Command output start (user pressed Enter)     |
//! | `133;D`       | Command end — shell ready for next command    |
//! | `133;D;N`     | Same as D, exit-code N                        |
//!
//! The host calls [`SemanticModel::record`] with the current **absolute line**
//! whenever one of these sequences arrives. A complete A→D cycle produces one
//! [`PromptMark`]; partial or interleaved sequences are tolerated gracefully
//! (the state machine resets on each fresh `A`).
//!
//! A full A→B→C→D cycle additionally produces one [`CommandBlock`] which
//! captures the typed command text and the output span. Blocks are stored in a
//! bounded list (controlled by [`SemanticModel::set_max_blocks`]) and pruned
//! alongside the scrollback.

#![warn(missing_docs)]

/// The semantic kind of an OSC 133 zone boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscKind {
    /// `\e]133;A\e\\` — prompt drawing has started.
    PromptStart,
    /// `\e]133;B\e\\` — prompt text ended, user input begins.
    InputStart,
    /// `\e]133;C\e\\` — user confirmed (Enter); command output begins.
    OutputStart,
    /// `\e]133;D[;exitcode]\e\\` — command finished, shell is ready again.
    CommandEnd,
}

/// A fully-resolved prompt mark: a prompt that started at `line` and whose
/// command, if any, exited with `exit_code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptMark {
    /// Absolute line index at which the prompt started (`OSC 133;A`).
    ///
    /// Absolute means: `line < 0` is in the scrollback (history), `line >= 0`
    /// is on the visible screen. Same coordinate space as alacritty's
    /// `alacritty_terminal::index::Line`.
    pub line: i32,
    /// Exit code reported by `OSC 133;D;N`, or `None` when no exit status
    /// was sent (e.g. the sequence is incomplete or the shell never emits D).
    pub exit_code: Option<u32>,
}

/// A captured command block assembled from a complete A→B→C→D OSC 133 cycle.
///
/// All line indices use the same absolute coordinate space as [`PromptMark`]:
/// negative values are in the scrollback, non-negative values are on the
/// visible screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBlock {
    /// Absolute line index where the prompt started (`OSC 133;A`).
    pub prompt_line: i32,
    /// Absolute line index where user input started (`OSC 133;B`).
    pub command_start_line: i32,
    /// Absolute line index where command output started (`OSC 133;C`).
    pub output_start_line: i32,
    /// Absolute line index where the command finished (`OSC 133;D`).
    /// `None` while the command is still running (C seen but D not yet).
    pub end_line: Option<i32>,
    /// The text the user typed between B and C, trimmed of leading/trailing
    /// whitespace. Supplied by the caller at C-time via
    /// [`SemanticModel::record_with_text`].
    pub command_text: String,
    /// Current-working-directory captured at C-time from the shell's last
    /// OSC 7 announcement. `None` when the shell hasn't emitted OSC 7 yet.
    pub cwd: Option<String>,
    /// Exit code from `OSC 133;D;N`. `None` when D carried no exit code, or
    /// when the block has not yet been finalised (still running).
    pub exit_code: Option<i32>,
}

/// In-flight builder while we've seen A but not yet D.
#[derive(Debug, Clone)]
struct Pending {
    prompt_line: i32,
    /// Set when we receive B.
    command_start_line: Option<i32>,
    /// Set when we receive C (block becomes "assembled but open").
    output_start_line: Option<i32>,
}

/// OSC 133 semantic-prompt state machine.
///
/// Feed events with [`SemanticModel::record`] (or
/// [`SemanticModel::record_with_text`] at C-time to capture the command text).
/// Query with [`SemanticModel::prev_prompt`], [`SemanticModel::next_prompt`],
/// and [`SemanticModel::iter_marks`]. Prune old marks with
/// [`SemanticModel::prune`].
///
/// Command blocks are additionally available via [`SemanticModel::blocks`],
/// [`SemanticModel::last_block`], and [`SemanticModel::block_at_line`].
#[derive(Debug, Default)]
pub struct SemanticModel {
    /// Completed prompt marks, oldest first.
    marks: Vec<PromptMark>,
    /// Partial state: set when we've received A but not yet D.
    pending: Option<Pending>,
    /// Completed (and in-progress) command blocks, oldest first.
    blocks: Vec<CommandBlock>,
    /// Maximum number of blocks to retain. Oldest blocks are dropped once this
    /// cap is exceeded. `0` disables the block list entirely (no blocks stored).
    max_blocks: usize,
}

impl SemanticModel {
    /// Create an empty model.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of [`CommandBlock`]s to retain in memory.
    ///
    /// When the list grows past this limit, the oldest blocks are dropped.
    /// `0` disables block capture entirely (no `CommandBlock` is ever stored).
    /// The default after [`Self::new`] is `0` — callers must opt in by calling
    /// this method with the configured cap (e.g. the value from
    /// `config.terminal.max_command_blocks`).
    pub fn set_max_blocks(&mut self, max: usize) {
        self.max_blocks = max;
    }

    /// Feed one OSC 133 event at the given absolute line position.
    ///
    /// `exit_code` is only meaningful when `kind == OscKind::CommandEnd`.
    /// The model tolerates missing or re-entrant A markers — a fresh A always
    /// restarts the pending state so a missed D doesn't wedge things.
    ///
    /// When called for [`OscKind::OutputStart`] (C), the `command_text` and
    /// `cwd` are not available through this call — use
    /// [`Self::record_with_text`] instead if you want to capture them.
    pub fn record(&mut self, kind: OscKind, line: i32, exit_code: Option<u32>) {
        self.record_with_text(kind, line, exit_code, String::new(), None);
    }

    /// Like [`Self::record`] but allows the caller to supply the command text
    /// (extracted from the B→C cell span) and the current working directory
    /// (from the latest OSC 7 announcement) when `kind ==
    /// OscKind::OutputStart`.
    ///
    /// For all other kinds `command_text` and `cwd` are ignored.
    pub fn record_with_text(
        &mut self,
        kind: OscKind,
        line: i32,
        exit_code: Option<u32>,
        command_text: String,
        cwd: Option<String>,
    ) {
        match kind {
            OscKind::PromptStart => {
                // Start a fresh pending state (reset any incomplete prior one).
                self.pending = Some(Pending {
                    prompt_line: line,
                    command_start_line: None,
                    output_start_line: None,
                });
            }
            OscKind::InputStart => {
                // Record B line in the pending state.
                if let Some(p) = self.pending.as_mut() {
                    p.command_start_line = Some(line);
                }
            }
            OscKind::OutputStart => {
                // C — command was submitted. If we have enough pending state,
                // assemble an open (end_line == None) CommandBlock.
                if let Some(p) = self.pending.as_mut() {
                    p.output_start_line = Some(line);
                    // Only materialise a block when block capture is enabled.
                    if self.max_blocks > 0 {
                        let block = CommandBlock {
                            prompt_line: p.prompt_line,
                            command_start_line: p.command_start_line.unwrap_or(p.prompt_line),
                            output_start_line: line,
                            end_line: None,
                            command_text: command_text.trim().to_string(),
                            cwd,
                            exit_code: None,
                        };
                        self.blocks.push(block);
                        // Enforce the cap: drop the oldest blocks.
                        if self.blocks.len() > self.max_blocks {
                            let drop = self.blocks.len() - self.max_blocks;
                            self.blocks.drain(..drop);
                        }
                    }
                }
            }
            OscKind::CommandEnd => {
                // D — finalise the most-recent open block (if any).
                if self.max_blocks > 0 {
                    if let Some(block) = self.blocks.iter_mut().rev().find(|b| b.end_line.is_none()) {
                        block.end_line = Some(line);
                        block.exit_code = exit_code.map(|c| c as i32);
                    }
                }

                if let Some(p) = self.pending.take() {
                    self.marks.push(PromptMark {
                        line: p.prompt_line,
                        exit_code,
                    });
                } else {
                    // If there was no pending A (e.g. we connected mid-session),
                    // record a mark at the current line with whatever exit code we
                    // have — better than dropping the event entirely.
                    self.marks.push(PromptMark {
                        line,
                        exit_code,
                    });
                }
            }
        }
    }

    /// Return the mark immediately **before** the given absolute line, or the
    /// last mark when `from_line` is past all of them.
    ///
    /// "Previous" = the nearest mark with `mark.line < from_line`, scanning
    /// backwards. Returns `None` when there are no marks at all.
    #[must_use]
    pub fn prev_prompt(&self, from_line: i32) -> Option<&PromptMark> {
        // Marks are appended in arrival order. We walk from the end to find
        // the newest mark that still sits strictly above (earlier than)
        // from_line.
        self.marks.iter().rev().find(|m| m.line < from_line)
    }

    /// Return the mark immediately **after** the given absolute line.
    ///
    /// "Next" = the nearest mark with `mark.line > from_line`, scanning
    /// forwards. Returns `None` when there are no marks past `from_line`.
    #[must_use]
    pub fn next_prompt(&self, from_line: i32) -> Option<&PromptMark> {
        self.marks.iter().find(|m| m.line > from_line)
    }

    /// Iterate all completed marks in insertion order (oldest first).
    pub fn iter_marks(&self) -> impl Iterator<Item = &PromptMark> {
        self.marks.iter()
    }

    /// Drop marks whose absolute line is older than `topmost_line` (the
    /// oldest line still retained in the scrollback). Keeps memory bounded
    /// to the scrollback capacity.
    ///
    /// Also prunes [`CommandBlock`]s whose entire range (prompt line through
    /// end line) has scrolled off the top of the buffer. An open block (whose
    /// `end_line` is `None`) is kept even if its prompt line is above the
    /// topmost line, because it may still be receiving output.
    pub fn prune(&mut self, topmost_line: i32) {
        self.marks.retain(|m| m.line >= topmost_line);
        self.blocks.retain(|b| {
            // Keep the block if its output region extends to at least the
            // topmost visible line, or if it is still open (running).
            let last_line = b.end_line.unwrap_or(i32::MAX);
            last_line >= topmost_line
        });
    }

    /// Slice of all completed (and in-progress) command blocks, oldest first.
    ///
    /// Requires `max_blocks > 0` (set via [`Self::set_max_blocks`]); otherwise
    /// this always returns an empty slice.
    #[must_use]
    pub fn blocks(&self) -> &[CommandBlock] {
        &self.blocks
    }

    /// The most-recently assembled block, or `None` when no blocks have been
    /// recorded yet.
    #[must_use]
    pub fn last_block(&self) -> Option<&CommandBlock> {
        self.blocks.last()
    }

    /// Return the block whose output range contains `abs_line`, or `None`.
    ///
    /// "Contains" means `output_start_line <= abs_line <= end_line` (or, for
    /// an open block, `output_start_line <= abs_line`).
    #[must_use]
    pub fn block_at_line(&self, abs_line: i32) -> Option<&CommandBlock> {
        self.blocks.iter().rev().find(|b| {
            if abs_line < b.output_start_line {
                return false;
            }
            match b.end_line {
                Some(end) => abs_line <= end,
                None => true, // open block — still writing output
            }
        })
    }

    /// Discard all currently-stored command blocks. Called when the feature is
    /// disabled at runtime so stale data is not retained in memory.
    pub fn clear_blocks(&mut self) {
        self.blocks.clear();
    }

    /// The absolute line of the `B` (InputStart) event in the current pending
    /// cycle, or `None` when no cycle is in progress or B has not arrived.
    ///
    /// Used internally by `sniff_osc_133` to pass the B line to the command-text
    /// extractor at C-time.
    #[must_use]
    pub fn pending_command_start_line(&self) -> Option<i32> {
        self.pending.as_ref().and_then(|p| p.command_start_line)
    }

    /// Number of completed marks currently tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    /// `true` when no marks have been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Returns `true` when a command is currently running according to the
    /// OSC 133 state machine.
    ///
    /// A pane is considered "running" if either:
    ///
    /// * The `pending` state has seen a `C` mark (`output_start_line.is_some()`)
    ///   but no `D` yet — i.e. the user pressed Enter and the shell has not
    ///   reported completion.
    /// * The block list contains at least one open (unfinalised) block whose
    ///   `end_line` is `None` — meaning a `C` was processed, a block was
    ///   materialised, but `D` has not arrived.
    ///
    /// Returns `false` when no OSC 133 sequence has been received at all (the
    /// shell does not support shell integration), to avoid a false-positive
    /// "busy" that would make the spinner spin forever.  The fallback
    /// output-activity heuristic on [`crate::Pane`] handles those shells.
    #[must_use]
    pub fn is_command_running(&self) -> bool {
        // Pending with a C mark seen — command submitted, D not yet received.
        if let Some(p) = &self.pending {
            if p.output_start_line.is_some() {
                return true;
            }
        }
        // Any open block in the list (max_blocks > 0 path).
        self.blocks.iter().any(|b| b.end_line.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a complete A→D cycle at the given (prompt_line, d_line, exit).
    // Does NOT enable block capture — for plain PromptMark tests.
    fn full_cycle(model: &mut SemanticModel, prompt_line: i32, d_line: i32, exit: Option<u32>) {
        model.record(OscKind::PromptStart, prompt_line, None);
        model.record(OscKind::InputStart, prompt_line, None);
        model.record(OscKind::OutputStart, prompt_line, None);
        model.record(OscKind::CommandEnd, d_line, exit);
    }

    // Helper: build a complete A→B→C→D cycle with block capture enabled, at
    // realistic distinct lines.
    fn full_block_cycle(
        model: &mut SemanticModel,
        prompt_line: i32,
        b_line: i32,
        c_line: i32,
        d_line: i32,
        cmd: &str,
        cwd: Option<&str>,
        exit: Option<u32>,
    ) {
        model.record(OscKind::PromptStart, prompt_line, None);
        model.record(OscKind::InputStart, b_line, None);
        model.record_with_text(
            OscKind::OutputStart,
            c_line,
            None,
            cmd.to_string(),
            cwd.map(String::from),
        );
        model.record(OscKind::CommandEnd, d_line, exit);
    }

    #[test]
    fn complete_cycle_produces_one_mark() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 10, 11, Some(0));
        assert_eq!(m.len(), 1);
        let mark = m.iter_marks().next().unwrap();
        assert_eq!(mark.line, 10, "mark should be at prompt_start line");
        assert_eq!(mark.exit_code, Some(0));
    }

    #[test]
    fn exit_code_nonzero_preserved() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 5, 6, Some(127));
        let mark = m.iter_marks().next().unwrap();
        assert_eq!(mark.exit_code, Some(127));
    }

    #[test]
    fn no_exit_code_is_none() {
        let mut m = SemanticModel::new();
        m.record(OscKind::PromptStart, 0, None);
        m.record(OscKind::CommandEnd, 1, None);
        let mark = m.iter_marks().next().unwrap();
        assert_eq!(mark.exit_code, None);
    }

    #[test]
    fn multiple_cycles_accumulate() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 0, 1, Some(0));
        full_cycle(&mut m, 5, 6, Some(1));
        full_cycle(&mut m, 10, 11, Some(0));
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn d_without_a_still_records() {
        // Mid-session connect: shell emits D with no prior A.
        let mut m = SemanticModel::new();
        m.record(OscKind::CommandEnd, 20, Some(0));
        assert_eq!(m.len(), 1);
        assert_eq!(m.iter_marks().next().unwrap().line, 20);
    }

    #[test]
    fn fresh_a_resets_pending() {
        // Two A in a row — second A wins.
        let mut m = SemanticModel::new();
        m.record(OscKind::PromptStart, 5, None);
        m.record(OscKind::PromptStart, 8, None); // resets
        m.record(OscKind::CommandEnd, 9, Some(0));
        assert_eq!(m.len(), 1);
        assert_eq!(m.iter_marks().next().unwrap().line, 8);
    }

    #[test]
    fn prev_prompt_returns_nearest_above() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 10, 11, Some(0)); // mark A
        full_cycle(&mut m, 20, 21, Some(0)); // mark B
        full_cycle(&mut m, 30, 31, Some(0)); // mark C

        // Standing at line 25 → prev is mark B (line 20).
        let prev = m.prev_prompt(25).unwrap();
        assert_eq!(prev.line, 20);

        // Standing exactly on mark B's line → prev is A (line 10).
        let prev = m.prev_prompt(20).unwrap();
        assert_eq!(prev.line, 10);
    }

    #[test]
    fn next_prompt_returns_nearest_below() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 10, 11, Some(0)); // mark A
        full_cycle(&mut m, 20, 21, Some(0)); // mark B
        full_cycle(&mut m, 30, 31, Some(0)); // mark C

        // Standing at line 15 → next is mark B (line 20).
        let next = m.next_prompt(15).unwrap();
        assert_eq!(next.line, 20);

        // Standing exactly on mark B → next is C (line 30).
        let next = m.next_prompt(20).unwrap();
        assert_eq!(next.line, 30);
    }

    #[test]
    fn prev_none_when_no_marks_before() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 10, 11, Some(0));
        // from_line = 10 — mark is at 10, not strictly below.
        assert!(m.prev_prompt(10).is_none());
        // from_line = 5 — mark is above 5, not below.
        assert!(m.prev_prompt(5).is_none());
    }

    #[test]
    fn next_none_when_no_marks_after() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 10, 11, Some(0));
        assert!(m.next_prompt(10).is_none());
        assert!(m.next_prompt(15).is_none());
    }

    #[test]
    fn prune_removes_old_marks() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, -50, -49, Some(0)); // will be pruned
        full_cycle(&mut m, -10, -9, Some(0));  // survives
        full_cycle(&mut m, 0, 1, Some(0));     // survives

        // topmost_line = -20: drop anything older (line < -20).
        m.prune(-20);
        assert_eq!(m.len(), 2);
        let lines: Vec<i32> = m.iter_marks().map(|mk| mk.line).collect();
        assert_eq!(lines, [-10, 0]);
    }

    #[test]
    fn empty_model_returns_none() {
        let m = SemanticModel::new();
        assert!(m.prev_prompt(0).is_none());
        assert!(m.next_prompt(0).is_none());
        assert!(m.is_empty());
    }

    // ── CommandBlock tests ────────────────────────────────────────────────────

    #[test]
    fn blocks_disabled_by_default() {
        // max_blocks defaults to 0 — no blocks ever stored.
        let mut m = SemanticModel::new();
        full_block_cycle(&mut m, 0, 1, 2, 5, "ls -la", Some("/home/user"), Some(0));
        assert!(m.blocks().is_empty(), "blocks should be empty when max_blocks == 0");
    }

    #[test]
    fn single_block_captures_command_and_span() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        full_block_cycle(&mut m, 0, 1, 2, 5, "ls -la", Some("/home/user"), Some(0));

        assert_eq!(m.blocks().len(), 1);
        let b = &m.blocks()[0];
        assert_eq!(b.prompt_line, 0);
        assert_eq!(b.command_start_line, 1);
        assert_eq!(b.output_start_line, 2);
        assert_eq!(b.end_line, Some(5));
        assert_eq!(b.command_text, "ls -la");
        assert_eq!(b.cwd.as_deref(), Some("/home/user"));
        assert_eq!(b.exit_code, Some(0));
    }

    #[test]
    fn failing_command_has_nonzero_exit_code() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        full_block_cycle(&mut m, 0, 1, 2, 5, "cat /nonexistent", None, Some(1));

        let b = m.last_block().expect("must have one block");
        assert_eq!(b.exit_code, Some(1));
    }

    #[test]
    fn two_commands_produce_two_blocks() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        full_block_cycle(&mut m, 0, 1, 2, 5, "echo hello", Some("/tmp"), Some(0));
        full_block_cycle(&mut m, 6, 7, 8, 12, "make test", Some("/src"), Some(2));

        assert_eq!(m.blocks().len(), 2);
        assert_eq!(m.blocks()[0].command_text, "echo hello");
        assert_eq!(m.blocks()[1].command_text, "make test");
        assert_eq!(m.blocks()[1].exit_code, Some(2));
    }

    #[test]
    fn open_block_has_none_end_line() {
        // Block is assembled at C but D hasn't arrived yet.
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        m.record(OscKind::PromptStart, 0, None);
        m.record(OscKind::InputStart, 1, None);
        m.record_with_text(
            OscKind::OutputStart,
            2,
            None,
            "sleep 60".to_string(),
            Some("/home".to_string()),
        );

        let b = m.last_block().expect("block must exist after C");
        assert_eq!(b.end_line, None, "block must be open (end_line == None)");
        assert_eq!(b.command_text, "sleep 60");
        assert_eq!(b.exit_code, None);
    }

    #[test]
    fn block_at_line_finds_correct_block() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        full_block_cycle(&mut m, 0, 1, 2, 5, "echo a", None, Some(0));
        full_block_cycle(&mut m, 10, 11, 12, 20, "echo b", None, Some(0));

        // Lines 2–5 belong to block 0.
        let b = m.block_at_line(3).expect("line 3 is in block 0");
        assert_eq!(b.command_text, "echo a");

        // Lines 12–20 belong to block 1.
        let b = m.block_at_line(15).expect("line 15 is in block 1");
        assert_eq!(b.command_text, "echo b");

        // Line between the two blocks matches nothing.
        assert!(m.block_at_line(7).is_none(), "gap line must not match any block");
    }

    #[test]
    fn block_at_line_returns_open_block_past_start() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        m.record(OscKind::PromptStart, 0, None);
        m.record(OscKind::InputStart, 1, None);
        m.record_with_text(OscKind::OutputStart, 5, None, "tail -f log".to_string(), None);

        // Any line >= output_start should match the open block.
        assert!(m.block_at_line(5).is_some());
        assert!(m.block_at_line(100).is_some());
        // Before output start — no match.
        assert!(m.block_at_line(4).is_none());
    }

    #[test]
    fn max_blocks_cap_evicts_oldest() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(3);

        for i in 0..5i32 {
            let base = i * 10;
            full_block_cycle(
                &mut m, base, base + 1, base + 2, base + 5,
                &format!("cmd{i}"), None, Some(0),
            );
        }

        assert_eq!(m.blocks().len(), 3, "only 3 newest blocks should be retained");
        // The three retained blocks are cmd2, cmd3, cmd4.
        assert_eq!(m.blocks()[0].command_text, "cmd2");
        assert_eq!(m.blocks()[2].command_text, "cmd4");
    }

    #[test]
    fn prune_drops_blocks_scrolled_off() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        full_block_cycle(&mut m, -50, -49, -48, -40, "old cmd", None, Some(0));
        full_block_cycle(&mut m, -10, -9, -8, -5, "recent cmd", None, Some(0));
        full_block_cycle(&mut m, 0, 1, 2, 5, "live cmd", None, Some(0));

        // topmost_line = -20: the old block (end_line = -40) scrolls off.
        m.prune(-20);
        assert_eq!(m.blocks().len(), 2, "only blocks with end_line >= -20 survive");
        assert_eq!(m.blocks()[0].command_text, "recent cmd");
    }

    #[test]
    fn prune_keeps_open_block_even_if_prompt_scrolled() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        // Open block: prompt at -100, output_start at -50, still running.
        m.record(OscKind::PromptStart, -100, None);
        m.record(OscKind::InputStart, -99, None);
        m.record_with_text(OscKind::OutputStart, -50, None, "big_build".to_string(), None);

        // Prune past the prompt_line but keep it because end_line is None.
        m.prune(-10);
        assert_eq!(m.blocks().len(), 1, "open block must survive prune");
    }

    // ── is_command_running ────────────────────────────────────────────────────

    /// A: waiting at the prompt (no pending at all) — not running.
    #[test]
    fn is_command_running_false_before_any_sequence() {
        let m = SemanticModel::new();
        assert!(!m.is_command_running(), "idle model must not report running");
    }

    /// B → C received (command submitted, D pending) — running.
    #[test]
    fn is_command_running_true_after_c_before_d() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        m.record(OscKind::PromptStart, 0, None);
        m.record(OscKind::InputStart, 1, None);
        m.record(OscKind::OutputStart, 2, None);
        assert!(
            m.is_command_running(),
            "must report running between C and D"
        );
    }

    /// A → B → C → D complete cycle — not running after D.
    #[test]
    fn is_command_running_false_after_d() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        full_block_cycle(&mut m, 0, 1, 2, 5, "ls", None, Some(0));
        assert!(
            !m.is_command_running(),
            "must not report running after D finalises the block"
        );
    }

    /// max_blocks=0 path: pending.output_start_line approach still works.
    #[test]
    fn is_command_running_true_after_c_no_blocks() {
        // Blocks disabled; only the pending-state check can fire.
        let mut m = SemanticModel::new(); // max_blocks == 0
        m.record(OscKind::PromptStart, 0, None);
        m.record(OscKind::InputStart, 1, None);
        m.record(OscKind::OutputStart, 2, None);
        assert!(
            m.is_command_running(),
            "pending C must be detected even when block capture is disabled"
        );
    }

    /// After D with no prior A (fallback mark path) — not running.
    #[test]
    fn is_command_running_false_for_orphan_d() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        m.record(OscKind::CommandEnd, 10, Some(0));
        assert!(!m.is_command_running(), "orphan D must not leave running state");
    }

    /// A seen but only B (no C) — not running yet (command not submitted).
    #[test]
    fn is_command_running_false_after_b_only() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        m.record(OscKind::PromptStart, 0, None);
        m.record(OscKind::InputStart, 1, None);
        assert!(
            !m.is_command_running(),
            "B alone (no C) must not report running"
        );
    }

    #[test]
    fn d_without_a_does_not_create_orphan_block() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        // D without preceding A → only a PromptMark, no block.
        m.record(OscKind::CommandEnd, 20, Some(0));
        assert!(m.blocks().is_empty(), "no block should be created without A");
        assert_eq!(m.len(), 1, "a fallback PromptMark should still be recorded");
    }

    #[test]
    fn no_exit_code_block_exit_is_none() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        full_block_cycle(&mut m, 0, 1, 2, 5, "git status", None, None);
        let b = m.last_block().unwrap();
        assert_eq!(b.exit_code, None, "absent exit code must be None");
    }

    // ── Prompt-navigation index math ──────────────────────────────────────────

    /// Helper: produce a list of failed blocks suitable for testing navigation.
    fn seed_mixed_blocks(m: &mut SemanticModel) {
        // Block layout (prompt_line, exit_code):
        //   0:  "echo a"   exit 0   (success)
        //   10: "make"     exit 2   (failed)
        //   20: "ls"       exit 0   (success)
        //   30: "cargo"    exit 1   (failed)
        //   40: "echo b"   exit 0   (success)
        m.set_max_blocks(100);
        full_block_cycle(m, 0,  1,  2,  3,  "echo a",  None, Some(0));
        full_block_cycle(m, 10, 11, 12, 13, "make",     None, Some(2));
        full_block_cycle(m, 20, 21, 22, 23, "ls",       None, Some(0));
        full_block_cycle(m, 30, 31, 32, 33, "cargo",    None, Some(1));
        full_block_cycle(m, 40, 41, 42, 43, "echo b",   None, Some(0));
    }

    #[test]
    fn prev_prompt_clamped_at_first_mark() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 10, 11, Some(0));
        full_cycle(&mut m, 20, 21, Some(0));
        // from_line == first mark: no mark strictly before → None (clamp, no wrap).
        assert!(m.prev_prompt(10).is_none(), "must clamp, not wrap");
    }

    #[test]
    fn next_prompt_clamped_at_last_mark() {
        let mut m = SemanticModel::new();
        full_cycle(&mut m, 10, 11, Some(0));
        full_cycle(&mut m, 20, 21, Some(0));
        // from_line == last mark (or beyond): no mark strictly after → None.
        assert!(m.next_prompt(20).is_none(), "must clamp, not wrap");
        assert!(m.next_prompt(25).is_none(), "beyond all marks must clamp");
    }

    #[test]
    fn failed_filter_selects_only_nonzero_exit_blocks() {
        let mut m = SemanticModel::new();
        seed_mixed_blocks(&mut m);

        let failed: Vec<i32> = m
            .blocks()
            .iter()
            .filter(|b| b.exit_code.is_some_and(|c| c != 0))
            .map(|b| b.prompt_line)
            .collect();

        assert_eq!(failed, vec![10, 30], "only blocks with non-zero exit should be selected");
    }

    #[test]
    fn failed_prev_from_middle_of_viewport() {
        let mut m = SemanticModel::new();
        seed_mixed_blocks(&mut m);
        // Viewport top at abs_line 25; looking backward for a failed block
        // strictly above 25. The nearest is prompt_line 10 (not 30 which is below).
        let top_abs = 25_i32;
        let found = m
            .blocks()
            .iter()
            .rev()
            .filter(|b| b.exit_code.is_some_and(|c| c != 0) && b.prompt_line < top_abs)
            .map(|b| b.prompt_line)
            .next();
        assert_eq!(found, Some(10), "prev failed from 25 must be the block at line 10");
    }

    #[test]
    fn failed_next_from_middle_of_viewport() {
        let mut m = SemanticModel::new();
        seed_mixed_blocks(&mut m);
        // Viewport bottom at abs_line 25; looking forward for a failed block
        // strictly below 25. The nearest is prompt_line 30.
        let bottom_abs = 25_i32;
        let found = m
            .blocks()
            .iter()
            .filter(|b| b.exit_code.is_some_and(|c| c != 0) && b.prompt_line > bottom_abs)
            .map(|b| b.prompt_line)
            .next();
        assert_eq!(found, Some(30), "next failed from bottom=25 must be the block at line 30");
    }

    #[test]
    fn failed_prev_clamps_when_no_earlier_failed() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        // Only one failed block at line 10; viewport top already above it.
        full_block_cycle(&mut m, 10, 11, 12, 15, "fail", None, Some(1));
        full_block_cycle(&mut m, 20, 21, 22, 25, "ok",   None, Some(0));
        let top_abs = 5_i32; // above all blocks
        let found = m
            .blocks()
            .iter()
            .rev()
            .filter(|b| b.exit_code.is_some_and(|c| c != 0) && b.prompt_line < top_abs)
            .map(|b| b.prompt_line)
            .next();
        // No failed block above top_abs → None (clamp).
        assert!(found.is_none(), "must clamp at oldest failed block");
    }

    #[test]
    fn failed_next_clamps_when_no_later_failed() {
        let mut m = SemanticModel::new();
        m.set_max_blocks(100);
        full_block_cycle(&mut m, 10, 11, 12, 15, "ok",   None, Some(0));
        full_block_cycle(&mut m, 20, 21, 22, 25, "fail", None, Some(1));
        let bottom_abs = 25_i32; // at or past all blocks
        let found = m
            .blocks()
            .iter()
            .filter(|b| b.exit_code.is_some_and(|c| c != 0) && b.prompt_line > bottom_abs)
            .map(|b| b.prompt_line)
            .next();
        // No failed block below bottom_abs → None (clamp).
        assert!(found.is_none(), "must clamp at newest failed block");
    }
}

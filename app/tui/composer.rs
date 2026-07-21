//! The input composer: a multi-line text field that PRODUCES a submit intent.
//!
//! Design note (why this is its own type): the cockpit's old input was a flat
//! `String` fused with parsing and a blocking call. Following the input study,
//! the composer owns editing ONLY. It never executes anything; it returns a
//! `Submit` intent the app loop dispatches. That seam is what lets dispatch
//! become non-blocking without touching editing.
//!
//! Dependencies are deliberately just ratatui + crossterm (no `tui-textarea`):
//! the editing surface we need (single cursor, multi-line, history, paste) is
//! small enough that a self-contained implementation is clearer than pulling a
//! crate whose undo/selection/wrapping machinery we would not use.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// One submitted message. Plain text OR a `/command` string; the app decides
/// which. A single-variant enum (not a bare `String`) so the intent stays
/// distinct from "no submission" and can grow variants without breaking callers.
pub enum Submit {
    Line(String),
}

/// The largest number of visual rows the composer will ask for. The input is a
/// composer, not a pager: past this it scrolls internally rather than eating the
/// transcript. Ten rows is generous for a pasted statement while leaving room.
const MAX_HEIGHT: u16 = 10;

/// Multi-line editor state. `lines` is always non-empty (at least one, possibly
/// empty, logical line) so cursor math never has to special-case an empty
/// buffer. `cursor_col` is a CHAR index within the current line (not a byte
/// offset) so multi-byte input can never split a codepoint or panic on slicing.
pub struct Composer {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    placeholder: String,
    /// Submitted lines, oldest first, newest last. Recall walks this backwards.
    history: Vec<String>,
    /// `Some(i)` while browsing `history[i]`; `None` while editing the live
    /// buffer. Any edit drops back to `None` (the recalled text becomes a fresh
    /// draft), which matches "editing a recalled line and submitting pushes a
    /// new entry".
    history_pos: Option<usize>,
    /// The live draft saved when history browsing begins, restored when the user
    /// walks back down past the newest entry. Without it, Up-then-Down would lose
    /// whatever the user had half-typed.
    stash: Option<String>,
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

impl Composer {
    pub fn new() -> Self {
        Composer {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            placeholder: String::new(),
            history: Vec::new(),
            history_pos: None,
            stash: None,
        }
    }

    pub fn set_placeholder(&mut self, s: &str) {
        self.placeholder = s.to_string();
    }

    /// Feed a key. Returns `Some(Submit)` only when the user submits (Enter on a
    /// non-empty buffer). Shift+Enter, and a trailing `\` before Enter, insert a
    /// newline instead. Up/Down recall history when the cursor is on the first/
    /// last line. Never panics: unknown keys and non-Press events are ignored.
    ///
    /// Note vs the API sketch: the sketch mentioned a `history_mode` argument the
    /// integrator would pass; the agreed signature takes only the key, so recall
    /// is gated purely on cursor row here. When a popup is consuming Up/Down the
    /// integrator simply does not forward those keys to us.
    pub fn input(&mut self, key: KeyEvent) -> Option<Submit> {
        // Release events (and any non-Press/Repeat kind) are not edits. Windows
        // delivers Release for every key; acting on them would double every
        // keystroke. Repeat is a held key and IS a real edit, so allow it.
        if matches!(key.kind, KeyEventKind::Release) {
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Enter => return self.on_enter(shift),
            KeyCode::Char(c) if !ctrl => self.insert_char(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.cursor_col = 0,
            KeyCode::End => self.cursor_col = self.cur_len(),
            KeyCode::Up => self.on_up(),
            KeyCode::Down => self.on_down(),
            // Everything else (Ctrl-chords, F-keys, Tab, Esc, ...) is the app
            // loop's concern, not the editor's. Ignore, never panic.
            _ => {}
        }
        None
    }

    /// Insert pasted text as ONE edit. A multi-line paste (a theorem statement,
    /// a blueprint) becomes newlines in the buffer, NOT a series of Enter
    /// submits. This is the whole point of a separate paste path: `\n` in a
    /// paste must never be read as "send".
    pub fn paste(&mut self, text: &str) {
        self.leave_history();
        // Normalize CRLF/CR so a Windows clipboard does not leave stray `\r`.
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut segments = normalized.split('\n');
        // First segment merges into the current line at the cursor.
        if let Some(first) = segments.next() {
            self.insert_str_inline(first);
        }
        // Each remaining segment starts a new line (splitting the current one at
        // the cursor on the first break, appending after).
        for seg in segments {
            self.split_line_at_cursor();
            self.insert_str_inline(seg);
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Empty means no content at all: a single empty logical line. Whitespace
    /// still counts as content here; submit-time trimming decides emptiness for
    /// the submit gate separately.
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Reset the editing buffer (history is retained: clearing the draft must not
    /// erase recall). Called by the integrator after a submit is dispatched.
    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.history_pos = None;
        self.stash = None;
    }

    /// Render the buffer to styled lines for the input rect: a reverse-video
    /// cursor marker on the active line, or the dimmed placeholder when empty.
    /// `'static` so the app can hold the lines past this borrow.
    ///
    /// `width` is accepted for API symmetry with `desired_height` but not used to
    /// hard-wrap here: the integrator draws these through a `Paragraph` with
    /// `Wrap`, which soft-wraps for display. Hard-wrapping here would fight that
    /// and misplace the cursor across wrap boundaries.
    pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let _ = width;
        if self.is_empty() && !self.placeholder.is_empty() {
            // Cursor block first so the caret is visible on an empty field, then
            // the dimmed hint. Both owned, so the returned lines are `'static`.
            return vec![Line::from(vec![
                Span::styled(
                    " ".to_string(),
                    Style::default().add_modifier(Modifier::REVERSED),
                ),
                Span::styled(
                    self.placeholder.clone(),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ])];
        }
        self.lines
            .iter()
            .enumerate()
            .map(|(row, s)| {
                if row == self.cursor_row {
                    self.line_with_cursor(s)
                } else {
                    Line::from(Span::raw(s.clone()))
                }
            })
            .collect()
    }

    /// Desired height in rows: at least 1, grows with the number of logical lines
    /// AND their soft-wrapped length at `width`, capped at [`MAX_HEIGHT`].
    pub fn desired_height(&self, width: u16) -> u16 {
        let usable = width.max(1) as usize;
        let rows: usize = self
            .lines
            .iter()
            .map(|l| {
                // A char count, not bytes: wrapping is by columns of text. Every
                // line is at least one row even when empty.
                let chars = l.chars().count();
                chars.div_ceil(usable).max(1)
            })
            .sum();
        (rows.max(1) as u16).min(MAX_HEIGHT)
    }

    /// Record a submitted line for Up/Down recall. Skips empties and consecutive
    /// duplicates so recall is not clogged by repeats.
    pub fn push_history(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if self.history.last().map(String::as_str) == Some(line) {
            return;
        }
        self.history.push(line.to_string());
    }

    // -- internals -----------------------------------------------------------

    /// Char length of the current line. Char-based so the cursor cannot land
    /// mid-codepoint.
    fn cur_len(&self) -> usize {
        self.lines[self.cursor_row].chars().count()
    }

    fn on_enter(&mut self, shift: bool) -> Option<Submit> {
        // Continuation forms insert a newline rather than submitting:
        //  - Shift+Enter, the explicit "new line" chord.
        //  - a trailing backslash immediately before the cursor: consume the `\`
        //    and break the line, the shell-style line-continuation idiom.
        if shift {
            self.newline();
            return None;
        }
        if self.cursor_col > 0 {
            let chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
            if chars[self.cursor_col - 1] == '\\' {
                // Drop the backslash, then break.
                self.cursor_col -= 1;
                let mut c = chars;
                c.remove(self.cursor_col);
                self.lines[self.cursor_row] = c.into_iter().collect();
                self.leave_history();
                self.newline();
                return None;
            }
        }
        // A real submit. Trim decides emptiness; an all-whitespace buffer is
        // "empty" and does nothing, matching the old loop's `trim().is_empty()`.
        let text = self.text();
        if text.trim().is_empty() {
            return None;
        }
        self.clear();
        Some(Submit::Line(text))
    }

    fn newline(&mut self) {
        self.leave_history();
        self.split_line_at_cursor();
    }

    /// Break the current line at the cursor: text after the cursor moves down to
    /// a new line, and the cursor lands at its start.
    fn split_line_at_cursor(&mut self) {
        let chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
        let (left, right) = chars.split_at(self.cursor_col);
        let left: String = left.iter().collect();
        let right: String = right.iter().collect();
        self.lines[self.cursor_row] = left;
        self.lines.insert(self.cursor_row + 1, right);
        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    fn insert_char(&mut self, c: char) {
        self.leave_history();
        let mut chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
        chars.insert(self.cursor_col, c);
        self.lines[self.cursor_row] = chars.into_iter().collect();
        self.cursor_col += 1;
    }

    /// Insert a run of text (no newlines) at the cursor on the current line.
    fn insert_str_inline(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let mut chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
        let insert: Vec<char> = s.chars().collect();
        let n = insert.len();
        for (i, ch) in insert.into_iter().enumerate() {
            chars.insert(self.cursor_col + i, ch);
        }
        self.lines[self.cursor_row] = chars.into_iter().collect();
        self.cursor_col += n;
    }

    fn backspace(&mut self) {
        self.leave_history();
        if self.cursor_col > 0 {
            let mut chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
            self.cursor_col -= 1;
            chars.remove(self.cursor_col);
            self.lines[self.cursor_row] = chars.into_iter().collect();
        } else if self.cursor_row > 0 {
            // At column 0: join this line onto the end of the previous one, with
            // the cursor at the seam.
            let cur = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.cur_len();
            self.lines[self.cursor_row].push_str(&cur);
        }
    }

    fn delete(&mut self) {
        self.leave_history();
        let len = self.cur_len();
        if self.cursor_col < len {
            let mut chars: Vec<char> = self.lines[self.cursor_row].chars().collect();
            chars.remove(self.cursor_col);
            self.lines[self.cursor_row] = chars.into_iter().collect();
        } else if self.cursor_row + 1 < self.lines.len() {
            // At end of line: pull the next line up onto this one.
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.cur_len();
        }
    }

    fn move_right(&mut self) {
        if self.cursor_col < self.cur_len() {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// Up on the first line recalls the previous history entry; otherwise it
    /// moves the cursor up one line, keeping the column where possible.
    fn on_up(&mut self) {
        if self.cursor_row == 0 {
            self.history_prev();
        } else {
            self.cursor_row -= 1;
            self.cursor_col = self.cursor_col.min(self.cur_len());
        }
    }

    /// Down on the last line recalls the next (newer) history entry; otherwise it
    /// moves the cursor down one line.
    fn on_down(&mut self) {
        if self.cursor_row + 1 >= self.lines.len() {
            self.history_next();
        } else {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_col.min(self.cur_len());
        }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_pos {
            // First step into history: remember the live draft, jump to newest.
            None => {
                self.stash = Some(self.text());
                self.history.len() - 1
            }
            Some(0) => 0, // already at the oldest; stay put
            Some(i) => i - 1,
        };
        self.history_pos = Some(next);
        self.load_buffer(self.history[next].clone());
    }

    fn history_next(&mut self) {
        match self.history_pos {
            None => {} // not browsing; nothing newer to recall
            Some(i) if i + 1 < self.history.len() => {
                let i = i + 1;
                self.history_pos = Some(i);
                self.load_buffer(self.history[i].clone());
            }
            Some(_) => {
                // Walked past the newest entry: restore the stashed live draft.
                let draft = self.stash.take().unwrap_or_default();
                self.history_pos = None;
                self.load_buffer(draft);
            }
        }
    }

    /// Replace the buffer with `text`, cursor at the very end (last line, last
    /// column). Used by history recall.
    fn load_buffer(&mut self, text: String) {
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(str::to_string).collect()
        };
        self.cursor_row = self.lines.len() - 1;
        self.cursor_col = self.cur_len();
    }

    /// Any edit drops out of history-browsing: the recalled text is now a fresh
    /// draft, so a later submit records a NEW entry rather than mutating an old
    /// one. Idempotent, so edit paths can call it unconditionally.
    fn leave_history(&mut self) {
        self.history_pos = None;
        self.stash = None;
    }

    /// Build the active line with a reverse-video block at the cursor column
    /// (a space when the cursor sits at end of line). All spans own their text.
    fn line_with_cursor(&self, s: &str) -> Line<'static> {
        let chars: Vec<char> = s.chars().collect();
        let before: String = chars[..self.cursor_col].iter().collect();
        let (at, after) = if self.cursor_col < chars.len() {
            (
                chars[self.cursor_col].to_string(),
                chars[self.cursor_col + 1..].iter().collect::<String>(),
            )
        } else {
            (" ".to_string(), String::new())
        };
        Line::from(vec![
            Span::raw(before),
            Span::styled(at, Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    // Build a Press KeyEvent. crossterm's `KeyEvent::new` defaults kind to Press.
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn keym(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    fn typ(c: &mut Composer, s: &str) {
        for ch in s.chars() {
            c.input(key(KeyCode::Char(ch)));
        }
    }

    fn submitted(r: Option<Submit>) -> Option<String> {
        r.map(|Submit::Line(s)| s)
    }

    #[test]
    fn type_then_enter_submits_text() {
        let mut c = Composer::new();
        typ(&mut c, "hello world");
        let out = submitted(c.input(key(KeyCode::Enter)));
        assert_eq!(out.as_deref(), Some("hello world"));
        // Buffer is cleared after a submit.
        assert!(c.is_empty());
    }

    #[test]
    fn empty_enter_yields_none() {
        let mut c = Composer::new();
        assert!(submitted(c.input(key(KeyCode::Enter))).is_none());
        // Whitespace-only is also "empty" for the submit gate.
        typ(&mut c, "   ");
        assert!(submitted(c.input(key(KeyCode::Enter))).is_none());
    }

    #[test]
    fn shift_enter_inserts_newline_no_submit() {
        let mut c = Composer::new();
        typ(&mut c, "line1");
        let out = c.input(keym(KeyCode::Enter, KeyModifiers::SHIFT));
        assert!(submitted(out).is_none());
        typ(&mut c, "line2");
        assert_eq!(c.text(), "line1\nline2");
    }

    #[test]
    fn trailing_backslash_enter_is_newline() {
        let mut c = Composer::new();
        typ(&mut c, "a\\"); // ends with a backslash
        let out = c.input(key(KeyCode::Enter));
        assert!(submitted(out).is_none());
        // The backslash is consumed and a line break inserted.
        assert_eq!(c.text(), "a\n");
        typ(&mut c, "b");
        assert_eq!(c.text(), "a\nb");
    }

    #[test]
    fn multiline_paste_is_one_buffer_not_submits() {
        let mut c = Composer::new();
        c.paste("theorem foo :\n  1 + 1 = 2\nby simp");
        // No submit happened; it is all one buffer.
        assert_eq!(c.text(), "theorem foo :\n  1 + 1 = 2\nby simp");
        assert!(!c.is_empty());
        // CRLF is normalized to plain newlines.
        let mut c2 = Composer::new();
        c2.paste("a\r\nb");
        assert_eq!(c2.text(), "a\nb");
    }

    #[test]
    fn up_after_two_submits_recalls_previous() {
        let mut c = Composer::new();
        // Simulate the integrator: on each submit, push into history.
        typ(&mut c, "first");
        if let Some(Submit::Line(s)) = c.input(key(KeyCode::Enter)) {
            c.push_history(&s);
        }
        typ(&mut c, "second");
        if let Some(Submit::Line(s)) = c.input(key(KeyCode::Enter)) {
            c.push_history(&s);
        }
        // Buffer is empty (cursor on the only line, which is the first line).
        c.input(key(KeyCode::Up));
        assert_eq!(c.text(), "second");
        c.input(key(KeyCode::Up));
        assert_eq!(c.text(), "first");
        // Cannot go older than the oldest.
        c.input(key(KeyCode::Up));
        assert_eq!(c.text(), "first");
        // Down walks back to newer, then restores the (empty) live draft.
        c.input(key(KeyCode::Down));
        assert_eq!(c.text(), "second");
        c.input(key(KeyCode::Down));
        assert_eq!(c.text(), "");
    }

    #[test]
    fn editing_a_recalled_line_pushes_new_entry() {
        let mut c = Composer::new();
        typ(&mut c, "alpha");
        if let Some(Submit::Line(s)) = c.input(key(KeyCode::Enter)) {
            c.push_history(&s);
        }
        c.input(key(KeyCode::Up));
        assert_eq!(c.text(), "alpha");
        // Edit the recalled entry, then submit.
        typ(&mut c, "X");
        assert_eq!(c.text(), "alphaX");
        let out = submitted(c.input(key(KeyCode::Enter)));
        assert_eq!(out.as_deref(), Some("alphaX"));
        c.push_history("alphaX");
        // History now has both; original is untouched.
        c.input(key(KeyCode::Up));
        assert_eq!(c.text(), "alphaX");
        c.input(key(KeyCode::Up));
        assert_eq!(c.text(), "alpha");
    }

    #[test]
    fn backspace_deletes_and_merges_lines() {
        let mut c = Composer::new();
        typ(&mut c, "ab");
        c.input(key(KeyCode::Backspace));
        assert_eq!(c.text(), "a");
        // Newline then backspace at col 0 merges the lines back.
        c.input(keym(KeyCode::Enter, KeyModifiers::SHIFT));
        typ(&mut c, "cd");
        assert_eq!(c.text(), "a\ncd");
        // Move to start of "cd" and backspace to merge.
        c.input(key(KeyCode::Home));
        c.input(key(KeyCode::Backspace));
        assert_eq!(c.text(), "acd");
    }

    #[test]
    fn home_end_and_left_right_move_cursor() {
        let mut c = Composer::new();
        typ(&mut c, "abc");
        c.input(key(KeyCode::Home));
        // Insert at start.
        typ(&mut c, "Z");
        assert_eq!(c.text(), "Zabc");
        c.input(key(KeyCode::End));
        typ(&mut c, "!");
        assert_eq!(c.text(), "Zabc!");
        // Left twice, insert in the middle.
        c.input(key(KeyCode::Left));
        c.input(key(KeyCode::Left));
        typ(&mut c, "-");
        assert_eq!(c.text(), "Zab-c!");
    }

    #[test]
    fn unknown_keys_and_release_never_panic_or_edit() {
        let mut c = Composer::new();
        typ(&mut c, "keep");
        // Esc, Tab, F-keys, Ctrl-chords: ignored by the editor.
        c.input(key(KeyCode::Esc));
        c.input(key(KeyCode::Tab));
        c.input(keym(KeyCode::Char('c'), KeyModifiers::CONTROL));
        // A Release event for a printable key must not insert it.
        let mut rel = key(KeyCode::Char('x'));
        rel.kind = KeyEventKind::Release;
        c.input(rel);
        assert_eq!(c.text(), "keep");
    }

    #[test]
    fn desired_height_grows_and_caps() {
        let mut c = Composer::new();
        assert_eq!(c.desired_height(20), 1);
        c.input(keym(KeyCode::Enter, KeyModifiers::SHIFT));
        c.input(keym(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(c.desired_height(20), 3);
        // A long single line wraps and adds rows.
        let mut c2 = Composer::new();
        typ(&mut c2, &"x".repeat(25));
        assert_eq!(c2.desired_height(10), 3); // ceil(25/10)
                                              // Cap holds.
        let mut c3 = Composer::new();
        for _ in 0..40 {
            c3.input(keym(KeyCode::Enter, KeyModifiers::SHIFT));
        }
        assert_eq!(c3.desired_height(20), MAX_HEIGHT);
    }

    #[test]
    fn lines_show_placeholder_when_empty() {
        let mut c = Composer::new();
        c.set_placeholder("Ask the agent, or /command");
        let ls = c.lines(40);
        assert_eq!(ls.len(), 1);
        // The placeholder text appears somewhere in the rendered spans.
        let rendered: String = ls[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(rendered.contains("Ask the agent"));
    }
}

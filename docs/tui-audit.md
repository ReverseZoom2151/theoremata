# TUI behavioral audit: ours vs Codex CLI

Our TUI (`app/tui/`) is a clean-room adaptation of Codex CLI's TUI design. This
audit walks each interactive behavior, compares how Codex handles it to how ours
does, and records a verdict. Fixes landed this pass are marked `gap-fixed`;
things we deliberately did not copy are `ok` or `deferred` with a reason.

Reference read (Apache-2.0, behavior only, no code copied):
`codex-rs/tui/src/bottom_pane/{scroll_state,selection_popup_common,popup_consts}.rs`,
`pager_overlay.rs`, `keymap.rs`, `bottom_pane/textarea.rs`.

## Gap table

| Behavior | How Codex does it | How ours did it | Verdict |
|---|---|---|---|
| More-content-ABOVE cue | Pager renders a scroll percentage; chat pins/follows bottom and the pager shows position | Only a "N more below" cue (shown when scrolled up). Nothing told the user content was hidden ABOVE while auto-following the bottom | gap-fixed: added a "N more above" cue that shows whenever `top > 0`, including while auto-following |
| Tall command result opens off-screen | Selection popups `ensure_visible`; pager can `scroll_chunk_into_view` | A tall cell (e.g. `/model`, `/graph`) pushed while stuck to bottom had its top scrolled off with no cue: the reported "cannot see all models" bug | gap-fixed: a one-shot `pending_scroll_to` pins the new cell's top (clamped to bottom) so a tall result opens at its start |
| PageUp/PageDown clamping | `scroll_offset.saturating_add/sub(page)` | `scrolled_up/down` clamp to `[0, max_top]`, restore auto-follow at bottom | ok (already correct) |
| Home/End jump top/bottom | Pager `jump_top`/`jump_bottom` on Home/End | Home/End were composer-only (line start/end) | gap-fixed: added Ctrl+Home / Ctrl+End for transcript top / live-bottom; plain Home/End stay composer editing keys (deliberate, see below) |
| Half-page scroll (Ctrl+U/D) | Pager supports half-page | none | deferred: the session decision is "scrolling is PageUp/PageDown only, intentionally"; Ctrl+U is a composer kill-line and Ctrl+D is quit, so half-page would fight the composer |
| Command popup windows around selection | `ScrollState` + `compute_item_window_start` keep the selected row visible | already windows around the selection (fixed earlier this session) | ok |
| Mention popup windowing / cue | Same scroll-window machinery | ranks and caps candidates at 8 best matches; no window past 8 | ok/deferred: the candidate list is pre-ranked and narrows as you type, so the top 8 are the useful set; a windowed scroll past 8 is low value and not worth the added state |
| Popup wrap at ends, Esc dismiss | Wrap on Up/Down, Esc cancels | both popups wrap; Esc clears the composer which deactivates both | ok |
| Popup "+N more" position cue | Implicit via the scroll window | none | deferred: low value; the windowed view already keeps the selection on screen |
| Focus / key routing (one consumer per key) | Keys routed through a view stack; the focused view consumes | `graph_nav` claims Up/Down only when the panel is shown AND focused; popups consume only nav keys; else composer. Pure, unit-tested | ok |
| Composer word delete (Ctrl+W, Alt/Ctrl+Backspace) | `delete_backward_word` bound to Ctrl+W, Alt/Ctrl+Backspace | ours dropped all Ctrl chords; only plain Backspace | gap-fixed: added Ctrl+W and Alt/Ctrl+Backspace word delete |
| Composer line-start/end (Ctrl+A / Ctrl+E) | bound alongside Home/End | Home/End only | gap-fixed: added Ctrl+A / Ctrl+E |
| Composer kill line (Ctrl+K / Ctrl+U) | `kill_line_end` / `kill_line_start` | none | gap-fixed: added Ctrl+K (to end) / Ctrl+U (to start) |
| Composer word motion (Ctrl/Alt + Left/Right) | `move_word_left/right` on Alt/Ctrl arrows | none | gap-fixed: added Ctrl/Alt + Left/Right |
| AltGr characters on Windows | textarea preserves Ctrl+Alt typed chars | ours guarded only on `!ctrl`, so it happened to work, but the new chords had to keep it working | ok/gap-fixed: chord guards treat only Ctrl-only or Alt-only as chords; Ctrl+Alt (AltGr) still types the character (unit-tested) |
| Multi-line, paste, history recall | textarea + paste burst + history | already present: Shift+Enter, trailing-backslash continuation, one-shot paste, Up/Down history with stash | ok |
| Streaming preview then commit | markdown stream then authoritative history insert | `active_stream` preview is discarded (set to None) in the same drain that pushes the authoritative cell, so text is never doubled; the viewport stays pinned to bottom during a turn | ok (no flicker, no jump) |
| Footer reflects mode / discoverability | dynamic footer/status line | footer shows model, busy/idle, status, and hints; graph-focus is reachable via Ctrl+G | gap-fixed (minor): footer hint now advertises "PgUp/PgDn scroll" since the mouse is gone |
| Resize reflow | width-aware cells, reflow on resize | cells are width-aware; draw re-splits and re-wraps every frame; scroll clamps to `max_top` each draw | ok |
| Quit / cleanup on every path incl. panic | installs teardown that restores the terminal | cleanup ran on normal return and on `?` error, but a PANIC in the draw loop skipped it, stranding raw mode + alt screen + hidden cursor | gap-fixed: a `TerminalGuard` Drop restores raw mode, alt screen, bracketed paste, and cursor on every exit path, panic included |
| Ctrl-C / Ctrl-D | Ctrl-C interrupts/quits; Ctrl-D EOF | Ctrl-C quit; Ctrl-D did nothing | gap-fixed: Ctrl-D on an empty, non-busy composer quits (shell EOF); with text present it is a no-op so it cannot eat a message |

## Highest-impact fixes

1. The reported "cannot see all models" bug: a tall synchronous command result
   now opens at its top (`pending_scroll_to`), and a "N more above" cue appears
   whenever content is hidden above the viewport, including while auto-following.
2. Composer editing parity: Ctrl+W / Alt+Backspace / Ctrl+Backspace word delete,
   Ctrl+A / Ctrl+E line ends, Ctrl+K / Ctrl+U kill line, Ctrl/Alt+Left/Right word
   motion. These are the everyday keys a reimplemented composer silently dropped.
3. Panic-safe terminal restore via a Drop guard, so a crash can no longer strand
   the user's terminal in raw mode on the alternate screen.

## Soundness note

No change touches verification, gating, or the honesty rule. `/verify`, the
graph panel, and every leaf cell still render only `formally_verified` green
(`panel_node_maps_status_and_kind_via_display` and
`verify_cell_greens_only_the_verified_node` still pass). The chat still cannot
mark a node `FormallyVerified`. All new logic is pure presentation: scroll math,
an above-cue count, composer token edits, and terminal teardown.

## Deliberately skipped (and why)

- Half-page and single-line scroll keys: the standing decision is PageUp/PageDown
  only; adding Ctrl+U/Ctrl+D would collide with the composer's kill-line and quit.
- Mention popup windowing past 8 candidates: the list is pre-ranked and narrows as
  you type; a scroll window past the top 8 is state we do not need.
- Popup "+N more" numeric cue: the windowed view already keeps the selection
  visible; the extra glyph is low value.

## Reported: no change needed outside `app/tui/`

`tui::run`'s signature is unchanged, so `app/lib.rs` needs no edit. Nothing outside
`app/tui/` was touched.

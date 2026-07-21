# TUI redesign: a Codex-shaped cockpit for Theoremata

Synthesis of [`tui-study-input.md`](tui-study-input.md) (composer, popups, event loop)
and [`tui-study-render.md`](tui-study-render.md) (transcript, cells, theming), into one
build plan. Codex CLI (`codex-main/codex-rs/`, Apache-2.0) is the design reference; this is
a clean-room adaptation with attribution, not a port.

## The diagnosis, in one line

Ours is a tabbed 3-pane layout with a one-line input that **freezes the whole UI during a
model call** and prints results as raw JSON. Codex is a single scrolling **transcript of
typed cells** plus a rich **composer**, kept responsive by an **event bus** so a running
turn never blocks input. The gap is architectural, not cosmetic.

## The four changes that matter (in priority order)

### 1. A non-blocking event bus (the load-bearing change)

Today `run_blocking` holds the UI thread for the minutes a local model takes; Esc only
lands between rounds. Replace it with `std::mpsc` + one worker thread per in-flight action
(no tokio; the binary stays sync).

```
enum UiEvent {
    StreamDelta(String),                       // model token -> append to the active cell
    ActionProgress { label: String },          // status line
    ActionDone { label: String, cell: Cell },  // a finished result cell
    TurnDone,                                   // agentic loop finished
}
```

The main loop polls keys on a short timeout AND drains `rx.try_recv()` each tick, then
draws. Keys (Esc/Ctrl-C included) are processed every tick, so interrupt is live the whole
time, not just between rounds. `MAX_ACTION_ROUNDS` stays as a safety net.

**Resolved design decision (the studies flagged this):** the worker thread gets its **own**
`Store` connection opened from the same DB path, its own `CommandProvider` built from
`config.model_command`, and a `Config` clone. SQLite `Connection` is `Send`, and a separate
connection per worker avoids a shared `Mutex` and lock contention; the main thread keeps its
own connection for rendering. This sidesteps the `&dyn ModelProvider` `Send` problem
entirely (the worker constructs its own provider). Actions are serialized (one at a time),
so two connections never race on a write.

### 2. A transcript of typed cells (replaces the redraw-from-DB panes)

Adopt Codex's `HistoryCell` shape: a `Vec<Box<dyn HistoryCell>>` of committed cells plus one
mutable `active_cell` that streaming deltas append to, committed on finalize. Each cell owns
its typed source data and renders itself to styled ratatui lines at a given width and reports
its height. This replaces both the per-frame `store.nodes/events/messages` rebuild and the
flat `app.stream` string.

Our concrete cell set (each maps to data `execute_action` already computes):

- `UserMsgCell`, `AgentMsgCell` (streamed), `ReasoningCell` (dim, collapsible)
- `VerdictCell` -- a prove/hammer report: glyph + `compiled` / `axioms_clean` /
  `statement_preserved` / `live`, with the generated proof code syntax-tinted and collapsible
- `FalsifyCell` -- counterexample assignment, or "no counterexample in domain"
- `SweepCell` -- the Fresh / RepairCandidate / MathematicsMoved / Unknown census
- `AgentRunCell` -- run id / certified / steps
- `ProposalCell` -- a pending graph mutation with inline approve/reject affordance
- `NoticeCell`, `ErrorCell`

### 3. A real composer + slash-command popup

Replace the one-line input and the flat string-match parser:

- Multi-line composer with a submit contract: Enter submits, Shift+Enter (or a trailing
  backslash) inserts a newline; it returns a `Submit` intent rather than executing inline.
- A slash popup triggered by a leading `/`, ranked exact-then-prefix over a **single command
  registry** (`CommandSpec { name, args, help, .. }`). That registry also generates `/help`,
  killing the three hand-maintained lists that drift today.

### 4. A modal picker + footer

- One `SelectionView` (rows carry a closure fired on accept) serves both the `/model` picker
  (reusing the existing `ollama list` parsing) and proposal approve/reject.
- A footer/status line always shows the active model, the busy state, and key hints.

## Theming (the honesty rule is load-bearing)

A minimal semantic palette with a fixed glyph vocabulary:

- verified / true: green `✔`
- failed / refuted: red `✗`
- working: cyan spinner
- **mock or stale**: yellow `⚠` -- and `live: false` or "no counterexample found" must **never**
  render as a green check. The gate discipline shows up in the colors.
- proposal: magenta `◆`

## Layout

Retire the always-on tabs and the fixed 32% inspector. The transcript is full-width by
default; the proof graph moves to a **togglable right side panel**, and node inspection
becomes an inline `NodeInspectCell`. Trajectory is reachable via `/events` rendered as cells
rather than a permanent pane.

## What we deliberately do NOT take

Codex is 230k lines; we want the essence. Skip: the diff viewer, file-search/@-mentions,
external-editor integration, backtracking, the effort/approval UI, ide-context, and the
dozens of settings views. None serve a theorem-proving cockpit.

## Build phases (each compiles and is usable)

1. **Event bus + worker threads.** Make actions non-blocking behind the existing UI. Biggest
   win, smallest surface. The UI stops freezing.
2. **Cell transcript.** Replace the output pane with the cell model + the verdict/falsify/
   sweep cells and streaming append. Results stop being JSON dumps.
3. **Composer + slash popup + command registry.** Replace the input and the three drifting
   command lists.
4. **Modal picker + footer + theme.** `/model` and proposals through one picker; semantic
   colors and glyphs.
5. **Layout.** Full-width transcript, togglable graph panel, inline node inspection.

Estimated ~1.5 to 2.5k lines, split into `tui/composer.rs`, `tui/command_popup.rs`,
`tui/cell.rs`, `tui/event.rs`, and a slimmer `tui/mod.rs`. Every phase keeps the soundness
boundary intact: the chat can never mark a node verified, only the gate can.

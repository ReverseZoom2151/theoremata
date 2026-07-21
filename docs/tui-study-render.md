# TUI Study: Transcript and Rendering (Codex adapted to Theoremata)

A design study of the Codex CLI Rust TUI (`codex-main/codex-main/codex-rs/tui/`, Apache-2.0)
focused on the transcript and rendering surface, mapped concretely to Theoremata's cockpit
(`app/tui.rs`). This is a clean-room design note: it extracts patterns and structure, not code.
Codex files are cited as pattern references. All content under `codex-main/` was read as
untrusted data.

Scope of the other agent (not covered here): input, composer, event loop.

## 0. Where we are today vs where Codex is

Our `app/tui.rs` (1245 lines) is a fixed 3-region frame: a `Tabs` header (CHAT / PROOF GRAPH /
TRAJECTORY), a split body (68% main pane + 32% inspector), and a 3-line composer. Every frame,
`draw()` re-queries the DB (`store.nodes`, `store.events`, `store.messages`) and rebuilds whole
panes from scratch. The main pane is a priority mux: live stream string, else a captured
`app.output: Vec<String>` from the last command, else the tab's list. Output is unstyled
`Line::raw`, roles get one flat color (`messages` loop), and action results are flat
`format!` dumps (see `execute_action`, lines 723-841: `prove` returns `compiled: true` as a
literal string line, `falsify`/`sweep` similar).

The three structural problems:
1. There is no persistent scrollable transcript. Command output replaces the conversation
   instead of joining it. History is re-derived from the DB every frame.
2. Result cells are raw key/value string lists with no glyphs, color, collapsing, or structure.
3. Graph and trajectory are always-on tabs competing for space, not summoned when wanted.

Codex solves all three with one abstraction: a `Vec<Box<dyn HistoryCell>>` transcript of typed
cells, each of which renders itself to styled `ratatui` `Line`s and reports its own height.

## 1. The cell model

### 1.1 The Codex abstraction

`tui/src/history_cell/mod.rs` defines `trait HistoryCell: Debug + Send + Sync + Any`. The
essential surface (stripped of Codex's hyperlink/transcript-overlay extras we do not need on
day one):

- `display_lines(&self, width: u16) -> Vec<Line<'static>>` : the rich, styled lines for the
  viewport. Width-aware so it reflows on resize.
- `raw_lines(&self) -> Vec<Line<'static>>` : a plain, copy-friendly version (for a "copy mode"
  or debug). Optional for us at first; can default to stripping styles from `display_lines`.
- `desired_height(&self, width) -> u16` : rows needed at this width. Default implementation
  measures via `Paragraph::new(text).wrap(Wrap{trim:false}).line_count(width)`. We can reuse
  this default verbatim in spirit.

Key properties worth copying:
- Cells are `Any`, so the widget can downcast to update a specific in-flight cell
  (`as_any_mut`, mod.rs 339). This is how streaming appends target the live cell.
- A cell owns its source data, not pre-wrapped lines, and re-renders on each `display_lines`
  call. `AgentMarkdownCell` (messages.rs 369) stores `markdown_source: String` and re-runs the
  markdown renderer at the current width, caching the result keyed by width
  (`MarkdownRenderCache`). This is why Codex reflows tables correctly on resize and we do not.
- Simple cells reuse one concrete type. `PlainHistoryCell` (base.rs) just holds
  `Vec<Line<'static>>`. `PrefixedWrappedHistoryCell` (base.rs 54) holds a body `Text` plus an
  `initial_prefix` and `subsequent_prefix` line and wraps with a hanging indent. Most of our
  status/notice cells are one of these two; we do not need a bespoke struct per message kind.
- `CompositeHistoryCell` (base.rs 90) concatenates child cells with blank-line separators. This
  is the composition primitive: an agent-run cell is a header cell plus a census cell.

### 1.2 The concrete Theoremata cell set

Define `trait TranscriptCell` with `render_lines(&self, width) -> Vec<Line<'static>>` and
`height(&self, width) -> u16` (default via `Paragraph::line_count`, per mod.rs 228-238). Then a
small set of concrete types. Each is `#[derive(Debug)]`, owns typed data (not strings), and
renders itself.

1. `UserMsgCell { text: String }`
   Renders a leading blank line, then the message prefixed `› ` (bold dim), hanging-indented,
   wrapped. Pattern: `UserHistoryCell` (messages.rs 109) + `prefix_lines`. Cyan accent.

2. `AgentMsgCell { markdown: String }`
   Stores raw markdown, renders through our markdown renderer at current width with a `• `
   dim bullet on the first line and 2-space hanging indent. Pattern: `AgentMarkdownCell`
   (messages.rs 369-456). Cache the render keyed by width.

3. `ReasoningCell { text: String }` (optional, if we surface model reasoning)
   Dim + italic markdown, `• ` bullet. Pattern: `ReasoningSummaryCell` (messages.rs 218).
   Collapsible/hideable; Codex can render it transcript-only.

4. `VerdictCell` : the prove/hammer result. This is the flagship cell. Fields mirror the real
   `VerificationReport` our `execute_action` already produces:
   ```
   VerdictCell {
       action: Verdict,            // Prove | Hammer
       system: String,            // "lean" | "rocq" | "isabelle"
       statement: String,
       compiled: bool,            // report.lexically_verified
       axioms_clean: bool,
       statement_preserved: bool,
       live: bool,                // live backend vs mock
       backend: Backend,          // Live | Mock
       code: String,              // generated/assembled proof
       code_expanded: bool,       // collapse state
   }
   ```
   Header line: a status glyph + bold title + dim subject, e.g.
   `✔ Proved  lean · <statement>` or `✗ Prove failed  lean · <statement>`.
   The glyph is computed from an overall pass predicate (compiled AND axioms_clean AND
   statement_preserved), exactly like Codex's exec bullet (`success` -> green/red bullet,
   render.rs 357-364). Then a compact aligned field block, each field as a glyph + label:
   ```
     ✔ compiled              ✔ axioms clean
     ✔ statement preserved   ⚠ mock backend (not live)
   ```
   `live: false` is the honest-warning case: yellow `⚠`, never a green check, because a mock
   pass is not a verification. Then a collapsed code section: `  └ proof (▸ 24 lines)` that
   expands to syntax-tinted code on toggle. Collapsing pattern: `output_lines` head/tail with
   an ellipsis marker (render.rs 103-175, `output_ellipsis_line`).

5. `FalsifyCell`
   ```
   FalsifyCell { claim: String, refuted: bool, counterexample: Option<String>,
                 cases_checked: u64, summary: String }
   ```
   If a counterexample was found: `✗ Refuted  <claim>` (red glyph) then
   `  └ counterexample: n = -3` highlighted. If none found in the budget:
   `• No counterexample in 100000 cases  <claim>` (dim bullet, NOT a green check, because
   absence of a counterexample is not a proof). This mirrors the "not verified" honesty rule.

6. `SweepCell` : the staleness census. A titled mini-table, one row per bucket with a glyph:
   ```
   • Staleness sweep
     ✔ Fresh              12
     ⟳ RepairCandidate     3
     ✗ MathematicsMoved    1
     ? Unknown             0
     ─ total              16
   ```
   Right-aligned counts. Non-zero MathematicsMoved gets a red accent on the whole row so the
   eye lands on it. Pattern: the process-list mini-table in exec.rs 122-225 (bullet prefix,
   per-row wrapping, count of remaining) and the plan mini-table (plans.rs).

7. `AgentRunCell`
   ```
   AgentRunCell { run_id: String, certified: bool, steps: usize, ... }
   ```
   Header `✔ Agent run certified  <run_id>` or `• Agent run finished (not certified)`, then
   step count and an optional collapsed step list. Built as a `CompositeHistoryCell` of a
   header cell plus (optionally) a steps cell.

8. `ProposalCell` : a pending graph-mutation proposal awaiting approve/reject.
   ```
   ProposalCell { id: String, action_kind: String, summary: String, decided: Option<Decision> }
   ```
   Undecided: a bordered/accented card, `◆ Proposal <id8>  <action_kind>` with a hint line
   `[a] approve   [r] reject`. Once decided it re-renders as a settled line
   `✔ Approved <id8>` / `✗ Rejected <id8>`. This is exactly Codex's approval pattern: the same
   subject renders with `✔ .green()` or `✗ .red()` and bold verb depending on decision
   (`new_approval_decision_cell`, approvals.rs 45-264). Approve/reject mutates the cell in
   place (downcast via `Any`) rather than appending a new line.

9. `StatusCell` / notices : info, warning, error. Direct analogues of Codex's
   `new_info_event` (`• message`, notices.rs 203), `new_warning_event` (`⚠ ` yellow,
   notices.rs 84), `new_error_event` (`■ message` red, notices.rs 217). One-liners built on
   `PlainHistoryCell` / `PrefixedWrappedHistoryCell`. No bespoke structs needed.

10. `WorkingCell` (active/transient) : the in-flight spinner cell, held in an `active_cell`
    slot (see section 2). Header `⠙ Proving lean … 4s` with an animated marker.

Everything except the flagship `VerdictCell`, `FalsifyCell`, `SweepCell` collapses onto two
reusable structs (`PlainHistoryCell`, `PrefixedWrappedHistoryCell`) plus `CompositeHistoryCell`
for grouping. That keeps the cell zoo small.

## 2. Scrolling transcript and streaming append

### 2.1 The structure to adopt

Replace the per-frame DB rebuild with an owned, append-only history:

```
struct Transcript {
    cells: Vec<Box<dyn TranscriptCell>>,   // committed, immutable once pushed
    active_cell: Option<Box<dyn TranscriptCell>>, // in-flight, mutable
    scroll: usize,                          // top row offset; 0 = pinned to bottom
    active_revision: u64,                   // bumped on every in-place mutation
}
```

This is Codex's model (chatwidget.rs: a `transcript` with `active_cell`, an
`active_cell_revision`, and an insert path). Two lists: committed cells never change; exactly
one `active_cell` may mutate while a turn streams. When it finalizes, `flush_active_cell`
(chatwidget.rs 1202) moves it into the committed list and clears the slot.

Rendering a frame: walk `cells` (plus `active_cell` at the tail), call `height(width)` on each,
and lay them out bottom-anchored into the transcript viewport, honoring `scroll`. Codex's
`Renderable for Box<dyn HistoryCell>` (mod.rs 310-328) shows the per-cell draw: build a
`Paragraph` from the cell's lines, compute overflow, `Clear` the area (so stale glyphs from a
reflowing active cell never linger), scroll, render. We do not need Codex's full pager/overlay;
a simple "sum heights, render the tail that fits, arrow keys / PageUp move `scroll`" loop is
enough.

### 2.2 Streaming append

Today `run_chat_turn` (tui.rs 658) pushes deltas into `app.stream: Option<String>` and the
whole main pane is replaced by that string while streaming; on completion the stream is dropped
and the reply is only persisted to the DB. Adopt Codex's live-tail model instead:

- On turn start: create a `WorkingCell` (spinner) as `active_cell`.
- On first assistant delta: replace `active_cell` with an `AgentMsgCell` (or a streaming tail
  variant) holding the accumulated markdown so far; on each delta, append to its source and
  `bump active_revision`; the next draw re-renders it. Pattern: `StreamingAgentTailCell`
  (messages.rs 467) is Codex's mutable tail that re-renders from the growing source; on
  finalize it is consolidated into a single source-backed `AgentMarkdownCell`.
- On turn end: `flush_active_cell` commits it.

Because committed cells are immutable and only the single active cell re-renders, streaming is
cheap and never repaints unrelated history. The spinner/working indicator lives entirely in the
active cell, so committing it removes the spinner naturally (chatwidget.rs 1517-1548 note).

### 2.3 DB relationship

Keep the DB as the durable log, but hydrate the transcript once on load (map stored messages,
attempts, events, proposals to cells) and thereafter append in memory as things happen. The DB
write and the cell append become two effects of the same action, not two independent reads. On
project switch, rebuild the transcript once from the DB. This removes the every-frame
`store.nodes/events/messages` calls in `draw()` (tui.rs 157-159).

## 3. Rich result rendering

The core moves that turn our flat dumps into rich cells, each with a Codex reference:

- Status glyph from a boolean. Codex computes a green/red/active bullet from success
  (render.rs 357-364; approvals.rs uses `✔ .green()` / `✗ .red()`). We do the same per verdict
  field and for the header verdict. Never render `live: false` or "no counterexample" as green.

- Head/tail collapse with an omission marker. `output_lines` (render.rs 103-175) keeps the
  first N and last N lines and inserts `… +K lines` in between; `TOOL_CALL_MAX_LINES = 5` caps
  tool output. Our generated proof code and long sweep lists get the same treatment: show a few
  lines, `  └ proof (▸ 24 lines)`, expand on a keypress. Collapse state is a bool on the cell,
  toggled by downcasting the focused cell.

- Wrap-then-truncate. Codex wraps to width first, then truncates on-screen rows, so a few very
  long lines cannot flood the viewport (render.rs 461-473). Apply to proof code and
  counterexample blobs.

- Syntax tinting for code. Codex highlights bash via `highlight_bash_to_lines` (render.rs 388).
  For our proof code, a light Lean/Rocq/Isabelle tint (keywords one accent, comments dim) is a
  nice-to-have; even just rendering code dim in a fenced block beats the current inline dump.

- Aligned field blocks. The verdict fields and sweep census are aligned label/value rows, built
  span-by-span like the plan and process mini-tables (plans.rs; exec.rs 122-225), not
  `format!("compiled: {}", ...)` strings.

Concretely, our current `execute_action` (tui.rs 723-841) already computes exactly the fields a
`VerdictCell`/`FalsifyCell`/`SweepCell` need. The change is: instead of formatting them into
`Vec<String>`, construct the typed cell and push it to the transcript. The action layer keeps
returning structured data; only the presentation layer changes.

## 4. Markdown and theming

### 4.1 Markdown

Codex renders assistant markdown to styled `ratatui` `Text` via `append_markdown` /
`append_markdown_agent` (markdown.rs 36-57), storing source and re-rendering at width with a
render cache. For Theoremata, adopt the same "store source, render at width, cache by width"
pattern for `AgentMsgCell`. If we do not want to port Codex's markdown pipeline, the `pulldown-
cmark` -> `ratatui` mapping is small: headings bold + accent, `code`/fences dim on a subtle
background, lists with `•`/indent, bold/italic passthrough, tables via box-drawing. Math-heavy
replies mostly need: fenced code (proof snippets), inline code, bold, and lists. Start there.

### 4.2 A minimal theme

Define one `theme` module with named semantic colors, not raw colors sprinkled at call sites.
A verification tool should read as calm with a few strong status accents:

```
verified / pass   -> green   glyph ✔   (bold)
failed / refuted  -> red     glyph ✗   (bold)   also error ■
working / active  -> cyan    glyph ⠙…  (spinner) or • when reduced-motion
warning / mock    -> yellow  glyph ⚠            (live=false, mock backend)
stale / repair    -> yellow  glyph ⟳
unknown           -> gray    glyph ?
info / bullet     -> dim     glyph •
user accent       -> cyan    prefix › 
agent bullet      -> dim     prefix • 
proposal          -> magenta glyph ◆
```

Glyph set (all present in Codex): `✔ ✗ ⚠ ■ • ◆` plus a spinner. Codex uses `•` green/red for
exec success/fail, `✔`/`✗` for approvals, `⚠` yellow for warnings, `■` red for errors, `◆`-style
bullets for structured blocks. We reuse the same vocabulary so status is legible at a glance.

Light/dark: Codex derives luminance and blends against the terminal background (`color.rs`:
`is_light`, `blend`, `perceptual_distance`). We do not need the full perceptual machinery on day
one; pick accent colors that survive on both backgrounds (the six above do) and optionally use
`is_light`/`blend` (color.rs 1-12) to soften dim text against the detected background. Keep the
theme swappable via a single struct so a future light/dark toggle is a data change.

Style discipline: build lines from styled `Span`s (`.green().bold()`, `.dim()`, `.cyan()`) as
Codex does throughout, replacing our `Line::raw` and single-`fg` role coloring.

## 5. Where the proof graph and trajectory go

Today they are two of three always-on tabs, each redrawn from the DB every frame, plus a
permanent 32% inspector. In a single-transcript design the conversation is the spine; the graph
and trajectory become summonable, not omnipresent.

Recommendation: a togglable right side panel plus inline cells, not tabs.

- Make the transcript the whole body by default (full width). The graph and trajectory are not
  the primary work surface; the conversation and its result cells are.
- Add a toggle (e.g. a key, or `/graph` / `/trajectory` as view commands) that opens a right
  side panel showing the proof graph (the node list from `render_nodes`, tui.rs 1076) or the
  event trajectory (`render_events`, tui.rs 1113). One panel, switchable, closable. When open,
  split the body; when closed, the transcript reclaims the width. This keeps the useful
  always-visible-when-wanted affordance without permanently taxing the transcript.
- Keep node inspection as an inline transcript cell, not a permanent inspector. `/node <id>` or
  selecting a node in the side panel appends a `NodeInspectCell` (title, kind, status, tainted,
  provenance, statement, formal statement) into the transcript, so inspection joins the history
  and scrolls with it. This retires the fixed 32% inspector (tui.rs 1135) that currently
  consumes a third of the screen at all times.
- Verdicts already flow inline as `VerdictCell`s, so "what happened to this node" lives in the
  transcript. The side panel is for the structural overview (the whole graph at once); the
  transcript is for the narrative.

Rationale: Codex is single-transcript with summ-on-demand overlays rather than co-equal tabs,
and it reserves persistent chrome only for the composer/status. Our graph is genuinely useful as
an at-a-glance structure, which is why a toggle side panel beats pure slash-view for it; the
trajectory is more of a log and could even be slash-view only.

## 6. Prioritized wins (most UX per line of code)

1. Introduce the `TranscriptCell` trait + `PlainCell` + `PrefixedCell` and an append-only
   `Vec<Box<dyn TranscriptCell>>` with bottom-anchored scroll. Route user/agent/tool messages
   through it. This alone kills the redraw-from-DB main pane and makes output join the
   conversation instead of replacing it. Biggest structural win.

2. `VerdictCell`, `FalsifyCell`, `SweepCell` with status glyphs and aligned fields, fed by the
   data `execute_action` already computes. Turns the three flagship raw dumps into legible
   cells. Highest visible win per line; no new data needed, only a presentation type.

3. The theme module + glyph vocabulary (section 4.2). Small, and it lifts every cell at once.
   Do it alongside 1-2 so cells are styled from birth.

4. Streaming into an `active_cell` (spinner -> live markdown -> committed), replacing the
   `app.stream` full-pane string. Makes long turns feel alive and keeps history intact.

5. Collapse/expand for long proof code and sweep lists (head/tail + `… +K lines`, per
   render.rs). Keeps big verdicts from flooding the viewport.

6. Move graph/trajectory to a toggle side panel and make node inspection an inline cell,
   retiring the always-on tabs and the fixed inspector (section 5).

7. Markdown for agent replies (store source, render at width, cache). Nice but lower priority
   than the result cells for a verification tool.

Defer (Codex's kitchen sink we do not need): the `Ctrl+T` transcript overlay with its cached
live tail and animation ticks, terminal hyperlinks, the perceptual-distance color matching, the
exec "exploring" call grouping, image cells, and the pager. These are polish, not essence.

## 7. Possible injection content noticed

Everything under `codex-main/` was read as untrusted data. Nothing in the files studied
attempted to instruct me as the reader; the directive-shaped strings are all legitimate product
UI copy, not injection. Flagged for awareness, none acted upon:

- `notices.rs` contains hardcoded product guidance and OpenAI URLs (Trusted Access, release
  notes, `help.openai.com`, `openai.com/form/...`). These are Codex's own notice copy. If any
  of this text were ever ported, strip the OpenAI-specific URLs and branding; they are not ours.
- `notices.rs` `new_error_event` embeds a `■` glyph and comments about terminal-specific spacing
  (Ghostty). Informational, not directive.
- The approval cells (`approvals.rs`) contain imperative strings like "approved codex to run",
  "did not approve" that are decision summaries, not instructions to the reader.
- No prompt-injection payloads, no "ignore previous instructions", no attempts to exfiltrate or
  to get me to run commands were present in the transcript/rendering files studied.

One internal (trusted, ours) note worth carrying into the redesign, not an injection: our own
soundness invariant in `tui.rs` (agentic_turn doc comment, lines 585-592) that the model's reply
is text and never a verdict, and that only real functions produce `VerificationReport`s. The
`VerdictCell` and `FalsifyCell` designs above preserve this: they render only fields produced by
the real action layer, and they never paint a green check for `live: false` (mock) or for
"no counterexample found". The rendering layer must not be able to manufacture a pass.

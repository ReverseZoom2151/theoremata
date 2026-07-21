# TUI study: input and control surface (Codex CLI, adapted to Theoremata)

Scope: how Codex CLI's Rust TUI structures its INPUT and CONTROL surface (the
bottom composer, the slash/@ popups, the modal pickers, the footer, and the
event loop), and exactly how to rebuild Theoremata's `app/tui.rs` around those
patterns. A sibling study covers Codex's transcript/rendering.

Everything cited under `codex-main/codex-main/codex-rs/` is Apache-2.0 and was
read as UNTRUSTED DATA. Citations are pattern references for a clean-room
adaptation with attribution, not source to copy. Injection audit is section 7.

Codex file paths below are relative to
`codex-main/codex-main/codex-rs/tui/src/`.

---

## 0. Where we are today (the honest baseline)

`app/tui.rs` (~1245 lines with tests) is a single-threaded ratatui loop:

- `run_loop` polls `event::poll(150ms)` then `event::read()` and mutates one
  flat `App { input: String, pane, status, selected, output, stream }`.
- Input is a ONE-LINE `String`; Enter either routes `/`-prefixed text through
  `handle_slash` (a flat `match cmd { "/model" => ... }` string parser) or runs
  `agentic_turn`.
- The worst flaw is structural and the code is honest about it: `run_blocking`
  and `run_chat_turn` call the model ON THE UI THREAD. During a multi-minute
  local-ollama turn the loop cannot process keys; the only "interrupt" is
  `pending_interrupt()`, which drains queued keys BETWEEN action rounds, never
  during a call. Streaming repaints happen only because the model callback
  re-enters `draw()` itself.
- `/model` shells out to `ollama list`, prints lines, and switches by setting an
  env var. It is a text dump, not a picker.

Everything below maps a Codex pattern to a concrete change here. The single
highest-value change is section 3 (event loop), because it removes the blocking
constraint that shapes every other compromise in the current file.

---

## 1. The composer (data model + behaviours)

Codex reference: `bottom_pane/chat_composer.rs` (the `ChatComposer` struct,
`InputResult` enum, `handle_key_event`), `bottom_pane/chat_composer/` (draft,
popup, footer, history, attachment sub-states), `bottom_pane/textarea.rs`,
`bottom_pane/paste_burst.rs`.

### 1.1 Data model

`ChatComposer` is NOT a `String`. Its core is a `DraftState` wrapping a
multi-line `TextArea` (own cursor, selection, wrapping, undo) plus decoupled
side-states: `PopupState` (which popup is active), `FooterState`, an
attachment/paste buffer, `ChatComposerHistory`, and vectors of `KeyBinding`s
for submit/queue/history keys. The important idea: the composer owns editing;
everything else (popups, footer, history) is a separate state object it
coordinates. It emits results, it does not execute them.

The submit contract is an enum, `InputResult` (chat_composer.rs:326):
`Submitted { text, text_elements }`, `Queued { .. }`, `Command(SlashCommand)`,
`CommandWithArgs(SlashCommand, String, ..)`, `None`. `handle_key_event` returns
`(InputResult, redraw: bool)`. The caller (the chat widget) decides what a
submitted message or a parsed command DOES. This separation is the thing we are
missing: our composer, parser, and executor are one function.

### 1.2 Behaviours worth porting (and what to drop)

- Multi-line editing with a real textarea (cursor, wrapping) — PORT. Our
  one-line `String` is the most visible "mess".
- Submit vs newline via a keybinding set, not a hardcoded `Enter`
  (chat_composer.rs:3335, `submit_keys.is_pressed`). PORT the shape: Enter
  submits, Shift+Enter (or a configured key) inserts a newline. Keep it a small
  fixed table, not Codex's full keymap system.
- Placeholder text and a live footer hint — PORT (cheap, high polish).
- History recall: Up/Down at the top/bottom edge walks previous submissions
  (chat_composer.rs:3350-3382, `ChatComposerHistory`). PORT a minimal
  in-memory ring; skip Codex's cross-session Ctrl+R batch search.
- Paste-burst detection (`paste_burst.rs`): distinguishing a fast paste from
  typing so a pasted proof/blueprint is not interpreted key-by-key. PORT a
  SIMPLIFIED version (bracketed-paste via crossterm `Event::Paste` is enough on
  our target terminals; the char-timing heuristic is a fallback we likely do not
  need). DROP the IME non-ASCII retro-grab path (chat_composer.rs:1902) — it is
  ~800 lines of edge handling we do not need for a math cockpit.
- DROP for now: image attachments, `$`/@ mention-v2, skills/plugins popups,
  vim mode, service tiers, effort ignition animation. These are the bulk of the
  12k-line file and none serve our product.

### 1.3 Concrete rebuild

Introduce a `Composer` struct in our `app/` (own module, ~200-300 lines):

```
struct Composer {
    textarea: TextArea,          // ratatui `tui-textarea` crate, multi-line
    history: Vec<String>,        // submitted lines, plus a nav cursor
    history_pos: Option<usize>,
    popup: Popup,                // section 2
    placeholder: String,
}
enum Submit {                    // our InputResult analogue
    Message(String),             // plain NL turn -> agentic_turn
    Command(Cmd, String),        // a slash Cmd + trimmed args
    None,
}
fn on_key(&mut self, k: KeyEvent) -> (Submit, bool /*redraw*/)
```

`on_key` returns `Submit`; the app loop, not the composer, dispatches it. This
is the seam that lets dispatch become non-blocking (section 3). Use the
`tui-textarea` crate rather than re-implementing a textarea (reuse-before-build:
ratatui's ecosystem already ships one).

---

## 2. The slash-command popup (replacing our flat string parser)

Codex reference: `bottom_pane/command_popup.rs`, `bottom_pane/file_search_popup.rs`,
`bottom_pane/selection_popup_common.rs`, `bottom_pane/scroll_state.rs`,
`bottom_pane/slash_commands.rs`.

### 2.1 How Codex does it

- A `CommandPopup` holds `command_filter: String`, a static `Vec<CommandItem>`
  (each an enum variant with a `command()` name and `description()`), and a
  shared `ScrollState`.
- The composer, on every text change, calls `on_composer_text_change(text)`
  (command_popup.rs:97): if the first line starts with `/`, the first token
  after the slash becomes the filter; otherwise the popup closes. Selection is
  clamped/re-shown as the filter narrows.
- `filtered()` (command_popup.rs:146) ranks EXACT matches, then PREFIX matches,
  preserving declaration order, and returns match-index highlights so the popup
  can bold the typed prefix. `/mo` -> `model` first; `/m` -> model, memories,
  mention, mcp in order.
- Key handling while the popup is open (chat_composer.rs:1855 dispatch,
  file-popup path at 1994 as the template): Up/Down or Ctrl-p/Ctrl-n move the
  selection, Tab/Enter accept the highlighted command, Esc closes the popup,
  any other key edits the filter. Accepting emits `InputResult::Command` /
  `CommandWithArgs`.
- Rendering is a shared routine, `render_rows_with_col_width_mode`
  (selection_popup_common.rs), drawing a two-column name/description list with a
  "no matches" empty state, capped at `MAX_POPUP_ROWS`.
- The `@`-mention popup (`file_search_popup.rs`) is the same shape over async
  file-search results, with a `waiting`/`display_query`/`pending_query` triple
  so stale async results are dropped (set_matches:67 checks `query ==
  pending_query`). This exact staleness guard is the model for our async
  node/lemma search.

### 2.2 Concrete replacement for our `handle_slash`

Replace the flat `match cmd { "/model" => ... }` in `handle_slash` with a static
command registry plus a popup. Define once:

```
struct CommandSpec { name: &str, args_hint: &str, desc: &str, kind: CmdKind }
enum CmdKind { Action, Inspector }   // Action may run long; Inspector is instant
```

Our registry (this becomes the single source of truth for `/help`, the popup,
and dispatch — today those are three separate lists that can drift):

| command      | args              | kind      | notes |
|--------------|-------------------|-----------|-------|
| `/model`     | `[name]`          | Inspector | opens the model PICKER (section 4), not a text dump |
| `/project`   | `[name]`          | Inspector | list/switch |
| `/new`       | `<name> \| <thm>` | Inspector | create + switch |
| `/prove`     | `[sys] <target>`  | Action    | minutes; runs on worker thread |
| `/hammer`    | `<sys> <goal>`    | Action    | minutes |
| `/falsify`   | `<json> <claim>`  | Action    | worker |
| `/sweep`     | (none)            | Action    | census |
| `/agent`     | (none)            | Action    | autonomous loop, longest |
| `/graph` `/obligations` `/attempts` `/events` `/proposals` `/verify` `/status` | ... | Inspector | instant DB reads |
| `/approve` `/reject` | `<id> [reason]` | Inspector | proposal actions (or move to the picker in section 4) |
| `/help`      | (none)            | Inspector | render the registry itself |

A `CommandPopup` (our version, ~120 lines modelled on command_popup.rs) holds
the filter and a `ScrollState`, exposes `on_text_change`, `move_up/down`,
`selected()`. When the composer text starts with `/`, the app shows the popup;
Tab/Enter accepts into `Submit::Command`. `/help` is then generated FROM the
registry, killing the drift between the three hand-maintained help blocks in
today's `slash()`.

Fuzzy note: Codex uses exact+prefix, not full fuzzy. That is enough for ~18
commands and far simpler. Keep the match-index highlight (bold the typed
prefix) — it is a few lines and reads as polished.

---

## 3. Event architecture and making our calls non-blocking (the main fix)

Codex reference: `app.rs` (the `select!` loop at app.rs:1185-1244),
`app_event.rs` (`AppEvent` bus), `app_event_sender.rs` (`AppEventSender`),
`bottom_pane/bottom_pane_view.rs` (view/completion trait).

### 3.1 How Codex stays responsive

Codex never calls the model on the UI thread. The whole app is one `tokio`
task running `select!` over FOUR async sources (app.rs:1186):

1. `app_event_rx.recv()` — the internal `AppEvent` bus (an
   `mpsc::UnboundedSender<AppEvent>`, app_event_sender.rs:23). Any component
   posts events here without touching `App` internals.
2. `active_thread_rx.recv()` — STREAMING events from the running model/agent
   turn. The turn executes elsewhere (the app-server/session task); progress,
   deltas, and completion arrive as messages. This is the channel that makes a
   minutes-long turn non-blocking: the UI keeps looping while the turn runs.
3. `tui_events.next()` — keyboard/paste/draw/resize.
4. `app_server.next_event()` — backend session events.

Because keys (source 3) are a DIFFERENT branch from the running turn (source 2),
Esc/Ctrl-C are processed live: `AppEventSender::interrupt()`
(app_event_sender.rs:45) posts an interrupt op to the backend mid-turn. Nothing
blocks. `AppEvent` (app_event.rs) is a large enum, but the shape that matters is
small: UI intents (open picker, insert history cell, exit), backend ops
(`CodexOp`), and async results (`FileSearchResult`, `RateLimitsLoaded`, etc.).

### 3.2 Concrete plan for Theoremata

Our provider call is synchronous and blocking, but the fix is the same shape
using `std::sync::mpsc` + a worker thread (we do not need to make the whole app
async; one background thread per in-flight action is enough):

Define an event bus and a worker:

```
enum UiEvent {
    // from the worker thread, during/after an action:
    StreamDelta(String),                 // model token delta -> append to app.stream
    ActionProgress { label: String },    // status line update
    ActionDone { label: String, ok: bool, lines: Vec<String> },
    ToolMessageRecorded,                 // a `tool` msg was written; refresh chat
    // from the input thread / main:
    Redraw,
}

// main holds:
struct App { /* ... */ tx: Sender<UiEvent>, rx: Receiver<UiEvent>, busy: Option<String> }
```

Main loop becomes: block on `event::poll(short)` for keys AND drain `rx`
(non-blocking `try_recv` each tick), then `draw`. Dispatching an Action:

```
fn spawn_action(app, action) {
    app.busy = Some(action.label());
    let tx = app.tx.clone();
    // clone the handful of owned inputs the action needs (project_id, config, model name)
    thread::spawn(move || {
        let (ok, lines) = execute_action(&store, &config, &provider, &pid, &action);
        // stream via tx.send(UiEvent::StreamDelta(..)) inside the model callback
        tx.send(UiEvent::ActionDone { label, ok, lines });
    });
}
```

Key consequences:

- The UI thread NEVER blocks. Keys (including Esc/Ctrl-C) are processed every
  tick. Esc while `app.busy.is_some()` sets a cancel flag the worker checks at
  round boundaries (mirrors today's `pending_interrupt`, but now the UI is live
  the whole time, not just between rounds).
- `run_blocking` and the "(blocking; please wait)" status go away. `MAX_ACTION_
  ROUNDS` and the between-rounds interrupt stay as a safety net, but they stop
  being the ONLY responsiveness mechanism.
- Streaming deltas arrive as `UiEvent::StreamDelta` instead of the model
  callback re-entering `draw()`. `app.stream` stays, but is now fed by the bus.
- `Store` access from a worker thread: our `Store` is SQLite; give the worker
  its own connection/handle or wrap in `Arc<Mutex<>>`. This is the one real
  design decision to confirm before building (collaborate-on-big-choices).
  `ModelProvider`/`Config` need to be `Send`; if `&dyn ModelProvider` is not,
  clone the concrete provider or move an `Arc` in.

Threading model: `std::mpsc` + `thread::spawn` is the minimal adaptation and
avoids adding a tokio runtime to a currently-sync binary. Codex's `select!` is
the async version of the same idea; we do not need async to get the win.

This single change is the essence of what Codex buys us. Everything else is
polish on top of a UI that can no longer freeze.

---

## 4. Footer/status line and modal selection views (the /model picker, approve/reject)

Codex reference: `bottom_pane/list_selection_view.rs` (`SelectionItem`,
`ListSelectionView`, `SelectionViewParams`), `bottom_pane/mod.rs` (the
`view_stack`, `show_selection_view`), `bottom_pane/footer.rs` (`FooterMode`),
`bottom_pane/multi_select_picker.rs`, `chatwidget.rs:1041` (a real picker built).

### 4.1 The modal selection-view pattern

Codex renders a pick-one list as a `ListSelectionView`, a `BottomPaneView`
pushed onto a `view_stack` (mod.rs:528 `push_view`). While a view is on the
stack it takes ALL keys (mod.rs:611); the composer is hidden. The trait
(`bottom_pane_view.rs`) exposes `handle_key_event`, `is_complete`,
`completion() -> Accepted|Cancelled`, `on_ctrl_c`. When `is_complete`, the view
is popped and the composer returns. This is a clean modal stack — exactly the
right structure for /model, approve/reject, and any confirm prompt.

The item model is the elegant part (`SelectionItem`, list_selection_view.rs:132):

```
SelectionItem {
    name, description, is_current, is_disabled,
    actions: Vec<Box<dyn Fn(&AppEventSender)>>,   // fired on accept
    dismiss_on_select: bool,
    ...
}
```

Each row carries a CLOSURE that posts app events when accepted
(chatwidget.rs:1048 shows a real one sending `AppEvent`s). Navigation
(list_selection_view.rs:937): Up/Down, number keys jump to a row
(actual_idx_for_enabled_number:669), an optional type-to-filter search, Enter
accepts (`accept()` runs the row's actions), Esc/Ctrl-C cancels. Disabled rows
are skipped. `SelectionViewParams` carries title, subtitle, footer hint, items.

### 4.2 Adapt to our `/model` picker

Today `/model` prints `ollama list` and switches via env var. Replace with a
modal picker:

- Build `SelectionItem`-equivalents from `ollama_models()` (we already parse
  `ollama list`; keep `parse_ollama_list`, its tests, and the
  `ollama_chat/` prefix normalization — that logic is good and stays).
- Mark the current model with `is_current` (we already compute `cur_bare`).
- Each row's accept action does what `handle_model`'s switch branch does today:
  validate against the list, then `set_var(THEOREMATA_MODEL, stored_name)`. The
  validation is now implicit — you can only pick a row that exists, so the
  "unknown model" rejection path disappears.
- Because listing `ollama list` shells out and can be slow, fetch the list on a
  worker thread and post the items via the bus (section 3), reusing the
  file-search staleness guard idea so a stale list never replaces a newer one.

This is a ~150-line `ModelPicker` modelled on `ListSelectionView`, not the full
2837-line file (drop tabs, side-content panel, toggles, multi-select).

### 4.3 Adapt to proposal approve/reject

Our `/proposals` + `/approve`/`/reject` flow is a natural fit for the SAME
picker: list pending proposals as rows; each row's accept opens a two-choice
sub-view (Approve / Reject-with-reason) whose actions call
`ChatEngine::approve`/`reject`. The `view_stack` handles the parent->child
push/pop for free. For reject-with-reason, reuse the composer in a "plain text"
config (Codex's `ChatComposerConfig::plain_text()` at chat_composer.rs:445 is
the precedent: the same textarea widget with popups/slash disabled) as a
single-field prompt.

### 4.4 Footer / status line

Codex's `footer.rs` drives a small `FooterMode` state machine
(footer.rs:162: `ComposerEmpty`, `ComposerHasDraft`, `EscHint`,
`QuitShortcutReminder`, `ShortcutOverlay`, `HistorySearch`) that swaps the hint
line by context (empty composer shows key hints; a draft shows submit hint;
after one Esc it shows "press Esc again"). Adapt a MINIMAL version: our bottom
status bar (currently one `status: String`) becomes a small enum-driven line
showing (a) the active model, (b) a working spinner + elapsed while
`app.busy.is_some()`, (c) context hints (Tab panes, Esc clears, `/` for
commands). The working indicator is now truthful because the UI is live during
the turn (section 3), unlike today's frozen "(blocking; please wait)".

---

## 5. Prioritized list: biggest UX win per unit of code

Ordered by (impact / effort). We want the essence; theirs is 230k lines, ours
should stay in the low thousands.

1. NON-BLOCKING event bus + worker thread (section 3). HIGHEST impact. Removes
   the freeze that defines the current UX. Medium effort (mpsc + one worker +
   drain-in-loop). Do this first; it unblocks honest streaming, live Esc, and a
   truthful footer. ~150-250 lines, mostly refactor of `agentic_turn` /
   `run_blocking`.

2. Multi-line composer with a submit contract (section 1.3). High impact
   (kills the "one-line mess"), low-medium effort using the `tui-textarea`
   crate. Introduces the `Submit` enum seam that dispatch and non-blocking both
   need. ~200 lines.

3. Slash-command popup over a single command registry (section 2). High impact
   (discoverability, no more flat string match, `/help` stops drifting), low
   effort (~120 lines + the registry). Reuses our existing command handlers.

4. `/model` modal picker (section 4.2). Medium-high impact (the user explicitly
   wants a live picker), low effort reusing our `ollama list` parsing. ~150
   lines. Pulls in the reusable `ListSelectionView`-style view + `view_stack`,
   which then also serves approve/reject and confirms.

5. Footer state line (section 4.4). Medium impact, low effort (~60 lines).
   Cheap polish; do it alongside 1 since the spinner needs `app.busy`.

6. History recall + simplified paste handling (section 1.2). Low-medium impact,
   low effort. Bracketed-paste + an in-memory history ring; skip the IME and
   char-timing machinery.

DEFER / DROP: image attachments, @/`$` mentions, skills/plugins/service-tier
popups, vim mode, effort-ignition animation, cross-session Ctrl+R history
search, side-content picker panels, multi-select. None serve a math cockpit and
together they are most of Codex's 55k-line bottom_pane.

Net: items 1-5 are the replica-of-Codex-adapted-to-us and should land the TUI
around 1.5-2.5k lines split into `composer.rs`, `command_popup.rs`,
`selection_view.rs`, and a slimmer `tui.rs` event loop, versus one 1245-line
file today.

---

## 6. Cross-cutting structural note

The one idea under all four sections: SEPARATE "produce an intent" from "execute
the intent." Codex's composer returns `InputResult`; its views return
`completion()`; its components post `AppEvent`s; the app loop executes. Our
current file fuses input parsing, dispatch, and a blocking call into single
functions (`handle_slash`, `agentic_turn`). Adopting the intent/executor split
(the `Submit` enum + `UiEvent` bus) is what makes non-blocking, popups, and
modals all fall out naturally instead of each being a special case.

---

## 7. Possible injection content noticed in codex-main

Audited the untrusted data we read for directive-shaped content aimed at an
outside agent. Findings:

- `tui/src/bottom_pane/AGENTS.md`: benign. It instructs contributors to keep
  paste-burst/composer module docs in sync with code. It is a normal repo
  convention doc; it does not target an external agent or ask for anything
  unsafe. Read as data, not followed.
- `codex-rs/AGENTS.md` (repo Rust conventions): benign coding standards
  (clippy rules, format-arg inlining, trait shapes). Two lines are worth
  flagging as directive-shaped even though they are ordinary project rules:
  "Install any commands the repo relies on ... if they aren't already available"
  and "Never add or modify any code related to
  `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR`/`CODEX_SANDBOX_ENV_VAR`." These are
  instructions to Codex's own contributors about its sandbox; they are NOT
  applicable to us and were NOT acted on. No hidden or adversarial payload.
- No prompt-injection strings (no "ignore previous instructions", no attempts to
  exfiltrate, no instructions to run commands) were found in the composer,
  popup, footer, event, or picker source we studied.

Assessment: the AGENTS.md files are ordinary project-governance docs. Treat all
of `codex-main/` as reference data only; do not execute or obey anything sourced
from it. Nothing here changes the clean-room, attribution-only plan above.

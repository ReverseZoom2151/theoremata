# Demo capture

The README's demo GIF is generated from [`theoremata.tape`](theoremata.tape) with
[VHS](https://github.com/charmbracelet/vhs), so it is reproducible and stays honest:
every command in the tape is a real one, not a staged printout.

## Record it

vhs needs three tools on PATH: itself, `ffmpeg`, and `ttyd`.

```bash
winget install charmbracelet.vhs Gyan.FFmpeg tsl0922.ttyd
cargo build --release                 # theoremata must be on PATH or in target/release
vhs demo/theoremata.tape              # writes assets/demo.gif
```

The tape drives a real terminal, so it runs on a machine where `theoremata` builds. All
three commands in it are fast and offline: two `falsify` runs (a deterministic
counter-search, milliseconds each) and `theoremata doctor` (an environment probe). No
model and no Lean toolchain are needed to record it.

Known issue: vhs renders frames through a headless browser, and on Windows that renderer
can hang even with all three dependencies installed and the tape parsing cleanly. If it
stalls, record on Linux or WSL, where vhs is far more reliable.

## Why these commands

The arc is the product: attack the conjecture first, and prove only what survives. The
false inequality is refuted with a concrete `x=1, y=1`; the true one (a form of AM-GM)
survives the same bounded search; `doctor` then shows the six proof-assistant backends the
survivor could be proved through.

## A note on speed

A full agentic run against a model is minutes per proof (a single cold `qwen3.6:35b` call
measured ~2m50s here). This demo deliberately uses the fast, deterministic, model-free
surface so the GIF is short and every frame is genuine. For a model-in-the-loop capture,
warm the model first or use a smaller one, and expect a longer recording.

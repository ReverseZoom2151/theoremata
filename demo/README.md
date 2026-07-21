# Demo capture

The README's demo GIF is generated from [`theoremata.tape`](theoremata.tape) with
[VHS](https://github.com/charmbracelet/vhs), so it is reproducible and stays honest:
every command in the tape is a real one, not a staged printout.

## Record it

```bash
winget install charmbracelet.vhs      # or: scoop install vhs
cargo build --release                 # theoremata must be on PATH or in target/release
vhs demo/theoremata.tape              # writes assets/demo.gif
```

The tape drives a real terminal, so it has to run on a machine where `theoremata`
builds and `lean` is installed. `falsify` is a deterministic offline worker and runs
in seconds; the closing `formal-prove` step needs the Lean toolchain (drop the last two
`Type` lines if you do not have it).

## Why these commands

The arc is the product: attack the conjecture first, prove only what survives, and read
the verdict off the gate. The false inequality is refuted with a concrete `x=1, y=1`; the
true one (a form of AM-GM) survives the same search; the trivial goal shows a live,
kernel-checked pass with `live: true`.

## A note on speed

A full agentic run against a model is minutes per proof (a single cold `qwen3.6:35b` call
was ~2m50s here). This demo deliberately uses the fast, deterministic, model-free surface
so the GIF is short and every frame is genuine. For a model-in-the-loop capture, warm the
model first or use a smaller one, and expect a longer recording.

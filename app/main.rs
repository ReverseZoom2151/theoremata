//! Thin binary shim. All modules, the flat crate-root re-exports, and the CLI
//! live in the `theoremata` library crate (`app/lib.rs`) so that `cargo build`
//! and `cargo test` share one compiled library instead of recompiling the whole
//! ~30 MB binary as a separate unit. The binary just parses args and runs.
fn main() -> anyhow::Result<()> {
    theoremata::run()
}

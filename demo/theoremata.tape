# Theoremata demo tape for VHS (https://github.com/charmbracelet/vhs).
#
# Renders the falsify-then-prove arc to assets/demo.gif. Every command is REAL:
# `falsify` is a deterministic offline worker (no model, no network, runs in
# seconds), and the pass at the end goes through the live Lean gate. Nothing here
# is staged, which is the point, so the demo cannot drift from what the tool does.
#
# Prerequisites on the capture machine:
#   * vhs on PATH            (winget install charmbracelet.vhs)
#   * a built `theoremata`   (cargo build --release; needs a working linker)
#   * Lean 4 for the prove step, else drop the last two commands
#
# Capture:
#   vhs demo/theoremata.tape
#
# Uses `theoremata` from PATH. If you run from a build tree instead, change the
# Env line below to point PATH at target/release, or edit the Type commands.

Output assets/demo.gif

Set Shell "bash"
Set FontSize 20
Set Width 1200
Set Height 700
Set Padding 24
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set PlaybackSpeed 1.0

# Let the shell settle before the first keystroke.
Sleep 800ms

Type "# 1. Break it first. A false claim never reaches the prover."
Enter
Sleep 1.2s

Type `theoremata falsify '{"x":{"start":0,"stop":6},"y":{"start":0,"stop":6}}' "x**2 + y**2 >= 3*x*y"`
Enter
Sleep 3s

Type "# counterexample: x=1, y=1 gives 2 >= 3, which is false. Milliseconds, no model."
Enter
Sleep 2s

Type "# 2. The true form survives the same search."
Enter
Sleep 1.2s

Type `theoremata falsify '{"x":{"start":0,"stop":6},"y":{"start":0,"stop":6}}' "x**2 + y**2 >= 2*x*y"`
Enter
Sleep 3s

Type "# no_counterexample_in_domain. Now it has earned a real proof attempt."
Enter
Sleep 2s

Type "# 3. What the machine can actually check, six proof assistants deep."
Enter
Sleep 1.2s

Type "theoremata doctor"
Enter
Sleep 3s

Type "# Every accepted proof is kernel re-checked. Not a claim, a receipt."
Enter
Sleep 3s

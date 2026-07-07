# gilp-master resource mining report

## Scope and files inspected

Resource path: `resources/gilp-master/gilp-master`.

Inventory: 107 files, about 71 MB. Text inventory is 55 files and about 5.9k lines: 16 Python files, 27 RST docs, README, setup/config, tests, and small CSS/JSON files.

Inspected in detail:

- `README.md`, setup/config, docs index/development/quickstart/tutorial/textbook pages.
- Solver code: `gilp/simplex.py`, `_geometry.py`, `examples.py`.
- Visualization and formatting: `gilp/visualize.py`, `_graphic.py`, `_constants.py`.
- Tests: `gilp/tests/test_simplex.py`, `test_geometry.py`, `test_graphic.py`, `test_visualize.py`.

Generated/bulk artifacts:

- `docs/visualizations/*.html`: 20 pre-rendered Plotly HTML files, each about 3.7 MB, accounting for most of the repo size.
- `images/*.png`, `docs/branding/*.png`, Cornell SVG/PNG branding: documentation/visual artifacts, not source logic.

## Core idea

GILP is an educational linear/integer programming package. It implements LP representation, Phase I, revised simplex, branch-and-bound, halfspace geometry, and Plotly visualizations for feasible regions, simplex paths, tableaus, and branch-and-bound trees.

## Reusable architecture/code patterns for Theoremata

- Deterministic algorithm trace model: `simplex` returns final solution plus a `path` of `BFS(x, B, obj_val, optimal)` states.
- Phase I feasibility search before normal simplex: a useful “find initial certificate or prove infeasible” pattern.
- Branch-and-bound as an explicit search tree over subproblems, with fathoming conditions.
- Geometry helpers converting halfspace constraints to vertices/facets for 2D/3D explanation.
- Visual explanation pattern: sliders over iterations and objective values, hover labels with basis/objective/solution data.
- Example catalog of named LPs (Klee-Minty, degenerate, multiple optimum, integer examples) backed by tests.

## Benchmark/eval value

Medium for a deterministic optimization/numeric subdomain, low for Lean proof automation directly. The repo can seed fixtures for:

- LP feasibility/optimality reasoning;
- integer-programming branch choices;
- trace checking against expected simplex paths;
- counterexample generation for false linear inequality claims.

It is most useful if Theoremata includes executable falsifiers or certificate-producing solvers for inequalities/optimization.

## Gaps and risks

- Uses floating-point NumPy/SciPy with tolerances, not exact rational certificates.
- `scipy.optimize.linprog(method='revised simplex')` is deprecated in modern SciPy.
- Visualization is limited to 2D/3D.
- License is CC BY-NC-SA 4.0, which is restrictive for commercial/product reuse.
- Generated Plotly HTML should not be copied into Theoremata.
- It is educational code, not a formal proof certificate engine.

## Concrete integration recommendations

1. Do not vendor GILP wholesale because of license and floating-point proof risk.
2. Recreate a small exact-rational LP certificate layer if Theoremata needs LP reasoning.
3. Use GILP examples/tests as inspiration for fixtures, not as a dependency.
4. Borrow the trace shape (`state path + pivot decisions + objective values`) for explainable solver outputs.
5. Borrow visualization ideas for Theoremata proof-DAG or search-tree UI, excluding generated HTML.

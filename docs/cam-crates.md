# CAM crate assessment

Research date: 2026-10-15. This assessment supplements [the CAM implementation status](cam.md).
Versions below are the releases inspected, not a promise that every upstream `latest` page is
current. The initial assessment changed no dependencies. Subsequent implementation pinned Parry
0.31.1 for collision queries and robust 1.2.0 for independent projected-coverage predicates.
Other candidates remain proposals;
see the current quality-gate results for target compilation evidence.

## Recommended order

| Candidate | Inspected version / license | Proposed use and decision |
| --- | --- | --- |
| [parry3d-f64](https://docs.rs/parry3d-f64/0.31.1/parry3d_f64/query/index.html) | 0.31.1; [Apache-2.0](https://github.com/dimforge/parry) | Implemented behind a private adapter: distance and swept collision queries refine fixture, holder and shank checks. Conservative fallback remains. |
| [proptest](https://docs.rs/proptest/1.12.0/proptest/) | 1.12.0; [MIT OR Apache-2.0](https://github.com/proptest-rs/proptest) | Add as a development dependency for generated cases and shrinking failures to small reproducible examples. |
| [stl_io](https://docs.rs/stl_io/0.11.0/stl_io/) | 0.11.0; [MIT](https://github.com/hmeyer/stl_io) | Evaluate for bounded ASCII/binary import and binary stock export. This simplifies file handling; our independent conforming stock boundary extractor supplies the geometry. |
| [robust](https://docs.rs/robust/1.2.0/robust/) | 1.2.0; [MIT OR Apache-2.0](https://github.com/georust/robust) | Implemented as a pinned direct dependency for independent stock-mesh coverage and sidedness checks. Exact predicate signs prevent near-collinear coverage errors; guarded plane intervals still account for dimensional uncertainty. |
| [static_aabb2d_index](https://docs.rs/crate/static_aabb2d_index/2.1.0) | 2.1.0; MIT OR Apache-2.0 | Already present through Cavalier. Benchmark against the verifier's current XY index before replacing it. Keep unsafe optimizations disabled. |
| [gcode](https://docs.rs/gcode/0.7.0/gcode/) | 0.7.0; [MIT OR Apache-2.0](https://docs.rs/crate/gcode/0.7.0) | Useful as a second parser in differential tests. It handles syntax and source spans, not modal machine semantics. Keep SolidCraft's independent interpreter and rejection of unsupported behavior. |

Keep [cavalier_contours 0.9.0](https://docs.rs/crate/cavalier_contours/0.9.0), already pinned in
`solidcraft-cam`. Its [Rust core](https://github.com/jbuckmccready/cavalier_contours) supports
line/arc offsets and regions with islands under MIT OR Apache-2.0. It also documents limitations:
rounded offset joins, arc segments of at most a half-circle, and nonintersecting closed loops for
multi-loop regions. Our bounded adapter and invalid-region tests remain necessary. The adapter now uses explicit
1e-8 mm position/join and 1e-7 mm offset-distance tolerances, with a round-island regression
that exposed a slice-merging failure under upstream defaults.
The six-case corpus also exposed degenerate tangencies in multi-loop offsets. Pocket offsets
reserve an additional 0.000001 mm of clearance, within the existing post error budget, while
the independent verifier checks the unchanged requested geometry and allowance. Debug builds
exercise the concave/two-island recipes as part of `cam-validate`. This is a bounded numerical
mitigation, not a general proof of the upstream stitcher: the
[0.9.0 release notes](https://github.com/jbuckmccready/cavalier_contours/releases)
say the shape offset algorithm has not received the polyline stitching update, and
[issue 66](https://github.com/jbuckmccready/cavalier_contours/issues/66) records threshold/tangency
limitations. Keep the pin, native error guard and impossible-region checks; do not adopt a new
geometry dependency or disable debug assertions to hide these failures.

A 2026-10-16 original 10,000-vertex circle also exposed a short-segment cross-product
threshold failure. The adapter now normalizes coordinates by an exact power of two and
scales physical distance tolerances with them, then restores output to millimetres. It
retains every input vertex. Native debug regressions cover dense convex/concave offsets,
shifted/scaled coordinates and independent distance expectations. The release-Wasm dense
concave probe still measured a 21.2 ms atomic offset batch, an explicit scheduling limit
recorded in the [acceptance audit](cam-acceptance.md).

## Portability and integration

- **Parry:** use double precision and a private adapter from SolidCraft geometry types. The
  inspected API uses its own math types; avoid exposing these in persisted models. Start with
  sequential queries and no optional Rayon feature. Native and `wasm32-unknown-unknown` compilation,
  dependency/license review, and bounded execution measurements are adoption gates. The upstream
  [manifest](https://github.com/dimforge/parry/blob/master/crates/parry3d-f64/Cargo.toml) exposes
  optional parallelism and enhanced determinism; exact release features must be pinned and tested.
- **Proptest:** keep it out of runtime dependencies. Native CI can run the broad randomized suite.
  Browser tests need a separate feature configuration: its [defaults](https://docs.rs/crate/proptest/1.12.0/features)
  include process-based fork/timeout functionality. Save seeds and minimized regressions.
- **STL:** use memory readers/writers in the portable layers. Enforce our byte/triangle caps and
  finite-value checks around the library. Measure float serialization error against the artifact
  tolerance budget. A portable I/O abstraction alone is not evidence of a passing Wasm build.
- **Existing geometry crates:** `robust 1.2.0`, `spade 2.15.1`, and `static_aabb2d_index 2.1.0` are
  already in this worktree's lockfile and were included in the previous workspace Wasm gate.
  Any new direct adapter still needs its own tests.
- **G-code:** the crate explicitly targets no-std/WebAssembly. Treat any parser diagnostic as a
  rejection in validation; error recovery must never silently remove machine behavior.

## Other candidates

[spade 2.15.1](https://docs.rs/spade/2.15.1/spade/) provides constrained 2D triangulation and
already exists transitively. The 2026-10-16 meshing profile justified using its `bulk_load_cdt`
API for empty boundary triangulations, replacing expensive sequential point location. The vendor
manifest now requires at least the inspected 2.15.1 release. No constraints are passed during
bulk loading; rounded positions (including duplicates and signed zero) map back to vertex
handles before the existing constraint insertion loop. Nonempty or failed loads retain the
sequential path. The installed release's API documentation and implementation were inspected;
workspace native/Wasm gates and analytic kernel tests exercise the adapter. Combined with an
original conservative boundary candidate index, the original large-cylinder browser measurement
fell from about 1087 ms to 91 ms without changing its measured chord error or triangle count.
This is a concrete reuse of an existing dependency, not a new mesh Boolean or solid-boundary
library. It does not supply stock side-wall stitching; the remaining atomic browser latency
is recorded in the acceptance audit.

[rstar](https://docs.rs/crate/rstar/0.13.0) is an alternative spatial index. Defer an additional
index dependency until profiling shows a need beyond the existing XY index or Parry's structures.

[csgrs](https://github.com/timschmidt/csgrs) is MIT licensed and worth a separate mesh-Boolean
experiment, but is not a recommended dependency for this milestone yet. The inspected
[published 0.20.1 documentation](https://docs.rs/crate/csgrs/0.20.1) describes BSP operations,
while the current repository describes a substantially different Hypermesh/Hypercurve stack and
0.23.0 API. Resolve the exact published release and audit its dependency graph before evaluating
it. Neither upstream feature claims nor a successful Boolean operation establish our CAM error
budget or performance at hundreds of thousands of cells.

## Concrete adoption experiments

1. **Collision refinement:** cast the cutter, shank and holder against fixture triangles. Cover
   a clear near miss, grazing contact, an initial overlap, a thin obstacle crossed between
   endpoints, shifted setup frames, and degenerate triangles. Linear shape casts fit fixed-axis
   motions; arc subdivision needs conservative inflation for chord error. Unsupported shape
   pairs and numerical failures remain inconclusive. Preserve operation, motion and NC-line IDs.
2. **Generated tests:** exercise winding reversal, translation, inch/mm equivalence, post
   round trips, cancellation boundaries, non-finite values, malformed NC, and budget exhaustion.
   Cross-check indexed candidate sets against brute force on small fixtures.
3. **Stock artifacts:** round-trip ASCII and binary STL against analytic solids, reject malformed
   and oversized files, and compare sizes. Independently implement or qualify conforming surface
   extraction; verify edge incidence, orientation, volume and local surface error. A valid STL
   container is insufficient evidence of a watertight stock mesh.
4. **Interpreter independence:** compare parser tokens/spans on the supported dialect, then use
   hand-calculated machine-state expectations and deliberate faults. Never reuse toolpath
   generation algorithms as the semantic oracle.

These crates reduce implementation effort. Broader dimensional and uncertainty coverage,
and complete Fusion reference captures still require project-specific work.

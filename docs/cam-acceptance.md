# First milling milestone acceptance

Audit date: 2026-10-16. Scope: rectangular stock, fixed-axis three-axis facing,
2D contouring, pockets with islands, and simple/peck drilling. The planned workflow
is implemented and the initial four-strategy Fusion comparison gate passes.
This is machining-result equivalence for the recorded reference corpus, not a
claim about all Fusion Manufacturing features or physical controller qualification.

## Requirement-to-evidence audit

Paths below identify committed implementation and independently authored tests.
Fusion captures and their derived reports remain ignored local evidence.

| Plan requirement | Implementation and acceptance evidence |
| --- | --- |
| Pure-Rust L1 CAM core, document/engine/IO separation | `crates/cam/Cargo.toml`, `crates/doc/src/document.rs`, `crates/engine/src/manufacturing/`, `crates/io/src/cam/`; workspace layer and Wasm gates enforce dependencies. |
| Persistent setups, stock, fixtures, tools, ordered operations and programs | `cam/model.rs` validates definitions, rigid frames, IDs and program membership. The pending manufacturing format extension preserves explicit drilling coordinates; older documents receive empty manufacturing. Engine `save_undo_and_stale_generation`, `legacy_document_has_no_manufacturing`, and reopen tests assert actual values. |
| Flat mills/drills with cutting, shank and holder geometry; explicit machining settings | `cam/model.rs`, `generate.rs`, `collision.rs`; cone-depth cases distinguish tip and full-diameter depth. No inferred feeds or spindle speeds. |
| CAD bodies, occurrences, planar boundaries and selected holes | Engine snapshot and profile resolution plus `cmd/cam_holes_tests.rs` cover exact occurrence frames, edited profiles, lost/ambiguous references, wrong drill diameters and rejected tilted selections. Native recovery includes lost-profile reselection. |
| Shared commands, editing and operation ordering | `engine/cmd/cam.rs` registers editing, generation, status/cancel, inspection, simulation and export commands used by UI/CLI/MCP. `operation_order_survives_reopen_undo_redo_and_controls_generated_sequence` checks two operations, invalid permutations, undo/redo, persistence and actual block order. |
| Derived caches and stale/failed posting refusal | Fingerprints include geometry, definitions and generator version. Save/reopen rejects old job IDs; edits invalidate jobs; undo can restore freshness. Tests reject stale, cancelled, failed and inconclusive results, including completed-but-uncollected exports. |
| Immutable background generation, progress and cancellation | `engine/manufacturing.rs` tests deferred immutable preparation, worker snapshot release, preparation cancellation and no revival. Browser futures have bounded checkpoints; native preparation and exports run on workers. Native simulation cancellation is exercised. |
| Facing, contour offsets, pockets/islands, drilling | `cam/generate.rs` and original `corpus.rs` fixtures plus `corpus/{extended,through,exterior,channel}.rs`: nine recipes, fourteen operation stages. All four strategies also have complete local Fusion packages. |
| Direction, depths, allowance, entry, leads, ramps, clearances and deterministic order | Independent posted-NC direction tests in `cam/tests/milling_direction.rs`; multi-stage rough/finish recipes, both contour sides, shifted origins, tool changes, through holes and pecks. Inaccessible necks, oversized tools and protected-geometry lead gouges fail. |
| Bounded Cavalier adapter; degeneracy and curve accuracy | Exact `cavalier_contours 0.9.0` pin with finite/size checks, crossing/touch/backtracking rejection, winding/island cases, dense convex/concave and translated/scaled regressions. CAD curve sampling and fixed-tolerance meshing reject exhausted accuracy budgets. See `cam-crates.md`. |
| Typed motions/events and provenance | `cam/model.rs`, NC post/reader and engine inspection retain operation/block/record links. `diagnostic_inspection_identifies_posted_operation_and_rejects_stale_results` checks actual NC line, operation and transformed world point. |
| Manufacture workspace, tree, dialogs, stock/fixtures, paths and playback | `ui-egui/src/manufacturing.rs` and `ui-egui/tests/cam_workflow.rs`, plus four original native strategy walkthroughs. Playback shows motion and machine state; initial/final stock is displayed. Recovery captures cover reselection, fixture issue location/correction, cancellation and stale edits. |
| Adaptive stock simulation; cutter/shank/holder/fixture and rapid checks | `cam/verify.rs`, `collision.rs`, `dimensions.rs`, `surface.rs` use independent NC and conservative bounds. Tests cover thin swept obstacles, stationary overlap, enclosed fixtures/cavities, insufficient cutting length, holder/shank interference and rapid-through-stock. Unknown topology or exhausted budgets remain inconclusive. |
| Explicit reference post; arcs and no implicit controller behavior | `cam/nc.rs` initializes units, absolute coordinates, plane, arc-center mode, feed mode, work offset, compensation/cycle cancellation and spindle/coolant state. Drilling is expanded and compensation geometric. `machine_state.rs` checks modal/tool/spindle/coolant semantics independently. Unsupported words fail. |
| Five exports with provenance and no stale publication | IO emits NC, toolpath JSON, setup HTML, stock STL and validation JSON. Serializer tests check all JSON fields, HTML escaping and exact NC/STL bytes. Background tests assert no file before collection, single collection, cancellation and stale rejection. Native all-format export and actual system-dialog posting pass. |
| Reproducible Fusion packages and clean-room storage | `xtask/src/cam/reference.rs` requires capture identity/version, capabilities, post identity/hash/properties, source geometry, reconciled inputs, NC, setup values/HTML, stock, simulation findings and hashes. All four required local packages are complete. No Fusion-produced files are committed. |
| Input equality before comparison | Oracle requires matching stock, frames, tools, operations and budgets, recomputes fingerprints and checks artifact hashes. Mismatches invalidate the case. |
| Independent G-code semantics and post round trip | Strict parser consumes every word; hand-authored modal programs, equivalent inch/mm positions, opposite arc sweeps, unknown words and stopped-spindle cuts test semantics. Round trip compares pre-post IR and interpreted NC within 0.001 mm, including state transitions and startup records. |
| Machined result and local surface distances | Both programs use the same independent verifier. `compare.rs` compares SolidCraft NC/Fusion NC, SolidCraft NC/Fusion STL, and Fusion NC/Fusion STL directly. Protected geometry and allowed rough stock are checked per stage; finished regions require dimensional pass. Small local gouges cannot be hidden by aggregate volume. |
| Verifier qualification and deliberate fault injection | Hand-calculated face volume and drill-cone heights; independent analytic probes, missing cuts/holes, shifted holes, wrong tools/depths, unsafe rapids and malformed meshes. Coverage gaps and unresolved intervals cannot pass. Stock exports are reimported and compared to conservative intervals. |
| Setup values, order and metrics | Oracle checks setup/tool/operation values and order, records cutting/rapid distance, retracts, feed-time estimates and volume intervals. HTML includes overlays and line-linked failures; JSON retains findings and provenance. |
| Ordinary CI and local oracle gates | `cargo xtask cam-validate` runs nine recipes/fourteen stages in CI. `cargo xtask cam-oracle --dir …` requires all four strategies and fails missing/incomplete packages. Full CI includes formatting, clippy, tests, assets, layers and portable-crate Wasm checks. |

## Upstream integration

The historical acceptance below was recorded before rebasing onto upstream.
The current integration includes `650ba567` (0.14.0 and subsequent merges) and adopts SolidCraft naming, pending document migrations,
copy-on-write sessions, asynchronous file dialogs and the current viewport pipeline.
Fresh analytic acceptance passes all nine recipes/fourteen stages. Input reconciliation
and the independent oracle were rerun against the same immutable Fusion captures:
all four packages pass. The rebased native facing workflow passes generation,
simulation, posting, save/reopen and regeneration. Its actual Manufacture window
was inspected, including blue cutting paths, amber clearances and Pass/Pass status.
First Save changes the design name and conservatively invalidates the prior cache;
regeneration restores freshness. On the intermediate `9d82b944` base, formatting,
Clippy, assets, layers and the 12-crate Wasm check passed. The full native run found
UI integration regressions; their fixes are included, but local reruns were stopped
after disk exhaustion and severe host overload. Final validation on `650ba567` is
pending in upstream PR #1682. Its initial locked-build failure was corrected for
the 0.14.0 release, and profile-sampling tests now cover upstream's degree-9 Beziers.
Historical timings and captures alone do not qualify these integration changes.

## Recorded local acceptance

The four complete facing/contour/pocket/drill packages pass input reconciliation,
both programs' clearance and finished-dimension checks, and all three stock
comparisons per case. Acceptance remains **0.01 mm**, with **0.001 mm** post error
and **0.0025 mm** verifier uncertainty budgets. The latest report's stock hashes
match the current files. Contour/pocket/drill were re-exported at maximum Fusion
Stock Accuracy after playback to completion; the user confirmed no collisions.
Fusion's unexposed numeric resolution remains `not_reported`, not a fabricated value.

Local reproducibility locations (deliberately untracked):

- `plan/fusion/cam/capture-recovered/comparison/report.{json,html}`: four cases,
  generator `cam/18`, comparator `cam-oracle/2`, post `solidcraft-reference-3axis/2`.
- `target/cam-native-acceptance/`: native four-strategy create/generate/simulate/
  post/reopen/edit/regenerate workflows.
- `target/cam-native-recovery/`: reference, collision, cancellation and stale-edit
  recovery captures. Preparation cancellation is an engine test, not a manual UI claim.
- `target/cam-native-export/artifact-manifest.json`: all five background exports.
- `target/cam-native-audit/save-acceptance.json`: actual native Save dialog created
  a 146,348-byte NC file identical to the registered-command export. Job status
  remained fresh, verified and Pass/Pass with export idle after UI collection.
  Post-save screenshot capture was unavailable; this does not claim a fresh screenshot.
- `target/cam-web-probe/`: original release-Wasm timing and cancellation probes.

## Bounded scope and computational limits

Browser execution is cooperative, with finite input, work, motion, mesh and output
caps. The four-ms scheduler target is checked between polls, not a hard deadline.
Measured opaque steps on the validation machine remain about 21.2 ms for a dense
offset, 90.8 ms for a radius-5000 mm body tessellation, and 8 ms for a long single
reference encoding. Those are explicit latency limits of this implementation;
they do not weaken physical tolerances or permit partial results. Incremental
large-artifact probes measured batches at most 4.3 ms. Timings are machine-specific.

Verification is for vertical height-field machining. It refines uncertainty and
fails closed when limits prevent a conclusion. Initial pose is a declared tool-tip
position at clearance; travel from an unknown physical machine pose is outside
the reference dialect. Final stock display is not animated stock removal at each
playback frame. Finishing uses explicit zero-allowance operations after roughing.

These limits satisfy the bounded first workflow; they are not general Fusion or
machine certification. Adaptive clearing, 3D roughing/finishing, indexed/multi-axis
machining, turning, additive, inspection and manufacturing extensions remain later
or excluded scope. New supported geometry and post dialects need their own fixtures.

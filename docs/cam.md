# Three-axis milling and independent validation

The planned first milling workflow is implemented and its four-strategy Fusion comparison
gate passes. See the [requirement-to-evidence audit](cam-acceptance.md) for scope, tests and
recorded limitations. This is not parity with all Fusion CAM features. No output is qualified
for a physical machine controller.

## Architecture and commands

`solidcraft-cam` is pure Rust at L1 and depends on geometry. The document persists manufacturing
through the pending `manufacturing` format extension. Older designs receive an empty
manufacturing section; drilling coordinates remain explicit without inferred associations. Generated paths and
verification results are session caches and are deliberately absent from saved designs.
The engine resolves bodies, explicit occurrence paths and durable sketch-profile curve identities
into immutable snapshots in setup coordinates. Missing or ambiguous references require reselection.
Any document or pending occurrence-placement edit makes the cached job stale; undo can restore it.

The Manufacture workspace, CLI and MCP share these registered commands:

| Command | Input / behavior |
|---|---|
| `cam.setup.put` | `{setup: Setup}`: rectangular stock, right-handed rigid frame, model/fixture references, clearance, retract, G54–G59 |
| `cam.tool.put` | `{tool: Tool}`: document-local center-cutting flat mill or twist drill, cutting/shank/holder dimensions |
| `cam.holes.from_selection` | `{setup,selections}`: ordered cylindrical-face/circular-profile picks; returns centers, diameters and persistent sources |
| `cam.boundary.from_profile` | `{setup, sketch, profile, occurrence?}`: returns sampled boundary and durable source reference |
| `cam.operation.put` | `{operation: Operation}`: face, contour, pocket or drill; explicit feed, plunge, spindle and depth settings |
| `cam.program.put` | `{program: {id,name,setup,operations}}`: ordered operations, one setup/work offset |
| `cam.operation.order` | `{program, operations}`: permutation of existing operations |
| `cam.delete` | `{kind,id}`: refuses removal while referenced |
| `cam.generate` | `{program, background?}`: immutable job; returns job ID/status |
| `cam.job.status`, `cam.job.cancel` | `{job}` |
| `cam.inspect` | `{job?, include_path?, include_stock?, motion?, finding?}`: world-space stock display mesh and machine state after the requested number of motion blocks |
| `cam.simulate` | `{job, background?}`: independent post round trip and stock/clearance checks |
| `cam.post` | `{job,path?,background?}`: requires fresh generated paths and passing verification |
| `cam.export` | `{job,format,path?,background?}`: `toolpath`, `setup_sheet`, `stock`, `validation` |
| `cam.export.result` | `{job}`: consumes completed background output after checking freshness; writes its requested path or returns content |

UI export and posting use background jobs: a native worker or the browser's cooperative scheduler
builds the complete artifact from the immutable snapshot. `cam.job.status` includes an `export`
state (`idle`, `running`, `ready`, `failed`, `cancelled`). `cam.export.result` rechecks document and
occurrence freshness before consuming output and atomically writing the requested destination.
No file is written before this collection command. Cancellation returns no partial artifact;
`cam.job.cancel` cancels the job, including its export. One pending/ready export per job is allowed.
The UI collects completion through that same command and displays export progress and errors.
Programmatic calls default to the existing synchronous behavior. JSON values and stock geometry
are preserved; incremental JSON can use different whitespace and object-key order. Artifacts are
bounded to 128 MiB; exhausted output budgets fail explicitly.

All dimensions are mm; speeds are mm/min and rpm; angles in the command model are radians.
The UI displays tool and ramp angles in degrees. IDs must be nonzero and unique per item kind.
See the serializable types in `crates/cam/src/model.rs` and original fixtures in
`crates/cam/src/corpus.rs` for complete payloads. Without a path, exports return `content`.

Facing cuts stock with raster passes. Contouring uses geometric inside/outside cutter-radius
offsets. Pocketing uses successive offsets, handles islands, and retracts between loops.
Drilling expands every peck into ordinary motions and distinguishes tip depth from full-diameter
depth. Contours/pockets can use tangential leads and feed-controlled ramps. Allowance means stock
to leave; use a separate zero-allowance operation for finishing. Flat mills are explicitly assumed
to be center cutting. Entry/lead collisions are checked during simulation, before posting.

The reference spindle rotates clockwise (M3) with right-handed cutters. Climb milling keeps
the finished material to the right of travel: counterclockwise inside walls, clockwise around
external bosses and pocket islands. Facing advances rows in +Y and cuts toward -X for climb;
conventional reverses each cutting direction. Full-width entry can engage both sides of the
cutter. Independent tests interpret posted NC and check travel direction and signed loop area
for both settings and both input windings. The convention follows the
[Haas mill programming workbook](https://www.haascnc.com/content/dam/haascnc/videos/bonus-content/ep81-essential-9-lines/programming-workbooks/mill---programming-workbook.pdf),
printed pages 51 and 61. Generator version `cam/14` invalidates paths made with the earlier
reversed direction settings; regenerate and simulate before posting.

Offsets use exactly `cavalier_contours 0.9.0` (MIT OR Apache-2.0) behind bounded input checks.
Crossing/touching loops, nested islands, oversized tools, non-finite values and exceeded budgets
are errors. The offset adapter uses 1e-8 mm position/join and 1e-7 mm distance tolerances;
these avoid merging small distinct slices in round-island pockets with upstream defaults.
Boundary validation queries an edge spatial index instead of charging every possible edge pair
against its work budget. Crossings, near touches and adjacent backtracking still fail; candidate
work remains capped, with cancellation checkpoints during construction, queries and containment.
The offset adapter scales coordinates by a power of two so the shortest edge is at least one
library unit, and scales distance tolerances by the same factor. This avoids the library treating
short nonparallel segments as parallel through its cross-product epsilon. It removes no vertices
and restores output to millimetres; the original input remains the independent verifier's target.
Dense convex/concave, reversed-winding, translated/scaled and 100-island regressions cover the
adapter. Output retains operation IDs, machine events and linear/planar-arc motions.

## Reference NC dialect

The post emits tool-tip coordinates, three linear axes, absolute mm positioning, XY arcs with
incremental I/J centers, units-per-minute feed and geometrically applied cutter compensation.
Post version `solidcraft-reference-3axis/2` explicitly initializes
G21/G90/G17/G91.1/G94/G54–59/G40/G49/G80 and stops the spindle and coolant with M5/M9
before the declared initial pose. It emits tool/spindle/coolant events,
and expands drilling. Startup is a declared position at clearance; physical travel to that
initial pose is outside this reference dialect's contract. There is no machine kinematics,
G28/G53 homing, length offset activation, canned cycle or controller-specific behavior.

The separate NC reader consumes every word and rejects unsupported commands. It supports G20
and G91 for comparison, but requires complete absolute XYZ for the initial pose. Modal groups,
units, feed, tool changes, spindle state, planar arc radii and program termination are checked.
M6 stops the spindle, preserves coolant and the programmed speed, and requires a subsequent
M3 before cutting. Speed words take effect before motion and alongside coolant/stop commands.
Program end includes spindle/coolant shutdown in the interpreted event stream. Hand-authored
modal programs check these transitions and reject cutting with a stopped spindle. The post
round trip checks its three startup events separately, without assigning them an operation;
subsequent motions are compared against the pre-post typed IR with a 0.001 mm error budget.

Semantics reference: [NIST RS274/NGC](https://tsapps.nist.gov/publication/get_pdf.cfm?pub_id=823374).
The reader is a deliberately strict subset, not a general RS274 interpreter.

## Verification and present limits

The independent verifier uses adaptive XY cells with conservative lower/upper stock heights,
flat-mill or drill-cone removal, local target-surface checks, and swept rapid/shank/holder bounds.
Fixture checks use pinned `parry3d-f64 0.31.1` (Apache-2.0): separate cutter, drill cone, shank and
holder shapes are swept against fixture triangles. Initial-distance and closed-volume winding
checks catch stationary contact and tools already inside fixtures. Body groups preserve enclosed
cavities and prevent opposite winding in separate fixtures from cancelling. Open/nonmanifold
meshes, clearance within the interpolation/refinement margin, and failed numerical queries remain
**inconclusive**. It does not call offset or toolpath-generation algorithms and observes cancellation
during topology preparation and collision queries. The current algorithm fingerprint is `cam/18`;
the geometry and tessellation changes invalidate older generated/verified caches.
No target fitting, translation, unit guessing or discarded unknown NC commands is allowed.

Default dimensional tolerance is 0.01 mm; interpolation/post error is 0.001 mm; requested spatial
refinement is 0.0025 mm. Reports distinguish requested refinement from the actual maximum stock
height interval. Limits are 250,000 cells and 30 million work units by default. Exhaustion or an
unresolved collision blocks posting. Remaining-stock volume is an interval, not an exact number.

Stock STL contains a conforming conservative exterior of the upper height envelope. Shared edges and
vertical steps are split, internal walls are removed, and every edge and vertex link is checked
for a closed, consistently oriented manifold. Coplanar cells merge exactly without resampling.
Vertical drill cones use exact rectangular-cell radial extrema. Adaptive cells can still create
tiny saddle contacts: export may fill these upward by at most 0.00001 mm per cell, with no XY
snapping or removed material. This declared artifact budget does not alter simulation intervals;
analytic artifact checks independently compare the resulting STL against those intervals.
Exports are bounded to 60,000 merged cells, 500,000 triangles and 128 MiB. Larger or unresolved
non-manifold contacts are rejected explicitly; validation JSON retains the cell intervals. `stock_complete` is
false after verification budget exhaustion, which blocks export of a misleading partial stock.
A watertight upper-bound mesh does not establish finished-part dimensional accuracy.

Stock comparisons require declared stock bounds and exact, non-overlapping cell coverage. Older
reports without bounds remain inconclusive. Imported reference meshes must be closed, consistently
oriented manifolds within those bounds, with downward faces only on the stock bottom. Unsupported
undercuts, reversed faces, degenerate triangles and out-of-frame vertices are rejected. Projection
coverage uses boundary incidence and pinned `robust 1.2.0` orientation predicates, not sums of
triangle areas. Duplicate or overlapping faces cannot counterfeit coverage. Extremely small nonzero
XY coordinates below 1e-100 are rejected to avoid predicate underflow; coordinates are never snapped.

The comparator independently bounds reference heights from triangles, reusing only the validated
spatial partition before further adaptive subdivision. It certifies whole-column distances for
both remaining material and removed space, in both directions, including the stock exterior.
This resolves narrow uncertainty bands at vertical walls without concealing an omitted deep hole.
Failures require a definitely present material/void point farther than tolerance from all possible
corresponding columns; inconclusive intervals are not failures. Distance maxima describe examined
regions; a certified failure may stop at its first witness. Cell/work exhaustion remains inconclusive.
Tests include independent analytic round holes, alternate facet layouts, generated drilling-stock
round trips, shifted walls, missing holes, incomplete coverage and malformed reference meshes.

The current passing verification status certifies the implemented no-gouge/clearance checks.
The separate `finished_dimensions` status certifies facing planes, analytic drill cylinders/cones,
pocket floors and islands, and the selected side of contour walls. The milling verifier uses
independent robust winding and point-to-segment distances, with whole-cell signed-distance bounds;
it never calls the generator's offset functions. A contour must finish its selected wall and floor
within one tool radius of the allowance boundary. Farther stock on the cutting side may remain as
roughing stock; it cannot be cut below the specified floor. A pocket must clear its full selected
interior to the allowance floor while preserving its islands and outer wall.

The radial tolerance band reserves half the dimensional tolerance for paired-wall/diameter error,
then subtracts the post/interpolation budget. Associated CAD profiles also reserve their chord-error
budget, at most 0.0005 mm (and at most 1/16 of operation tolerance), during engine sampling and
independent certification. Requested features drive adaptive refinement even if the NC omits them.
Multiple operations compose by taking the minimum permitted remaining-height envelopes.
Drilling requires zero stock allowance: use explicit depth, since drill paths do not apply milling
allowance. Sharp inaccessible internal corners, omitted islands, wrong tools/depths and shifted
paths fail dimensional validation and block posting. Exhaustion and unresolved bounds stay
inconclusive. Posting requires both clearance and finished dimensions to pass and complete stock.

Motion queries use an independent XY index and retain original sequence IDs for withdrawals and
machine-state checks. Exact item bounds exclude irrelevant candidates; shank/holder radii remain
included whenever those components can reach stock. Region predicates have bounded work and
cooperative cancellation. The analytic contour/pocket recipes now use an original 128-sided round
boundary and matching protected walls/floors; they no longer rely on a plane-only square smoke test.
Constant-height and vertical mill sweeps use rectangular-cell lateral bounds: the segment's
bounding box tightens minimum distance, and convexity bounds maximum distance at the four corners.
This avoids artificial overlap from circumscribed disks along straight walls. Sloping moves retain
the common-disk height bound; neither tolerances nor resource limits are relaxed.

Protected height-field boundaries use local spatial certificates. A conservative upper target
height at a neighboring XY point bounds distance to the outside of the protected solid for an
entire potentially removed cell column. A gouge is reported only with a definitely removed point
inside a protected volume by more than tolerance. Clearance reserves 0.001 mm post/interpolation
error plus the 0.0025 mm verifier budget; CAD tessellation uses half that verifier budget as a fixed physical tolerance. It bypasses the
display mesher's size-dependent tolerance floor and planar display reprojection. Exhausted edge
or surface refinement, non-finite evaluations, missing faces and excessive triangle output fail
preparation instead of accepting a coarser or partial target. The original large-cylinder
regression (radius 5000 mm) previously had 0.00954 mm radial chord error for a 0.00125 mm
request; the verification path reduces it below 0.00125 mm, including through engine snapshot
preparation. These checks expose known mesher fallbacks; they are not an analytic approximation
proof for arbitrary oscillatory surfaces.
Queries use an independent XY index, observe work limits and cancellation, and leave unsuccessful
certificates explicitly inconclusive with a `protected_surface_unresolved` finding.

The original drilling fixture now contains the plate top, hole wall and cone, replacing its former
plane-only protected target. Tests cover boundary clearance, shifted-hole and through gouges,
sloping surfaces, overlapping projected targets and large-coordinate precision. A native CAD
drilled body also passes create/generate/simulate/post/save/reopen through registered commands.
This does not establish qualification of arbitrary non-height-field solids.
Native CAD pocket and contour workflows also pass through durable sketch-profile references,
ramp entry, generation, simulation, posting and save/reopen. All four analytic strategies now
require passing finished-dimension checks; independent point probes supplement those certificates.
The Fusion oracle requires the same dimensional checks for both programs. Current acceptance
evidence and supported limits are:

- Nine analytic recipes / fourteen operation stages cover concave regions, two islands, a narrow
  channel, allowances, multiple depths/tools, shifted origins and blind/through drilling. Deliberate
  faulty programs check missing cuts, protected material, unsafe leads and inaccessible channels.
- Fixture regressions cover thin swept obstacles, initial overlap, enclosed volumes and cavities,
  holder/shank interference, malformed/open meshes and cancellation. Supported stock/model geometry
  remains vertical height-field scope. Fixture triangles must form closed consistently wound bodies
  for volume-containment certification; surface contact can still be detected on an open mesh.
- Background jobs yield during preparation, boundary validation, between offset queries and every
  256 emitted motions, as well as during verification, NC round trips, fingerprint encoding and
  conforming stock extraction. The browser shares a four-ms scheduling target across jobs in
  round-robin order; status queries advance one checkpoint. This target is checked between polls,
  so it is not a hard deadline. Measured dense offset, large-body tessellation and long-reference
  encoding remain atomic for about 21.2, 90.8 and 8 ms respectively on the documented browser.
  The previously synchronous export path is now incremental: original large export probes measure
  batches at most4.3ms, with no batch above16ms; direct synchronous command calls remain atomic.
  The measured geometry calls above remain explicit scheduling limits, not a hard deadline.
  Native background jobs perform profile sampling, tessellation, transforms and hashing on the
  worker from an immutable snapshot. Synchronous command calls remain available. Status distinguishes
  `preparing` from `running`; the fingerprint is null until preparation completes. Cancellation
  cannot publish a partial path or revive a cancelled job.
- Associated CAD curves use second-derivative chord-error bounds after placement, including a
  numerical reserve and a shared 10,000-vertex boundary limit. Requests exceeding precision or
  vertex budgets fail explicitly rather than silently loosening the operation tolerance.
- The four initial local Fusion reference cases pass. Native desktop walkthroughs cover each
  strategy through generation, simulation, posting through the system save dialog, save/reopen/
  edit/regenerate, stock display and playback. Eight additional recovery captures cover lost-profile
  reselection, line-linked fixture collisions and correction, simulation cancellation and stale
  feed edits followed by regeneration. Preparation cancellation is covered by an engine regression;
  it was not manually clicked during that walkthrough.
- The [completed acceptance audit](cam-acceptance.md) maps the original requirements to
  implementation and concrete assertions, including multi-operation ordering and native saving.

The first milling milestone is complete within these declared limits. The passing reference
corpus is evidence for its recorded fixtures, not qualification of arbitrary parts or physical
controllers.

## Reproducible comparisons

Run `cargo xtask cam-validate`. Original analytic recipes generate input snapshots, canonical paths,
NC, stock, setup sheets, validation JSON and an HTML report under ignored `target/cam-validation/`.
Nine recipes cover the four initial strategies, a rounded concave pocket with two islands,
a four-operation facing → roughing → finishing → peck-drilling program with multiple tools and
a shifted G55 setup, two clamped through holes, an external square-boss contour and a narrow capsule channel. The harness
verifies prefixes of the actual complete program after every operation, including roughing
allowance, coolant transitions and conventional finishing. All fourteen
stages require protected-surface, dimensional, point-probe and independent exported-STL checks.
The new target surfaces have independently calculated volume expectations; deleting islands,
omitting finishing, choosing the wrong tool or changing the work offset must fail. Reports link
each stage's NC diagnostics, setup sheet and stock, and include tool order, retract count, cutting
and rapid distance, feed-time estimate and a remaining-volume interval. Analytic checks
run in ordinary CI and `cargo xtask ci`. Core tests include modal state, inch/mm equivalence, wrong
arc direction, deliberate gouges/unsafe rapids, shank interference, fixture overlap, geometry errors,
islands, ramps and cancellation. Engine tests cover save/reopen, undo and stale-result rejection.
Post validation also compares arc sweeps: a tiny arc rounding to coincident endpoints must not
become a full circle. Generated arcs shorter than 0.00025 mm become lines, bounded by their total
arc length. Shared reference-mesh edges do not cause false interior height discontinuities;
positive-area overlaps retain both heights even for extremely thin regions.

The through-hole recipe uses both simple and peck drilling with full-diameter depth below the
stock bottom, a closed raised clamp between the axes, and an independently calculated perforated
plate target. Tests reject missing breakthrough, insufficient cutting length and a rapid crossing
the clamp at Z7, identifying its NC line. The external-contour recipe uses a reversed square
boundary, four depth passes, tangential leads, ramp entry, 0.2 mm roughing allowance and a separate
conventional finish. Wrong-side cutting and omitted finishing cannot pass. The final external
contour uses 243,154 cells in the current native run, below the unchanged 250,000-cell default;
larger jobs can still return inconclusive when their declared budgets are exhausted.

The narrow-channel recipe has a 2.2 mm width, 10 mm between semicircle centers and a 1 mm
cut depth in a 20 × 20 × 6 mm plate. Its R1.1 ends use 64 original polygon segments per
semicircle; a 2 mm cutter must finish the floor and both walls while preserving the surrounding
plate. An independent polygon-area formula checks target volume, and point probes check both
ends, the center and protected stock. Negative regressions reject a 2.4 mm cutter, detect uncut
material in a 1.6 mm neck between accessible R2 lobes, and reject a 5 mm lead that gouges the
original target. The lead failure retains its operation/motion/NC-line attribution. These cases
exercise the unchanged dimensional, clearance and posting gates; they are original analytic
evidence and do not count as additional Fusion reference captures.

Run `cargo xtask cam-oracle --dir plan/fusion/cam/oracle` for local Fusion packages. The harness
requires separate `face`, `contour`, `pocket`, `drill` directories. Missing evidence is a failure
to establish parity, never a skipped pass. Each directory requires:

- `manifest.json`: nonempty `fusion_version`, `captured_at`, `input_fingerprint`,
  `post_identity`, SHA-256 `post_sha256`, and `capture_complete: true` only after actual capture.
  `capabilities` records a nonempty `license` string and an array of available `strategies`.
  `source_geometry` names the captured source file inside the package (for example `source.step`).
  `artifact_sha256` maps every required artifact below, including source geometry and both input
  snapshots, to its SHA-256 hash. Hashes bind the captured set and reject stale replacements;
  they do not establish that the input transcription or original capture was correct.
- `post.cps`, `post-properties.json`, `input.json`, independently captured `reference-input.json`.
  Input snapshots must match exactly, including frames, geometry, tools, allowance and settings.
- `reference.nc`, Fusion `setup-sheet.html`, `setup-sheet-values.json` with `setup`, `tools`,
  `operations` values transcribed from the captured sheet/API, and `simulation.json` as below.
- `stock.stl` in setup millimetres, captured after the operation. Each first fixture has one
  operation; multi-operation coverage must capture and validate every intermediate state.

`simulation.json` must record `status` (`pass`, `fail`, or `inconclusive`), `complete: true`,
`at_program_end: true`, a captured `findings` array, `completed_operations` in canonical input
ID order, and `stock_after_operation` equal to the final operation ID. Its `accuracy` object
records the actual UI `setting` and positive `resolution_mm` when available. If Fusion does
not report a numeric resolution, use `resolution_mm: null` and `resolution_status: "not_reported"`;
do not infer a resolution from the slider or confuse it with the independent verifier budget.
Missing accuracy/end-position evidence is incomplete. Captured failure or uncertainty prevents
a pass even when the independent checks pass. `post-properties.json` must contain valid JSON
(an object or array), including an empty object when the post has no properties.
Each finding has nonempty `detail` and `severity` (`error`, `unresolved`, or `information`).
An error or unresolved finding prevents a pass even if the captured summary says `pass`;
unknown severities are rejected. Add operation/motion/line locations when the reference exposes them.

The harness verifies provenance/input fingerprints and the actual post hash, interprets both
programs, simulates them through the same independent verifier, compares local spatial stock
bounds and the exported reference STL, checks setup values and reports distance/time/volume
deltas. Spatial indexing bounds local comparisons. HTML path overlays link to numbered NC listings and diagnostics. Stock heatmaps show sampled
remaining material relative to protected geometry; they are diagnostic views, not acceptance tests. Intermediate roughing must be judged against protected geometry and allowed stock;
identical roughing paths, retract counts or execution times are not acceptance criteria.

All three stock pairs must pass directly: SolidCraft NC versus reference NC, SolidCraft NC
versus reference STL, and reference NC versus its own exported STL. Tolerance comparisons
are not transitive: two results can each lie within 0.01 mm of a third while differing by
0.012 mm from each other. An original analytic regression exercises that failure. Reports
retain definite failures even alongside inconclusive checks and include `cam-oracle/2`
comparison provenance, capture settings, artifact hashes, generator/post identities and error
budgets. The imported NC report identifies the captured post, rather than attributing it to
SolidCraft's post. Synthetic unit-test packages are original expectations and never count
as Fusion parity evidence.

Create geometry from the original analytic recipe in each application. Record Fusion version,
license/capabilities, all resolved settings and capture timestamps. Use its current
[NC Program posting API](https://help.autodesk.com/cloudhelp/ENU/Fusion-360-API/files/PostToolpaths_Sample_Sample.htm)
and [Save Stock](https://www.autodesk.com/support/technical/article/caas/sfdcarticles/sfdcarticles/How-to-save-the-stock-as-an-STL-from-simulation-in-Autodesk-HSM-and-Fusion-360.html)
where no supported automated stock export exists. Do not fabricate missing artifacts or mark
incomplete captures complete. Additional fixtures must cover concavity, multiple islands,
reversed winding, narrow channels, round profiles, offsets, through holes and multiple tools.

All Fusion-produced files and derived reports belong in ignored `plan/` or outside the repository.
The oracle refuses an unignored in-repository destination. Commit only original recipes, harness
code and independently authored expectations. No vendor post source, templates or assets are
part of SolidCraft. Local references for facing, contouring, pocketing and drilling now pass
input reconciliation, setup values, both programs' clearance and dimensional checks, and all
three NC/stock comparisons (4/4). Together with the [acceptance audit](cam-acceptance.md),
this closes the planned first milling milestone. The contour, pocket and drill exports use the maximum
Stock Accuracy setting after replay to completion; the original coarser exports and their
failed/inconclusive comparisons remain in ignored local storage. Numeric Fusion resolution is
recorded as not reported, without substituting the independent verifier's resolution. Acceptance
remains 0.01 mm with the original post and verifier error budgets.

## Manufacture display and playback

The workspace shows the selected program's model, initial stock or simulated final stock, with
separate fixture coloring and visibility controls. Stock uses the setup coordinate frame; hidden
model and fixture bodies cannot be selected in the viewport. Editing a setup reveals the bodies
for reference selection. Missing or stale stock falls back to the model with an explicit message.
The remaining-stock display is the final conservative upper surface, not stock removal animated
at each playback step. Its mesh is cached by the simulation job and shares the bounded, conforming
boundary extraction used by STL export. Display extraction errors do not replace simulation status.

Playback advances typed motion/event blocks at an adjustable rate, independent of frame rate.
Zero means the initial clearance pose. The panel shows operation, tool, spindle and tool-tip
coordinates; paths distinguish rapid and cutting motion, including full-circle arcs. `cam.inspect`
exposes the same machine state, and `ui.inspect.manufacturing` exposes the current display choices.
Stock extraction runs on the native worker and cooperatively on WebAssembly. Cell sorting,
boundary stitching, triangulation, manifold validation and large topology-map cleanup yield and
check cancellation. Cancellation returns no partial mesh; synchronous STL export uses the same
implementation. Native UI tests cover stock-frame
placement, visibility, stale results and playback boundaries. The four-strategy headless UI
walkthrough exercises background generation/simulation, posting, stock selection and playback zero,
with CPU-rendered stock views. A separate native desktop-window walkthrough exercises facing,
internal contouring, pocketing and peck drilling on original CAD plates. Contour and pocket
operations retain associated circular sketch boundaries and ramp entry; drilling retains a
cylindrical face selected in the viewport. Closing and reopening the saved designs clears derived
jobs while retaining those references. Editing coolant in the operation dialog and regenerating
passes clearance and finished-dimension checks for each case. Native-posted contour, pocket and
drill files match their command exports byte for byte. Playback, final-stock display and stale-post
rejection were also checked. Local artifacts and screenshots are in ignored
`target/cam-native-acceptance/`; this is SolidCraft evidence, not a Fusion comparison.

A second disposable native-window walkthrough checks recovery. A missing profile produces a
reselection error; selecting the original profile and using the operation editor's **Use selected
sketch profile** and **Save** restores generation and passing simulation. A clamp intersecting
the pocket produces a cutter/fixture error attributed to motion 7 and NC line 20; **Locate issue**
seeks that motion. Moving the same associated clamp clear restores passing clearance and dimensions.
Cancelling active simulation through **Cancel job** leaves the job cancelled and blocks posting,
including after later jobs finish. Editing feed in the operation dialog marks the result stale;
posting rejects it, and regeneration/simulation passes. Geometry, deliberately broken references,
profile selection and fixture correction were arranged through registered commands; editor actions,
generation, simulation, cancellation and issue location used native UI input. Evidence is local in
`target/cam-native-recovery/`. Preparation cancellation is covered by engine regressions, not this
native walkthrough.

Interactive Save, Save As and Open use the same picker/background-loading flows as the toolbar;
JSON commands remain dialog-free. Keyboard shortcuts use the modifier state on each key event,
so a quick Command/Ctrl+S still saves if the modifier is released in the same frame. Regression
tests cover these flows, cancelled pickers and persistence of manufacturing definitions.

Verification findings include a one-based interpreted NC `record` when a physical line is
known. SolidCraft's independent post round trip also attaches `source` with physical line,
operation ID and one-based IR block (the completed-block count used by playback). Attribution
is added after verification and never influences clearance or dimensional decisions. Raw NC
comments and deserialized data cannot supply this trusted mapping; imported programs retain
line/record attribution without a guessed SolidCraft operation. Arc subdivisions share their
original source block. Program-wide and missing-cut findings may have no responsible line.

`cam.inspect {job, finding: index}` uses a zero-based finding index and returns the reported
world-space point, operation definition, associated CAD source, exact NC line and playback state
where mapped. It rejects stale jobs and invalid indices; failed verification still blocks posting.
The Manufacture panel's **Locate issue** action uses that command to center the reported witness
and seek the completed source block. The marker remains visible when paths are hidden, can be
cleared, and disappears when inputs change. The marked witness is not a claim of the exact first
collision contact, and the stock view remains the final conservative stock surface. Validation
JSON and HTML NC diagnostics retain the operation/block/line attribution.

Findings have explicit information, unresolved or error severity. The bounded list keeps one
representative witness per kind/NC line/severity, prioritizes definite errors when full, and
shows errors first. This list is diagnostic sampling, not an exhaustive inventory of affected
points; verification status and stock intervals are computed independently of finding retention.

## Browser responsiveness measurements

A local release-mode WebAssembly probe on Chrome 155 (macOS, 2026-10-15) measured the original
four analytic fixtures, using the actual generator, verifier and stock extractor. Before cooperative
stock extraction, its longest single call was 433.1 ms for contour, 275.8 ms for pocket and
383.1 ms for drill. After the change, the longest stock-extraction poll across those fixtures was
2.5 ms. Triangle counts and passing clearance/dimension results were unchanged.

A second probe reused the UI scheduler with a four-ms target and one batch per animation frame.
The largest observed batch was 4.9 ms; no batch exceeded 16 ms. These are local observations,
not performance guarantees or complete browser acceptance: the probe excludes CAD snapshot
preparation, the rendered application, export serialization, concurrent jobs and hostile-size
inputs. Later primitive/export probes below cover representative accepted-size inputs; their
remaining atomic latencies are explicit limits in the acceptance audit. Unit
regressions verify exact synchronous/cooperative mesh agreement, early/mid/late cancellation,
round-robin fairness, completed-job termination and a poll cap if the host clock stalls.

The clamped through-hole and external-contour recipes also pass the same local release Wasm
probe. Their largest observed scheduler batches were 5.2 ms and 7.4 ms, with no batch over 16 ms;
stock extraction peaked at 4.0 ms and 4.1 ms. The external contour produced 243,154 cells and
111,102 triangles. These measurements have the same limited scope described above.

## Associated drilling selections

In a drilling operation, select inward cylindrical faces or circular sketch profiles in drilling
order, then choose **Use selected holes**. The engine command accepts `selections` containing
`{kind:"face",body,index,occurrence?}` or `{kind:"profile",sketch,index,occurrence?}`. A face pick
without `occurrence` uses the displayed world body name; an explicit occurrence path uses the
body definition name. Profile picks use the sketch feature ID and evaluated profile index.
The returned `sources` belong in `Operation.hole_sources`, with the returned `holes` as the
preview coordinates in `geometry.holes`. The command also reports selected diameters.

Face sources store a body/occurrence identity and persistent face name. Profile sources store
sketch curve identities. Immutable job preparation resolves these again in setup coordinates,
including uncaptured occurrence moves. Stored preview centers are never used as a fallback.
Matching cylindrical seam faces on the same body occurrence become one hole, retaining the first
pick’s source. Duplicate identical references and conflicting coincident axes are rejected.

Missing, split-to-new-name or ambiguous faces/profiles require reselection. Noncircular profiles,
external cylindrical faces and axes not parallel to setup Z are rejected. A linked hole's diameter
must match the drill within
the operation tolerance; choose a matching tool or explicitly unlink before pilot drilling.

Linked coordinates are read-only in the operation dialog. **Unlink holes** retains the displayed
coordinates as explicit input. Feeds, spindle, top/bottom heights, peck depth and tip/full-diameter
depth remain user-controlled. The complete operation settings, resolved centers and source
identities are included in setup sheets and canonical toolpath JSON; inspection exposes the
operation's hole sources. Selection establishes an axis only: independent machining and clearance
verification still decides whether the program can be posted.

### Snapshot hashing responsiveness

A local Chrome 155 release-Wasm stress probe found synchronous snapshot hashing calls of
54.6 ms for 32 operations with 10,000 boundary vertices each, 172.6 ms for 1,000 operations
with 1,000 vertices each, and 92.2 ms for 200,000 target triangles. Background snapshot
preparation now hashes incrementally, yielding before each operation/tool and every 256
triangles. It preserves the exact existing serialized fingerprint, including field order and
numeric/string encoding; no algorithm-version or cache-key change is needed. Cancellation
checks surround every yield and cannot return a partial digest. Regression tests compare the
old and incremental encoders across the original corpus and a maximum-size triangle snapshot.
On the same local probe, maximum scheduler batches for those three large cases were 5.1,
4.2 and 4.2 ms respectively; none exceeded 16 ms. These are measured observations, not hard
deadlines.

One setup or operation is still encoded atomically. This hashing probe alone does not measure
large reference lists, CAD tessellation, a single offset query or direct synchronous exports.
The subsequent primitive/export probes cover those cases and record remaining atomic limits.
These measurements do not establish whole-application latency guarantees.

### NC round-trip responsiveness and limits

Background simulation now yields during reference posting, independent modal interpretation
and IR correspondence checking. Posting and correspondence yield every 256 blocks; the reader
yields every 256 physical lines, including comment/blank lines. Cancellation is checked before
and after each yield and returns no partial G-code, parsed program or trusted line mapping.
The synchronous APIs use the same processing code with immediately ready checkpoints.

A local Chrome 155 release-Wasm probe measured a 228.9 ms synchronous round trip for 199,990
blocks. The cooperative path's largest scheduler batch was 4.2 ms. A 200,000-block path also
passes, with 200,003 interpreted records including the three post startup events; its largest
batch was 4.1 ms. Neither case had a batch over 16 ms. The former reader incorrectly counted
startup against the generated-path limit and rejected the maximum-size path. Both posted blocks
and external interpreted records now have explicit, independently checked bounds.

Regression tests exercise the exact path limit and excess input, synchronous/cooperative bytes
and machine-state equality, operation/block/line provenance, unchanged unknown-command errors,
and cancellation at every checkpoint across all three stages. Measurements exclude direct
synchronous export commands, CAD preparation, offset queries and the rendered application;
these observations do not establish a universal latency guarantee.

### Dense boundary validation and offsetting

The original 10,000-vertex seven-lobe boundary exceeded the all-pairs validation budget before
reaching offset generation. Indexed candidate checks now admit that simple boundary without
relaxing intersection tests or the candidate-work limit. A separate dense-circle regression
exposed a library assertion on short segments; power-of-two coordinate normalization resolves
the reproduced case without simplifying geometry. Independent regular-polygon distance checks
cover every output vertex at two scales and shifted coordinates; concave checks sample output
vertices against the original input segments. These checks are not whole-stock certification.

A local Chrome 155 release-Wasm generation probe on 2026-10-16 produced 6,674 blocks from the
10,000-vertex seven-lobe input. Its longest scheduler batch was 21.2 ms (one batch above 16 ms),
compared with 6.4 ms for the 1,400-vertex case. Validation is cooperative, but the library offset
query remains atomic. This records an unresolved responsiveness limit; it is not a guarantee of
frame-rate responsiveness. The probe excludes CAD preparation and stock verification.


A 2026-10-16 primitive probe measures the remaining atomic work separately from machining
acceptance. A 100-island pocket with 9,904 vertices emits 80,307 blocks with a maximum 9.7 ms
scheduler batch. Hashing 1,000 setup references or 10,000 profile curve IDs (256-byte names and
64-element occurrence paths) takes batches up to 3.2 ms and 8.0 ms respectively. These original
synthetic probes test computation; they do not supply matching intended geometry for dimensional
acceptance. Fixed-tolerance CAD cylinder tessellation takes about 12 ms at radius 10 mm,
102 ms at radius 500 mm and 1,087 ms at radius 5000 mm, producing 3,560, 8,188 and 28,382
triangles. The largest case now respects the requested 0.00125 mm chord tolerance instead of
silently coarsening it. Native preparation runs on its worker, but a Wasm shell tessellation is
still atomic and the measured large-body stall remains a browser responsiveness gap. Local
before/after evidence is in `target/cam-web-probe/primitive-baseline.json` and
`primitive-fixed.json`; output budgets and tolerances were not relaxed.


The follow-up mesher optimization uses Spade's bulk point insertion and an original conservative
line/AABB index for trim-boundary candidates. It retains every boundary segment and the existing
winding predicate; uncertain arithmetic visits candidates rather than pruning them. Rounded
positions are mapped back to vertex handles so duplicate points do not change constraint indices.
Differential tests compare indexed and complete winding scans across concavity, holes, reversed
winding, translated/scaled coordinates, boundary points and nearly parallel rays. Kernel checks
independently assert concave two-hole prism volume/containment and large-cylinder chord/volume
bounds. The algorithm fingerprint advances to `cam/18` because tessellation can change triangle
ordering. A local instrumented prototype reduced the radius-5000 case from about 1089.8 ms to
96.5 ms with the same 28,382 triangles and 0.00100715 mm maximum measured radial chord error.
Its segment visits fell from 352,784,748 to 1,462,907. This optimization does not make shell
meshing incremental; its remaining atomic latency is recorded below. Incremental exports are
covered by the subsequent acceptance checks.

The uninstrumented production release-Wasm follow-up measured 10.3 / 27.2 / 90.8 ms for radii
10 / 500 / 5000 mm, with the same triangle counts and radial errors as the fixed-tolerance
baseline. The largest case remains an approximately 91 ms atomic step on this machine; no
hard scheduling deadline or full browser responsiveness claim follows from this measurement.
Evidence: `target/cam-web-probe/tessellation-production-index.json`.


## Incremental export acceptance

A quiet Chrome155 release-Wasm probe with an original 200,000-block path measured synchronous
toolpath JSON up to296.7ms and posting/roundtrip342.5–360.8ms. An original tiled height field
with40,000cells exported34,051,171bytes of STL in540.5ms; validation JSON took35.5ms. These are
serialization/resource measurements, not claims that a synthetic report passed machining checks.
The incremental path avoids a full intermediate JSON Value for large arrays, yields between
motion/cell/triangle batches and each operation/tool, and reuses cooperative stock extraction
through ASCII serialization. Focused tests compare all JSON fields, HTML values/escaping and
STL/NC bytes, inject cancellation at every serialization checkpoint, and check stale/cancelled
background results cannot write output. The UI regression now exercises background posting and collection through an injected save-picker
service. A disposable native command walkthrough additionally exports all five formats, including
a33.9MB stock mesh, and verifies no destination file exists before explicit result collection.
A fresh system save-dialog attempt was not completed: native accessibility timed out and window
capture reported no presented frame. That manual check is not counted as a new passing walkthrough.

The incremental follow-up measured at most4.3ms per scheduler batch for the same original export
cases (no batch above16ms). The40,000-cell STL is still34,051,171bytes, and the200,000-block NC
program is still11,999,980bytes. Toolpath JSON shrinks from41,002,015 to15,201,072bytes through
whitespace removal; semantic equivalence is asserted against the original serializer. Evidence:
`target/cam-web-probe/exports-baseline.json` and `exports-incremental.json`. These timings exclude
file-system publication and are machine-specific; command freshness checks still govern publication.


The final native save retry used a regular disposable app bundle. The actual system Save dialog
created `posted.nc` (146,348 bytes), identical to the registered-command NC export. Job status
confirmed a fresh verified job, passing clearance/dimensions and idle export after UI collection.
Post-save screenshot capture remained unavailable; the result is established by the output file
and job state. Evidence: `target/cam-native-audit/save-acceptance.json`.

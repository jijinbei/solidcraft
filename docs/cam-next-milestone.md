# Adaptive clearing and 3D milling

Status: planned implementation following the first milling PR. The strategies and
ball-end tooling below are not implemented or qualified yet.

## Scope and delivery order

Retain vertical, fixed-axis three-axis machining, rectangular stock, rigid setups,
explicit feeds/speeds and fail-closed posting. Indexed/multi-axis work and physical
controller qualification remain outside scope.

1. **Tool and verifier foundation.** Add ball-end mills with explicit cutting length,
   shank and holder dimensions, a pending document migration, command/UI editing and
   tool-tip semantics. Qualify swept spherical cutting envelopes independently on
   stationary, horizontal and sloping motions, including holder/shank clearance.
2. **Adaptive clearing.** Introduce explicit optimal load, maximum stepdown, entry
   and linking settings. Maintain material state during planning; bound actual radial
   engagement through corners and narrow passages. Generate smooth clearing loops
   with proved entry/link clearances. Ordinary offset pocketing is not adaptive clearing.
3. **3D roughing.** Slice the protected target at bounded Z levels, include radial
   and axial stock allowances, and support rest roughing from a fingerprinted preceding
   stock state. Reject unsupported undercuts, uncertain topology and inaccessible
   regions. Removed material must never be inferred solely from a preceding path's name.
4. **3D finishing.** Deliver parallel finishing for shallow surfaces and constant-Z
   finishing for steep surfaces, with ball tools, cutter-contact offsets, bounded
   chord error and explicit cusp/stepover settings. Scallop finishing follows only
   after its surface-spacing and connector bounds are independently qualified.
5. **Integrated acceptance.** Exercise editing, generation/cancellation, simulation,
   diagnosis, exports, save/reopen and stale-result refusal through commands and the
   visible Manufacture workspace. Keep portable jobs incremental and bounded.

## Independent validation

Keep the generator, NC interpreter and stock verifier separate. Extend typed motions
and provenance only where required; preserve the reference post's initialized machine
state and reject unsupported controller behavior. Inspect every emitted NC block.

Original fixtures cover a sloped plane, convex dome, concave bowl, saddle, stepped
boss, steep wall, islands connected by a narrow neck, interrupted cuts, rest stock,
shifted origins and tool changes. Each fixture declares protected surfaces, allowed
rough stock, finished regions, accessibility and independent dimensional expectations.
Hand-calculated sphere-plane contact and cusp-height cases qualify the verifier first.
Fault injections include the wrong ball radius, omitted roughing stock, excess
engagement, reversed arcs, connecting gouges, rapid-through-stock and holder contact.

Acceptance retains 0.01 mm dimensional tolerance, 0.001 mm post/interpolation budget
and 0.0025 mm verifier uncertainty. Local surface distances and hole/wall dimensions
accompany volume differences. Refine uncertain regions; exhausting resources yields
inconclusive and blocks posting. For roughing, verify protected geometry and allowed
stock per operation instead of requiring identical intermediate material removal.
Engagement checks report their definition, sampling/bound uncertainty and the NC line.

## Fusion comparison packages

Use the same original recipes in local Fusion and SolidCraft. Reconcile units, tools,
stock, frames, allowances, entry/links and operation settings before comparison.
Record version, available capabilities, post identity/hash/properties and timestamps.
Capture NC Program output, setup HTML and final simulation stock after every operation;
use Save Stock when automated export is unavailable. Preserve the actual Accuracy
setting and report an unexposed numeric resolution as not reported.

Both posted programs pass through the independent interpreter and verifier. Compare
each against intended geometry and Fusion stock, with line-linked diagnostics, path
overlays, surface differences, volume, distance, retract count and time estimates.
Missing artifacts or unavailable licensed strategies cannot count as passing parity.
Require complete passing packages for each delivered strategy before declaring parity.
All Fusion-produced artifacts and derived comparisons stay in ignored local storage;
commit only original recipes, harness code and independently authored expectations.

## Dependency decisions

Keep the existing pinned Cavalier 0.9.0 adapter for planar offsets/islands and Parry
0.31.1 for bounded collision/distance queries. Both need geometry-specific guards;
neither provides an adaptive machining algorithm or an independent stock proof.
Cavalier documents rounded joins and arc-size constraints; retain current degeneracy
and scale tests. Parry's double-precision mesh and point-query APIs are candidates for
cutter-contact acceleration; benchmark against a simple original implementation first.
No additional runtime dependency is justified yet. Reuse the existing robust predicates,
Spade and spatial index; consider property testing as a development-only dependency.

Primary references inspected on 2026-10-16:

- [Autodesk adaptive roughing overview](https://help.autodesk.com/cloudhelp/ENU/Fusion-CAM/files/3D-ADAPTIVE-READ.htm)
- [Autodesk adaptive parameters](https://help.autodesk.com/cloudhelp/ENU/Fusion-CAM/files/GUID09E44604-DAD8-47D6-ADC6-C100869DE724.htm)
- [Autodesk parallel finishing](https://help.autodesk.com/cloudhelp/ENU/Fusion-CAM/files/3D-PARALLEL-STEPS.htm)
- [Cavalier 0.9.0](https://docs.rs/crate/cavalier_contours/0.9.0)
- [Parry 0.31.1 triangle mesh](https://docs.rs/parry3d-f64/0.31.1/parry3d_f64/shape/struct.TriMesh.html)
- [Parry 0.31.1 point queries](https://docs.rs/parry3d-f64/0.31.1/parry3d_f64/query/point/trait.PointQuery.html)

Each implementation arc runs focused analytic/fault tests, then repository quality
and Wasm gates. The oracle command must require the strategies actually claimed by
that milestone, without relaxing the first milestone's completeness requirements.

# Exchange snapshot

Schema 1 (2026-10). Crate `solidcraft-exchange` (`crates/exchange`), CLI
`solidcraft-cli export-snapshot`.

Analysis tools (electromagnetics, structural, CFD) need a design's shape at given parameter
values, its faces by name and kind, and its materials. They should not depend on SolidCraft's
kernel or document model, which change often. The exchange snapshot is the one stable boundary
between them: tools depend on `solidcraft-exchange` alone, or read its JSON.

## API

```rust
let design = solidcraft_exchange::open(&bytes)?;            // a .sc / .solidcraft / JSON design
let params: Vec<Parameter> = design.parameters();          // name, expression, value, unit, kind
let snap: Snapshot = design.snapshot(
    &[("width".into(), Override::Value(30.0))],            // or Override::Expression("3 cm".into())
    &Options::default().with_tolerance(Tolerance::Relative(2e-4)),
)?;
let json = solidcraft_exchange::to_json(&snap)?;           // deterministic bytes
```

`snapshot` evaluates the timeline with the overrides applied (all or none) and leaves the design
as it was. Snapshots of one `Design` share an evaluation, so a second snapshot recomputes only
the features its overrides reach. A feature that fails at those values fails the snapshot, naming
the feature (`Error::Evaluation`). An override value is a number in the parameter's own unit;
an expression is what the parameters table takes.

```text
solidcraft-cli export-snapshot part.sc --out part.json --param width=30 --param r="1.5 mm" \
    [--tolerance 0.01 | --relative-tolerance 2e-4] [--body Body1]...
```

## Content

```json
{
  "schema": 1,
  "unit": "mm",
  "design": "bracket",
  "parameters": [
    {"name": "width", "expression": "30 mm", "value": 30.0, "unit": "mm", "kind": "length", "source": "user", "comment": ""}
  ],
  "materials": [
    {"name": "Steel", "density_kg_m3": 7850.0, "properties": []}
  ],
  "bodies": [
    {
      "name": "Body1", "material": "Steel", "closed": true,
      "positions": [[0.0, 0.0, 0.0], ...],
      "faces": [
        {"index": 0, "name": "F1:-z", "surface": {"type": "plane", "point": [0, 0, 0], "normal": [0, 0, -1]},
         "triangles": [[0, 2, 1], ...], "attributes": {}},
        {"index": 6, "name": "F2:side", "surface": {"type": "cylinder", "origin": [10, 10, -1], "axis": [0, 0, 1], "radius": 3.0},
         "triangles": [...], "attributes": {}}
      ],
      "attributes": {}
    }
  ]
}
```

- **Units.** Lengths and coordinates in mm (`unit`), angles in radians, density in kg/m³,
  material values in the SI unit their key ends with.
- **Parameters.** Every user parameter, sketch dimension, named feature input and text
  parameter, in the parameters table's order. `value` is in `unit` (`mm`, `deg`, `in`, … or empty
  for unit-less; none for text). `kind`: `length`, `angle`, `unitless`, `text`. `source`: `user`,
  `sketch`, `feature`. SolidCraft has no parameter bounds yet; they would be added as fields.
- **Materials.** The physical materials the exported bodies are assigned, by name. `density_kg_m3`
  is the density SolidCraft computes mass with. `properties` carries the engineering card of a
  design material (`material.create`), as entered, uncertified: `kind` is `density`, `elasticity`,
  `tensile_strength`, `compressive_strength`, `yield_strength`, `shear_strength`,
  `thermal_conductivity`, `thermal_expansion` or `specific_heat`; `values` hold `kg_m3`,
  `young_modulus_pa` and `poisson_ratio` (isotropic), `young_modulus_pa_1..3`,
  `shear_modulus_pa_1..3`, `poisson_ratio_1..3` (orthotropic), `pa`, `w_m_k_1..3`, `per_k_1..3`,
  `j_kg_k`, and the conditions `temperature_k`, `moisture_dry_basis_kg_kg`, `strain_rate_per_s`;
  `labels` hold `symmetry`, `axes` (`material123`, `wood_lrt`), `state` and `source` (the card's
  source id). A body without an assigned material has `material: null`.
- **Bodies.** In world coordinates, in the design's order. In an assembly every placement of a
  component's body is its own body: the first placement keeps the body's name, every further one
  adds its occurrence path in brackets (`Body1 [Arm:2]`), so names are unique and stay the same
  while the assembly's structure does.
- **Mesh.** One tessellation per body, the one the tagged 3MF export writes
  ([3mf-face-tags.md](3mf-face-tags.md)): welded watertight (vertices within 10⁻⁶ of the body size
  merged, T-junctions split), vertices shared across faces, triangles counter-clockwise seen from
  outside. `closed` says every edge is used once in each direction. The chord tolerance is
  2·10⁻⁴ of the body's size by default (the export's standard quality), never below 1 µm.
- **Faces.** In the body's own face order (`index`, as the 3MF face tags number them), each with
  its persistent name (`F1:-z`, `F2:side`, `F3:side:l4`, pieces of a split face as `F2:side#[F1:-x]`), which stays with the face
  when parameters change, and its analytic surface: `plane` (`point`, `normal` out of the
  material), `cylinder` (`origin`, `axis`, `radius`), `cone` (`apex`, `axis` into the cone,
  `half_angle`), `sphere` (`center`, `radius`), `torus` (`center`, `axis`, `major`, negative on a
  spindle torus's inner sheet, `minor`), or `other` (splines, imported meshes).
- **Attributes.** Key/value maps on bodies and faces for downstream semantics (ports, boundary
  conditions). Designs have no place to store them yet, so they are empty.

## Compatibility

- Fields are only added, never renamed, retyped or removed. A field added later has a default, so
  readers of any version read any snapshot: missing fields read as their defaults, unknown fields
  are ignored.
- An unknown surface type reads as `other`, an unknown parameter kind or source as `other`.
- `schema` goes up only for a breaking change; a reader refuses a schema above its own.

## Limits and determinism

The design file is at most 256 MB; a snapshot takes at most 1,024 overrides (finite values up to
10¹², expressions up to 4,096 bytes) and holds at most 100,000 bodies, 1,000,000 faces and
20,000,000 triangles. A relative tolerance lies in [10⁻⁶, 0.1], an absolute one in
[0.001, 10,000] mm. The same design, overrides and options give the same bytes: fixed body, face
and triangle order, sorted maps and shortest round-trip numbers (-0 written as 0).

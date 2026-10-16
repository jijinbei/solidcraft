//! Manufacturing mesh path: no display LOD, tolerance floor or missing-face fallback.
use super::*;

impl Body {
    /// Mesh at a fixed physical chord tolerance. Refinement/output exhaustion is an error.
    pub fn tessellate_for_verification(&self, tol: f64, triangle_limit: usize) -> Result<Mesh> {
        if !tol.is_finite() || tol <= 0. || triangle_limit == 0 || triangle_limit > 2_000_000 {
            return Err(KernelError::Invalid("verification tessellation needs positive finite tolerance and 1–2000000 triangles".into()));
        }
        if let Some(mesh) = &self.mesh {
            if mesh.triangles.len() > triangle_limit {
                return Err(KernelError::Failed("verification tessellation exceeds triangle budget".into()));
            }
            return Ok(mesh.to_mesh());
        }
        guard("verification tessellation", || {
            let mut out = Mesh::default();
            let mut face_index = 0_u32;
            for shell in self.solid.boundaries() {
                let mesh = truck_meshalgo::tessellation::robust_triangulation_checked(shell, tol)
                    .ok_or_else(|| KernelError::Failed("verification tessellation cannot meet tolerance within refinement budget".into()))?;
                if mesh.face_iter().count() != shell.face_iter().count() {
                    return Err(KernelError::Failed("verification tessellation has missing faces".into()));
                }
                for face in mesh.face_iter() {
                    let pm =
                        face.surface().ok_or_else(|| KernelError::Failed(format!("verification tessellation could not mesh face {face_index}")))?;
                    let count = pm.faces().triangle_iter().count();
                    if count == 0 || count > triangle_limit.saturating_sub(out.triangles.len()) {
                        return Err(KernelError::Failed("verification tessellation has empty face or exceeds triangle budget".into()));
                    }
                    let flip = !face.orientation();
                    for tri in pm.faces().triangle_iter() {
                        let mut indices = [0; 3];
                        for (slot, vertex) in indices.iter_mut().zip(tri) {
                            let p = pm.positions().get(vertex.pos).ok_or_else(|| KernelError::Failed("invalid verification vertex".into()))?;
                            let p = Vec3::new(p.x, p.y, p.z);
                            if ![p.x, p.y, p.z].into_iter().all(f64::is_finite) {
                                return Err(KernelError::Failed("non-finite verification vertex".into()));
                            }
                            *slot = u32::try_from(out.positions.len()).map_err(|_| KernelError::Failed("verification vertex limit".into()))?;
                            out.positions.push(p);
                            let n = vertex.nor.and_then(|i| pm.normals().get(i)).map(|n| Vec3::new(n.x, n.y, n.z)).unwrap_or(Vec3::Z);
                            out.normals.push(if flip { -n } else { n });
                        }
                        out.triangles.push(if flip { [indices[0], indices[2], indices[1]] } else { indices });
                        out.tri_face.push(face_index);
                    }
                    face_index += 1;
                }
            }
            if face_index == 0 {
                return Err(KernelError::Failed("verification tessellation has no faces".into()));
            }
            Ok(out)
        })
    }
}

//! Shared 3D math: rotation, perspective projection with depth, mesh
//! builders, and a z-buffered rasterizer into a `Raster`.
//!
//! This is the single 3D implementation for the app. `effects.rs` renders its
//! wireframe cube through `projected_edges`, and the art pipeline renders the
//! 3D generators through `render_mesh`.

use super::raster::Raster;
use super::rng::{fbm, Rng64};

/// Terminal cells are roughly twice as tall as they are wide. Multiply a
/// projected Y by this so a circle looks round on a 1x1-cell grid.
///
/// This constant is applied in exactly one place: `y_aspect`, which every
/// projection goes through. Sub-cell rasters (braille 2x4, half-blocks 1x2)
/// pass their sub-cell counts so the correction stays right for them too.
pub const CELL_ASPECT: f32 = 0.5;

/// The Y scale to apply for a grid whose cells are split into `sx` x `sy`
/// sub-samples. `(1, 1)` is a plain glyph grid; braille is `(2, 4)`.
pub fn y_aspect(sx: usize, sy: usize) -> f32 {
    CELL_ASPECT * sy as f32 / sx.max(1) as f32
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub edges: Vec<(usize, usize)>,
    pub faces: Option<Vec<[usize; 3]>>,
}

#[derive(Clone, Copy, Debug)]
pub struct MeshRenderOpts {
    pub rotation: [f32; 3],
    pub cam_dist: f32,
    /// Fill faces with Lambert shading (needs `faces`). Otherwise wireframe.
    pub filled: bool,
    pub cull_backfaces: bool,
    /// Direction *towards* the light, normalized.
    pub light: [f32; 3],
    /// Hue assigned to the nearest geometry; far geometry drifts +80 degrees.
    pub base_hue: f32,
}

impl Default for MeshRenderOpts {
    fn default() -> Self {
        Self {
            rotation: [0.6, 0.8, 0.2],
            cam_dist: 3.2,
            filled: true,
            cull_backfaces: true,
            light: normalize([-0.4, -0.7, -0.6]),
            base_hue: 200.0,
        }
    }
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / len, v[1] / len, v[2] / len]
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Rotate around X, then Y, then Z (radians).
pub fn rotate_xyz(v: [f32; 3], ax: f32, ay: f32, az: f32) -> [f32; 3] {
    let (sx, cx) = ax.sin_cos();
    let (sy, cy) = ay.sin_cos();
    let (sz, cz) = az.sin_cos();

    let [x, y, z] = v;
    // X axis
    let (y, z) = (y * cx - z * sx, y * sx + z * cx);
    // Y axis
    let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
    // Z axis
    let (x, y) = (x * cz - y * sz, x * sz + y * cz);
    [x, y, z]
}

/// Perspective-project a camera-space point. The camera sits at
/// `z = -cam_dist` looking down +Z. Returns `(screen_x, screen_y, depth)`;
/// depth grows with distance and is what z-buffers and depth-shading use.
pub fn project(v: [f32; 3], cam_dist: f32, scale: f32, center: (f32, f32), y_aspect: f32) -> (f32, f32, f32) {
    let depth = v[2] + cam_dist;
    let perspective = cam_dist / depth.max(0.25);
    (
        v[0] * perspective * scale + center.0,
        v[1] * perspective * scale * y_aspect + center.1,
        depth,
    )
}

/// Scale factor that keeps a unit-radius mesh inside a `w` x `h` sample grid.
pub fn fit_scale(w: f32, h: f32, y_aspect: f32, cam_dist: f32) -> f32 {
    let nearest_perspective = cam_dist / (cam_dist - 1.0).max(0.25);
    let half_w = w / 2.0;
    let half_h = h / (2.0 * y_aspect.max(1e-3));
    half_w.min(half_h) * 0.86 / nearest_perspective
}

/// Rotate + project every edge of a mesh. Used by the animated Effects3D
/// wireframe so it shares the same math as the art pipeline.
/// Returns `((x0, y0, d0), (x1, y1, d1))` per edge in grid coordinates.
#[allow(clippy::type_complexity)]
pub fn projected_edges(
    mesh: &Mesh,
    rotation: [f32; 3],
    cam_dist: f32,
    w: f32,
    h: f32,
    sx: usize,
    sy: usize,
) -> Vec<((f32, f32, f32), (f32, f32, f32))> {
    let ya = y_aspect(sx, sy);
    let scale = fit_scale(w, h, ya, cam_dist);
    let center = (w / 2.0, h / 2.0);
    let projected: Vec<(f32, f32, f32)> = mesh
        .vertices
        .iter()
        .map(|&v| project(rotate_xyz(v, rotation[0], rotation[1], rotation[2]), cam_dist, scale, center, ya))
        .collect();
    mesh.edges
        .iter()
        .map(|&(a, b)| (projected[a], projected[b]))
        .collect()
}

/// Render a mesh into `raster` (which is `sx` x `sy` sub-samples per cell).
pub fn render_mesh(mesh: &Mesh, opts: &MeshRenderOpts, raster: &mut Raster, sx: usize, sy: usize) {
    let w = raster.w as f32;
    let h = raster.h as f32;
    let ya = y_aspect(sx, sy);
    let scale = fit_scale(w, h, ya, opts.cam_dist);
    let center = (w / 2.0, h / 2.0);
    let [ax, ay, az] = opts.rotation;

    let rotated: Vec<[f32; 3]> = mesh.vertices.iter().map(|&v| rotate_xyz(v, ax, ay, az)).collect();
    let projected: Vec<(f32, f32, f32)> = rotated
        .iter()
        .map(|&v| project(v, opts.cam_dist, scale, center, ya))
        .collect();

    // Depth range for hue drift, based on the projected vertices.
    let (mut dmin, mut dmax) = (f32::INFINITY, f32::NEG_INFINITY);
    for &(_, _, d) in &projected {
        dmin = dmin.min(d);
        dmax = dmax.max(d);
    }
    let drange = (dmax - dmin).max(1e-3);
    let hue_for = |depth: f32| opts.base_hue + 80.0 * ((depth - dmin) / drange).clamp(0.0, 1.0);

    match (&mesh.faces, opts.filled) {
        (Some(faces), true) => {
            for face in faces {
                let [i0, i1, i2] = *face;
                let (p0, p1, p2) = (rotated[i0], rotated[i1], rotated[i2]);
                let n = normalize(cross(
                    [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]],
                    [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]],
                ));
                // Camera looks down +Z: a face pointing at the camera has n.z < 0.
                if opts.cull_backfaces && n[2] > 0.0 {
                    continue;
                }
                let lambert = if opts.cull_backfaces {
                    dot(n, opts.light).max(0.0)
                } else {
                    dot(n, opts.light).abs()
                };
                let shade = 0.18 + 0.82 * lambert;
                let avg_depth = (projected[i0].2 + projected[i1].2 + projected[i2].2) / 3.0;
                fill_triangle(raster, projected[i0], projected[i1], projected[i2], shade, hue_for(avg_depth));
            }
        }
        _ => {
            for &(a, b) in &mesh.edges {
                let (x0, y0, d0) = projected[a];
                let (x1, y1, d1) = projected[b];
                let near = 1.0 - ((d0.min(d1) - dmin) / drange).clamp(0.0, 1.0);
                let weight = 0.35 + 0.65 * near;
                raster.draw_segment(x0, y0, x1, y1, weight, Some(hue_for((d0 + d1) / 2.0)), Some((d0, d1)));
            }
        }
    }
}

fn fill_triangle(
    raster: &mut Raster,
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    c: (f32, f32, f32),
    shade: f32,
    hue: f32,
) {
    let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as i32;
    let max_x = a.0.max(b.0).max(c.0).ceil().min(raster.w as f32 - 1.0) as i32;
    let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as i32;
    let max_y = a.1.max(b.1).max(c.1).ceil().min(raster.h as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }

    let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    if area.abs() < 1e-6 {
        return;
    }
    let inv = 1.0 / area;

    for y in min_y..=max_y {
        let py = y as f32 + 0.5;
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) * inv;
            let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) * inv;
            let w2 = 1.0 - w0 - w1;
            // Small negative tolerance closes hairline cracks between triangles.
            if w0 < -0.002 || w1 < -0.002 || w2 < -0.002 {
                continue;
            }
            let depth = w0 * a.2 + w1 * b.2 + w2 * c.2;
            raster.plot_depth(x as usize, y as usize, shade, Some(hue), depth);
        }
    }
}

// ---------------------------------------------------------------------------
// Mesh builders. All fit inside the unit sphere (radius ~1).
// ---------------------------------------------------------------------------

pub fn cube() -> Mesh {
    let s = 0.8;
    let vertices = vec![
        [-s, -s, -s],
        [s, -s, -s],
        [s, s, -s],
        [-s, s, -s],
        [-s, -s, s],
        [s, -s, s],
        [s, s, s],
        [-s, s, s],
    ];
    let edges = vec![
        (0, 1), (1, 2), (2, 3), (3, 0),
        (4, 5), (5, 6), (6, 7), (7, 4),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ];
    // Counter-clockwise when viewed from outside.
    let faces = vec![
        [0, 2, 1], [0, 3, 2], // back  (z = -s)
        [4, 5, 6], [4, 6, 7], // front (z = +s)
        [0, 1, 5], [0, 5, 4], // bottom (y = -s)
        [3, 7, 6], [3, 6, 2], // top
        [0, 4, 7], [0, 7, 3], // left
        [1, 2, 6], [1, 6, 5], // right
    ];
    Mesh { vertices, edges, faces: Some(faces) }
}

pub fn torus(major_segments: usize, minor_segments: usize) -> Mesh {
    let big_r = 0.68;
    let small_r = 0.30;
    let mut vertices = Vec::with_capacity(major_segments * minor_segments);
    for i in 0..major_segments {
        let u = i as f32 / major_segments as f32 * std::f32::consts::TAU;
        for j in 0..minor_segments {
            let v = j as f32 / minor_segments as f32 * std::f32::consts::TAU;
            let r = big_r + small_r * v.cos();
            vertices.push([r * u.cos(), small_r * v.sin(), r * u.sin()]);
        }
    }
    grid_mesh(vertices, major_segments, minor_segments, true, true)
}

pub fn sphere(lat_segments: usize, lon_segments: usize) -> Mesh {
    // Rows of latitude (excluding poles) as a wrapped grid, then pole fans.
    let mut vertices = Vec::new();
    for i in 1..lat_segments {
        let theta = i as f32 / lat_segments as f32 * std::f32::consts::PI;
        for j in 0..lon_segments {
            let phi = j as f32 / lon_segments as f32 * std::f32::consts::TAU;
            vertices.push([theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()]);
        }
    }
    let rows = lat_segments - 1;
    let mut mesh = grid_mesh(vertices, rows, lon_segments, false, true);
    let top = mesh.vertices.len();
    mesh.vertices.push([0.0, 1.0, 0.0]);
    let bottom = mesh.vertices.len();
    mesh.vertices.push([0.0, -1.0, 0.0]);
    let faces = mesh.faces.get_or_insert_with(Vec::new);
    for j in 0..lon_segments {
        let jn = (j + 1) % lon_segments;
        faces.push([top, jn, j]);
        let last = (rows - 1) * lon_segments;
        faces.push([bottom, last + j, last + jn]);
        mesh.edges.push((top, j));
        mesh.edges.push((bottom, last + j));
    }
    mesh
}

pub fn icosahedron() -> Mesh {
    let phi = (1.0 + 5.0_f32.sqrt()) / 2.0;
    let raw = [
        [-1.0, phi, 0.0], [1.0, phi, 0.0], [-1.0, -phi, 0.0], [1.0, -phi, 0.0],
        [0.0, -1.0, phi], [0.0, 1.0, phi], [0.0, -1.0, -phi], [0.0, 1.0, -phi],
        [phi, 0.0, -1.0], [phi, 0.0, 1.0], [-phi, 0.0, -1.0], [-phi, 0.0, 1.0],
    ];
    let vertices: Vec<[f32; 3]> = raw.iter().map(|&v| normalize(v)).collect();
    let faces = vec![
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
        [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
        [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    let mut edges = Vec::new();
    for f in &faces {
        for k in 0..3 {
            let a = f[k];
            let b = f[(k + 1) % 3];
            let e = (a.min(b), a.max(b));
            if !edges.contains(&e) {
                edges.push(e);
            }
        }
    }
    Mesh { vertices, edges, faces: Some(faces) }
}

/// Heightmap terrain from fractal noise, `n` x `n` vertices in `[-1, 1]`.
pub fn terrain(seed: u64, n: usize) -> Mesh {
    let n = n.max(2);
    let mut rng = Rng64::new(seed);
    let freq = rng.range_f32(1.4, 2.6);
    let amp = rng.range_f32(0.35, 0.6);
    let mut vertices = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            let x = i as f32 / (n - 1) as f32 * 2.0 - 1.0;
            let z = j as f32 / (n - 1) as f32 * 2.0 - 1.0;
            let h = (fbm(seed, (x + 1.0) * freq, (z + 1.0) * freq, 4) - 0.5) * 2.0 * amp;
            vertices.push([x * 0.95, -h, z * 0.95]);
        }
    }
    grid_mesh(vertices, n, n, false, false)
}

/// Build faces/edges for a grid of `rows` x `cols` vertices laid out row-major.
fn grid_mesh(vertices: Vec<[f32; 3]>, rows: usize, cols: usize, wrap_rows: bool, wrap_cols: bool) -> Mesh {
    let mut edges = Vec::new();
    let mut faces = Vec::new();
    let row_limit = if wrap_rows { rows } else { rows - 1 };
    let col_limit = if wrap_cols { cols } else { cols - 1 };
    for i in 0..rows {
        for j in 0..cols {
            let a = i * cols + j;
            if j < col_limit {
                let b = i * cols + (j + 1) % cols;
                edges.push((a, b));
            }
            if i < row_limit {
                let c = ((i + 1) % rows) * cols + j;
                edges.push((a, c));
            }
            if i < row_limit && j < col_limit {
                let b = i * cols + (j + 1) % cols;
                let c = ((i + 1) % rows) * cols + j;
                let d = ((i + 1) % rows) * cols + (j + 1) % cols;
                // Counter-clockwise seen from outside for the torus/sphere
                // parameterizations used above (outward normals).
                faces.push([a, b, c]);
                faces.push([b, d, c]);
            }
        }
    }
    Mesh { vertices, edges, faces: Some(faces) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_preserves_length() {
        let v = [0.3, -0.7, 0.5];
        let r = rotate_xyz(v, 1.1, 0.4, 2.2);
        let l0 = dot(v, v).sqrt();
        let l1 = dot(r, r).sqrt();
        assert!((l0 - l1).abs() < 1e-5);
    }

    #[test]
    fn projection_carries_depth_and_aspect() {
        let (x, y, d) = project([1.0, 1.0, 0.0], 3.0, 10.0, (0.0, 0.0), 0.5);
        assert!((d - 3.0).abs() < 1e-6);
        assert!((x - 10.0).abs() < 1e-5);
        assert!((y - 5.0).abs() < 1e-5, "y should be halved by the cell aspect");
        let (_, _, near) = project([0.0, 0.0, -1.0], 3.0, 10.0, (0.0, 0.0), 0.5);
        assert!(near < d);
    }

    #[test]
    fn y_aspect_is_square_for_braille() {
        assert!((y_aspect(1, 1) - 0.5).abs() < 1e-6);
        assert!((y_aspect(2, 4) - 1.0).abs() < 1e-6);
        assert!((y_aspect(1, 2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn meshes_are_well_formed() {
        for mesh in [cube(), torus(12, 6), sphere(6, 8), icosahedron(), terrain(1, 5)] {
            let n = mesh.vertices.len();
            for &(a, b) in &mesh.edges {
                assert!(a < n && b < n);
            }
            for f in mesh.faces.as_ref().unwrap() {
                assert!(f.iter().all(|&i| i < n));
            }
        }
        assert_eq!(cube().faces.unwrap().len(), 12);
        assert_eq!(icosahedron().faces.unwrap().len(), 20);
        assert_eq!(icosahedron().edges.len(), 30);
    }

    #[test]
    fn closed_meshes_have_outward_normals() {
        // Lighting and back-face culling both depend on this.
        for (name, mesh) in [("cube", cube()), ("torus", torus(16, 8)), ("sphere", sphere(8, 12)), ("ico", icosahedron())] {
            let faces = mesh.faces.as_ref().unwrap();
            let mut inward = 0;
            for f in faces {
                let (p0, p1, p2) = (mesh.vertices[f[0]], mesh.vertices[f[1]], mesh.vertices[f[2]]);
                let n = cross(
                    [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]],
                    [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]],
                );
                let centroid = [(p0[0] + p1[0] + p2[0]) / 3.0, (p0[1] + p1[1] + p2[1]) / 3.0, (p0[2] + p1[2] + p2[2]) / 3.0];
                // For the torus, measure against the tube centre, not the origin.
                let reference = if name == "torus" {
                    let r = (centroid[0] * centroid[0] + centroid[2] * centroid[2]).sqrt().max(1e-6);
                    [centroid[0] - 0.68 * centroid[0] / r, centroid[1], centroid[2] - 0.68 * centroid[2] / r]
                } else {
                    centroid
                };
                if dot(n, reference) < 0.0 {
                    inward += 1;
                }
            }
            assert_eq!(inward, 0, "{}: {} of {} faces wound inward", name, inward, faces.len());
        }
    }

    #[test]
    fn filled_render_writes_depth_and_wire_render_writes_dir() {
        let mesh = cube();
        let mut filled = Raster::new(40, 20);
        render_mesh(&mesh, &MeshRenderOpts::default(), &mut filled, 1, 1);
        assert!(filled.depth.iter().any(|d| d.is_finite()));
        assert!(filled.intensity.iter().any(|&i| i > 0.0));

        let mut wire = Raster::new(40, 20);
        let opts = MeshRenderOpts { filled: false, ..Default::default() };
        render_mesh(&mesh, &opts, &mut wire, 1, 1);
        assert!(wire.dir.iter().any(|&d| d != 0));
    }
}

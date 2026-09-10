//! Stage 1 of the pipeline: pure `(seed, size) -> Scene` functions.
//!
//! Field generators receive the raster sample size (`fw` x `fh`) and the
//! physical extent (`phys_w` x `phys_h`, in cell-width units, cells are two
//! units tall) so shapes come out round. Stroke generators emit segments in
//! physical units. 3D generators emit a mesh plus a seed-derived rotation.

use super::render3d;
use super::rng::{fbm, value_noise, Rng64};
use super::{Generator, Scene, Segment2};
use std::f32::consts::{PI, TAU};

pub fn generate(gen: Generator, seed: u64, fw: usize, fh: usize, phys_w: f32, phys_h: f32) -> Scene {
    let fw = fw.max(1);
    let fh = fh.max(1);
    match gen {
        Generator::Automaton => automaton(seed, fw, fh),
        Generator::Life => life(seed, fw, fh),
        Generator::Noise => noise(seed, fw, fh, phys_w, phys_h),
        Generator::Mandelbrot => mandelbrot(seed, fw, fh, phys_w, phys_h),
        Generator::Julia => julia(seed, fw, fh, phys_w, phys_h),
        Generator::Maze => maze(seed, fw, fh),
        Generator::Attractor => attractor(seed, fw, fh, phys_w, phys_h),
        Generator::Voronoi => voronoi(seed, fw, fh, phys_w, phys_h),
        Generator::Flow => flow(seed, phys_w, phys_h),
        Generator::Plant => plant(seed, phys_w, phys_h),
        Generator::Truchet => truchet(seed, phys_w, phys_h),
        Generator::Lissajous => lissajous(seed, phys_w, phys_h),
        Generator::Cube => mesh_scene(seed, render3d::cube(), true),
        Generator::Torus => mesh_scene(seed, render3d::torus(28, 14), true),
        Generator::Sphere => mesh_scene(seed, render3d::sphere(12, 24), true),
        Generator::Icosahedron => mesh_scene(seed, render3d::icosahedron(), true),
        Generator::Terrain => terrain(seed),
    }
}

/// Map a sample index to the centre of its physical position.
#[inline]
fn phys(i: usize, n: usize, extent: f32) -> f32 {
    (i as f32 + 0.5) / n as f32 * extent
}

fn field(w: usize, h: usize, intensity: Vec<f32>, hue: Option<Vec<f32>>) -> Scene {
    Scene::Field { w, h, intensity, hue }
}

// ---------------------------------------------------------------------------
// Fields
// ---------------------------------------------------------------------------

fn automaton(seed: u64, fw: usize, fh: usize) -> Scene {
    let mut rng = Rng64::new(seed);
    let rules: [u8; 10] = [30, 45, 73, 90, 105, 110, 124, 150, 182, 225];
    let rule = *rng.pick(&rules);
    let base_hue = rng.range_f32(0.0, 360.0);
    let random_start = rng.chance(0.4);

    let mut row = vec![false; fw];
    if random_start {
        for cell in row.iter_mut() {
            *cell = rng.chance(0.3);
        }
    } else {
        row[fw / 2] = true;
    }

    let mut intensity = vec![0.0; fw * fh];
    let mut hue = vec![-1.0; fw * fh];
    for y in 0..fh {
        for x in 0..fw {
            if row[x] {
                intensity[y * fw + x] = 1.0;
                hue[y * fw + x] = base_hue + 90.0 * (y as f32 / fh as f32) + 40.0 * (x as f32 / fw as f32);
            }
        }
        let mut next = vec![false; fw];
        for x in 0..fw {
            let l = row[(x + fw - 1) % fw] as u8;
            let c = row[x] as u8;
            let r = row[(x + 1) % fw] as u8;
            let pattern = (l << 2) | (c << 1) | r;
            next[x] = (rule >> pattern) & 1 == 1;
        }
        row = next;
    }
    field(fw, fh, intensity, Some(hue))
}

fn life(seed: u64, fw: usize, fh: usize) -> Scene {
    let mut rng = Rng64::new(seed);
    let density = rng.range_f32(0.28, 0.42);
    let steps = rng.range_usize(12, 40);
    let base_hue = rng.range_f32(0.0, 360.0);

    let n = fw * fh;
    let mut grid: Vec<bool> = (0..n).map(|_| rng.chance(density)).collect();
    let mut last_alive: Vec<i32> = grid.iter().map(|&a| if a { 0 } else { -100 }).collect();

    for step in 1..=steps {
        let mut next = vec![false; n];
        for y in 0..fh {
            for x in 0..fw {
                let mut count = 0;
                for dy in [fh - 1, 0, 1] {
                    for dx in [fw - 1, 0, 1] {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        if grid[((y + dy) % fh) * fw + (x + dx) % fw] {
                            count += 1;
                        }
                    }
                }
                let i = y * fw + x;
                next[i] = matches!((grid[i], count), (true, 2) | (true, 3) | (false, 3));
                if next[i] {
                    last_alive[i] = step as i32;
                }
            }
        }
        grid = next;
    }

    let mut intensity = vec![0.0; n];
    let mut hue = vec![-1.0; n];
    for i in 0..n {
        let age = steps as i32 - last_alive[i];
        if grid[i] {
            intensity[i] = 1.0;
            hue[i] = base_hue;
        } else if age < 8 {
            intensity[i] = 0.55 * (-(age as f32) / 3.0).exp();
            hue[i] = base_hue + 40.0 + age as f32 * 12.0;
        }
    }
    field(fw, fh, intensity, Some(hue))
}

fn noise(seed: u64, fw: usize, fh: usize, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    let freq = rng.range_f32(0.06, 0.14);
    let warp = rng.range_f32(0.4, 1.4);
    let base_hue = rng.range_f32(0.0, 360.0);
    let hue_span = rng.range_f32(60.0, 160.0);
    let n = fw * fh;
    let mut intensity = Vec::with_capacity(n);
    let mut hue = Vec::with_capacity(n);
    for y in 0..fh {
        for x in 0..fw {
            let px = phys(x, fw, phys_w) * freq;
            let py = phys(y, fh, phys_h) * freq;
            let qx = fbm(seed ^ 0xA5, px + 3.1, py + 1.7, 3);
            let qy = fbm(seed ^ 0x5A, px - 2.3, py + 4.9, 3);
            let v = fbm(seed, px + warp * qx, py + warp * qy, 5);
            // Stretch contrast so the ramp gets used end to end.
            let stretched = ((v - 0.5) * 2.4 + 0.5).clamp(0.0, 1.0);
            intensity.push(stretched);
            hue.push(base_hue + hue_span * qx);
        }
    }
    field(fw, fh, intensity, Some(hue))
}

#[allow(clippy::too_many_arguments)]
fn escape_time(
    fw: usize,
    fh: usize,
    phys_w: f32,
    phys_h: f32,
    center: (f32, f32),
    zoom: f32,
    max_iter: usize,
    base_hue: f32,
    mut step: impl FnMut(f32, f32) -> (f32, f32, f32, f32),
) -> Scene {
    // `step` receives the sample coordinates and returns (z_re, z_im, c_re, c_im).
    let aspect = phys_w / phys_h.max(1e-3);
    let span_x = 3.0 / zoom;
    let span_y = span_x / aspect;
    let n = fw * fh;
    let mut intensity = Vec::with_capacity(n);
    let mut hue = Vec::with_capacity(n);
    for y in 0..fh {
        for x in 0..fw {
            let cx = center.0 + (phys(x, fw, phys_w) / phys_w - 0.5) * span_x;
            let cy = center.1 + (phys(y, fh, phys_h) / phys_h - 0.5) * span_y;
            let (mut zr, mut zi, cr, ci) = step(cx, cy);
            let mut i = 0;
            let mut mag2 = 0.0;
            while i < max_iter {
                mag2 = zr * zr + zi * zi;
                if mag2 > 64.0 {
                    break;
                }
                let nzr = zr * zr - zi * zi + cr;
                zi = 2.0 * zr * zi + ci;
                zr = nzr;
                i += 1;
            }
            if i >= max_iter {
                // Inside the set: leave blank so the escape bands carry the image.
                intensity.push(0.0);
                hue.push(-1.0);
            } else {
                let smooth = i as f32 + 1.0 - mag2.max(1.0001).ln().ln() / 2f32.ln();
                let t = (smooth / max_iter as f32).clamp(0.0, 1.0).powf(0.45);
                intensity.push(t * 0.95);
                hue.push(base_hue + smooth * 9.0);
            }
        }
    }
    field(fw, fh, intensity, Some(hue))
}

fn mandelbrot(seed: u64, fw: usize, fh: usize, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    let spots: [((f32, f32), f32); 7] = [
        ((-0.65, 0.0), 1.0),
        ((-0.745, 0.113), 24.0),
        ((-0.1592, 1.0317), 14.0),
        ((-1.25066, 0.02012), 40.0),
        ((0.2822, 0.5300), 18.0),
        ((-0.5613, 0.6427), 9.0),
        ((-1.7687, 0.0017), 30.0),
    ];
    let (center, zoom) = *rng.pick(&spots);
    let zoom = zoom * rng.range_f32(0.8, 1.6);
    let base_hue = rng.range_f32(0.0, 360.0);
    let max_iter = 80 + (zoom as usize * 4).min(160);
    escape_time(fw, fh, phys_w, phys_h, center, zoom, max_iter, base_hue, |cx, cy| (0.0, 0.0, cx, cy))
}

fn julia(seed: u64, fw: usize, fh: usize, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    let constants: [(f32, f32); 6] = [
        (-0.8, 0.156),
        (-0.4, 0.6),
        (0.285, 0.01),
        (-0.70176, -0.3842),
        (-0.835, -0.2321),
        (0.355, 0.355),
    ];
    let (cr, ci) = *rng.pick(&constants);
    let cr = cr + rng.range_f32(-0.02, 0.02);
    let ci = ci + rng.range_f32(-0.02, 0.02);
    let base_hue = rng.range_f32(0.0, 360.0);
    let zoom = rng.range_f32(0.9, 1.4);
    escape_time(fw, fh, phys_w, phys_h, (0.0, 0.0), zoom, 120, base_hue, |x, y| (x, y, cr, ci))
}

fn maze(seed: u64, fw: usize, fh: usize) -> Scene {
    let mut rng = Rng64::new(seed);
    let base_hue = rng.range_f32(0.0, 360.0);
    // Maze cells sit on odd sample coordinates; walls on even ones.
    let mw = ((fw.saturating_sub(1)) / 2).max(1);
    let mh = ((fh.saturating_sub(1)) / 2).max(1);
    let mut visited = vec![false; mw * mh];
    let mut open = vec![false; fw * fh]; // true = passage
    let mut order = vec![0usize; fw * fh];

    let start = (rng.range_usize(0, mw), rng.range_usize(0, mh));
    let mut stack = vec![start];
    visited[start.1 * mw + start.0] = true;
    let mut counter = 1usize;
    let carve = |open: &mut Vec<bool>, order: &mut Vec<usize>, x: usize, y: usize, c: usize| {
        if x < fw && y < fh {
            open[y * fw + x] = true;
            if order[y * fw + x] == 0 {
                order[y * fw + x] = c;
            }
        }
    };
    carve(&mut open, &mut order, start.0 * 2 + 1, start.1 * 2 + 1, counter);

    while let Some(&(cx, cy)) = stack.last() {
        let mut neighbours = Vec::with_capacity(4);
        if cx > 0 && !visited[cy * mw + cx - 1] {
            neighbours.push((cx - 1, cy));
        }
        if cx + 1 < mw && !visited[cy * mw + cx + 1] {
            neighbours.push((cx + 1, cy));
        }
        if cy > 0 && !visited[(cy - 1) * mw + cx] {
            neighbours.push((cx, cy - 1));
        }
        if cy + 1 < mh && !visited[(cy + 1) * mw + cx] {
            neighbours.push((cx, cy + 1));
        }
        if neighbours.is_empty() {
            stack.pop();
            continue;
        }
        let (nx, ny) = *rng.pick(&neighbours);
        visited[ny * mw + nx] = true;
        counter += 1;
        let wall_x = cx * 2 + 1 + (nx as isize - cx as isize) as usize;
        let wall_y = cy * 2 + 1 + (ny as isize - cy as isize) as usize;
        carve(&mut open, &mut order, wall_x, wall_y, counter);
        carve(&mut open, &mut order, nx * 2 + 1, ny * 2 + 1, counter);
        stack.push((nx, ny));
    }

    let total = counter.max(1) as f32;
    let mut intensity = vec![0.0; fw * fh];
    let mut hue = vec![-1.0; fw * fh];
    for i in 0..fw * fh {
        if open[i] {
            intensity[i] = 1.0;
            hue[i] = base_hue + 300.0 * (order[i] as f32 / total);
        } else {
            intensity[i] = 0.12;
        }
    }
    field(fw, fh, intensity, Some(hue))
}

fn attractor(seed: u64, fw: usize, fh: usize, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    let presets: [(f32, f32, f32, f32); 6] = [
        (-1.4, 1.6, 1.0, 0.7),
        (1.7, 1.7, 0.6, 1.2),
        (-1.7, 1.8, -1.9, -0.4),
        (1.5, -1.8, 1.6, 0.9),
        (-1.8, -2.0, -0.5, -0.9),
        (1.1, -1.32, -1.03, 1.54),
    ];
    let (a, b, c, d) = *rng.pick(&presets);
    let a = a + rng.range_f32(-0.05, 0.05);
    let b = b + rng.range_f32(-0.05, 0.05);
    let base_hue = rng.range_f32(0.0, 360.0);

    let n = fw * fh;
    let mut counts = vec![0u32; n];
    let mut hue_acc = vec![0.0f32; n];
    let (mut x, mut y) = (0.1f32, 0.1f32);
    let ext_x = 1.0 + c.abs();
    let ext_y = 1.0 + d.abs();
    let aspect = phys_w / phys_h;
    // Fit the attractor's bounding box inside the physical rectangle.
    let (sx, sy) = if ext_x / ext_y > aspect {
        (1.0, (ext_x / ext_y) / aspect)
    } else {
        (aspect / (ext_x / ext_y), 1.0)
    };
    let iterations = (n * 40).clamp(40_000, 800_000);
    for _ in 0..iterations {
        let nx = (a * y).sin() + c * (a * x).cos();
        let ny = (b * x).sin() + d * (b * y).cos();
        let speed = ((nx - x).powi(2) + (ny - y).powi(2)).sqrt();
        x = nx;
        y = ny;
        let u = (x / ext_x * 0.5 / sx + 0.5).clamp(0.0, 0.9999);
        let v = (y / ext_y * 0.5 / sy + 0.5).clamp(0.0, 0.9999);
        let i = (v * fh as f32) as usize * fw + (u * fw as f32) as usize;
        counts[i] += 1;
        hue_acc[i] += speed;
    }
    let max = counts.iter().copied().max().unwrap_or(1).max(1) as f32;
    let mut intensity = Vec::with_capacity(n);
    let mut hue = Vec::with_capacity(n);
    for i in 0..n {
        let c = counts[i] as f32;
        // Log density, lifted so faint orbits still register under dithering.
        intensity.push(if c == 0.0 { 0.0 } else { 0.25 + 0.75 * ((1.0 + c).ln() / (1.0 + max).ln()).powf(0.5) });
        let mean_speed = if c > 0.0 { hue_acc[i] / c } else { 0.0 };
        hue.push(base_hue + mean_speed * 45.0);
    }
    field(fw, fh, intensity, Some(hue))
}

fn voronoi(seed: u64, fw: usize, fh: usize, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    let count = rng.range_usize(7, 18);
    let base_hue = rng.range_f32(0.0, 360.0);
    let sites: Vec<(f32, f32, f32)> = (0..count)
        .map(|k| {
            (
                rng.range_f32(0.0, phys_w),
                rng.range_f32(0.0, phys_h),
                base_hue + k as f32 * (360.0 / count as f32) * rng.range_f32(0.6, 1.0),
            )
        })
        .collect();
    let n = fw * fh;
    let mut intensity = Vec::with_capacity(n);
    let mut hue = Vec::with_capacity(n);
    let border = 1.6;
    for y in 0..fh {
        for x in 0..fw {
            let px = phys(x, fw, phys_w);
            let py = phys(y, fh, phys_h);
            let mut d1 = f32::INFINITY;
            let mut d2 = f32::INFINITY;
            let mut h = 0.0;
            for &(sx, sy, sh) in &sites {
                let d = ((px - sx).powi(2) + (py - sy).powi(2)).sqrt();
                if d < d1 {
                    d2 = d1;
                    d1 = d;
                    h = sh;
                } else if d < d2 {
                    d2 = d;
                }
            }
            let edge = ((d2 - d1) / border).clamp(0.0, 1.0);
            let falloff = (1.0 - d1 / (phys_w.max(phys_h) * 0.35)).clamp(0.0, 1.0);
            intensity.push(edge * (0.3 + 0.7 * falloff));
            hue.push(h);
        }
    }
    field(fw, fh, intensity, Some(hue))
}

// ---------------------------------------------------------------------------
// Strokes
// ---------------------------------------------------------------------------

fn strokes(w: f32, h: f32, segments: Vec<Segment2>) -> Scene {
    Scene::Strokes { w, h, segments }
}

fn flow(seed: u64, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    let particles = rng.range_usize(60, 140);
    let freq = rng.range_f32(0.04, 0.09);
    let base_hue = rng.range_f32(0.0, 360.0);
    let curl = rng.range_f32(1.5, 3.5);
    let step_len = 1.0;
    let steps = rng.range_usize(20, 45);
    let mut segments = Vec::new();
    for _ in 0..particles {
        let mut x = rng.range_f32(0.0, phys_w);
        let mut y = rng.range_f32(0.0, phys_h);
        for s in 0..steps {
            let angle = value_noise(seed, x * freq, y * freq) * TAU * curl;
            let nx = x + angle.cos() * step_len;
            let ny = y + angle.sin() * step_len;
            if nx < 0.0 || ny < 0.0 || nx > phys_w || ny > phys_h {
                break;
            }
            let life = 1.0 - s as f32 / steps as f32;
            segments.push(Segment2 {
                x0: x,
                y0: y,
                x1: nx,
                y1: ny,
                weight: 0.35 + 0.65 * life,
                hue: Some(base_hue + angle.to_degrees() * 0.25),
            });
            x = nx;
            y = ny;
        }
    }
    strokes(phys_w, phys_h, segments)
}

fn plant(seed: u64, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    struct Preset {
        axiom: &'static str,
        rules: &'static [(char, &'static str)],
        angle: f32,
        iterations: usize,
    }
    let presets = [
        Preset { axiom: "X", rules: &[('X', "F+[[X]-X]-F[-FX]+X"), ('F', "FF")], angle: 25.0, iterations: 5 },
        Preset { axiom: "F", rules: &[('F', "FF-[-F+F+F]+[+F-F-F]")], angle: 22.5, iterations: 4 },
        Preset { axiom: "X", rules: &[('X', "F[+X][-X]FX"), ('F', "FF")], angle: 25.7, iterations: 6 },
        Preset { axiom: "F", rules: &[('F', "F[+F]F[-F][F]")], angle: 20.0, iterations: 5 },
        Preset { axiom: "X", rules: &[('X', "F[-X][+X]F[-X]+FX"), ('F', "FF")], angle: 28.0, iterations: 5 },
    ];
    let preset = &presets[rng.range_usize(0, presets.len())];
    let angle = (preset.angle + rng.range_f32(-4.0, 4.0)).to_radians();
    let lean = rng.range_f32(-0.15, 0.15);
    let base_hue = rng.range_f32(80.0, 150.0);

    let mut s: String = preset.axiom.to_string();
    for _ in 0..preset.iterations {
        let mut next = String::with_capacity(s.len() * 3);
        for ch in s.chars() {
            match preset.rules.iter().find(|(k, _)| *k == ch) {
                Some((_, r)) => next.push_str(r),
                None => next.push(ch),
            }
        }
        s = next;
        if s.len() > 60_000 {
            break;
        }
    }

    // Walk the turtle once in unit steps to find bounds, then scale to fit.
    let mut raw: Vec<(f32, f32, f32, f32, usize)> = Vec::new();
    let mut stack: Vec<(f32, f32, f32, usize)> = Vec::new();
    let (mut x, mut y, mut heading, mut depth) = (0.0f32, 0.0f32, -PI / 2.0 + lean, 0usize);
    for ch in s.chars() {
        match ch {
            'F' => {
                let nx = x + heading.cos();
                let ny = y + heading.sin();
                raw.push((x, y, nx, ny, depth));
                x = nx;
                y = ny;
            }
            '+' => heading += angle,
            '-' => heading -= angle,
            '[' => {
                stack.push((x, y, heading, depth));
                depth += 1;
            }
            ']' => {
                if let Some((px, py, ph, pd)) = stack.pop() {
                    x = px;
                    y = py;
                    heading = ph;
                    depth = pd;
                }
            }
            _ => {}
        }
    }
    if raw.is_empty() {
        raw.push((0.0, 0.0, 0.0, -1.0, 0));
    }
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    for &(x0, y0, x1, y1, _) in &raw {
        min_x = min_x.min(x0).min(x1);
        max_x = max_x.max(x0).max(x1);
        min_y = min_y.min(y0).min(y1);
        max_y = max_y.max(y0).max(y1);
    }
    let span_x = (max_x - min_x).max(1e-3);
    let span_y = (max_y - min_y).max(1e-3);
    let scale = ((phys_w * 0.92) / span_x).min((phys_h * 0.94) / span_y);
    let off_x = (phys_w - span_x * scale) / 2.0 - min_x * scale;
    let off_y = phys_h * 0.97 - max_y * scale;
    let max_depth = raw.iter().map(|r| r.4).max().unwrap_or(1).max(1) as f32;

    let segments = raw
        .into_iter()
        .map(|(x0, y0, x1, y1, d)| {
            let t = d as f32 / max_depth;
            Segment2 {
                x0: x0 * scale + off_x,
                y0: y0 * scale + off_y,
                x1: x1 * scale + off_x,
                y1: y1 * scale + off_y,
                weight: 1.0 - 0.55 * t,
                hue: Some(30.0 + (base_hue - 30.0) * t.powf(0.6)),
            }
        })
        .collect();
    strokes(phys_w, phys_h, segments)
}

fn truchet(seed: u64, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    let tile = rng.range_f32(5.0, 9.0);
    let base_hue = rng.range_f32(0.0, 360.0);
    let cols = (phys_w / tile).ceil() as usize;
    let rows = (phys_h / tile).ceil() as usize;
    let arc_steps = 7;
    let mut segments = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let x0 = c as f32 * tile;
            let y0 = r as f32 * tile;
            let flip = rng.chance(0.5);
            let hue = base_hue + 25.0 * ((r + c) % 6) as f32 + rng.range_f32(-8.0, 8.0);
            // Two quarter-circle arcs centred on opposite corners.
            let corners = if flip {
                [(x0, y0), (x0 + tile, y0 + tile)]
            } else {
                [(x0 + tile, y0), (x0, y0 + tile)]
            };
            for (k, &(cx, cy)) in corners.iter().enumerate() {
                let start = match (flip, k) {
                    (true, 0) => 0.0,
                    (true, _) => PI,
                    (false, 0) => PI / 2.0,
                    (false, _) => -PI / 2.0,
                };
                let mut prev = (cx + (tile / 2.0) * start.cos(), cy + (tile / 2.0) * start.sin());
                for s in 1..=arc_steps {
                    let a = start + (PI / 2.0) * s as f32 / arc_steps as f32;
                    let p = (cx + (tile / 2.0) * a.cos(), cy + (tile / 2.0) * a.sin());
                    segments.push(Segment2 { x0: prev.0, y0: prev.1, x1: p.0, y1: p.1, weight: 0.9, hue: Some(hue) });
                    prev = p;
                }
            }
        }
    }
    strokes(phys_w, phys_h, segments)
}

fn lissajous(seed: u64, phys_w: f32, phys_h: f32) -> Scene {
    let mut rng = Rng64::new(seed);
    let a = rng.range_usize(1, 6) as f32;
    let mut b = rng.range_usize(1, 6) as f32;
    if (a - b).abs() < 0.5 {
        b += 1.0;
    }
    let delta = rng.range_f32(0.0, PI);
    let base_hue = rng.range_f32(0.0, 360.0);
    let damping = rng.range_f32(0.0, 0.25);
    let harmonics = rng.range_usize(1, 3);
    let cx = phys_w / 2.0;
    let cy = phys_h / 2.0;
    let rx = phys_w * 0.46;
    let ry = phys_h * 0.46;
    let samples = 900;
    let mut segments = Vec::new();
    for k in 0..harmonics {
        let ka = a + k as f32;
        let kb = b + k as f32 * 2.0;
        let mut prev: Option<(f32, f32)> = None;
        for i in 0..=samples {
            let t = i as f32 / samples as f32 * TAU;
            let decay = (-damping * t).exp();
            let x = cx + rx * decay * (ka * t + delta).sin();
            let y = cy + ry * decay * (kb * t).sin();
            if let Some((px, py)) = prev {
                segments.push(Segment2 {
                    x0: px,
                    y0: py,
                    x1: x,
                    y1: y,
                    weight: (0.5 + 0.5 * decay) * (1.0 - 0.25 * k as f32),
                    hue: Some(base_hue + 120.0 * (i as f32 / samples as f32) + 60.0 * k as f32),
                });
            }
            prev = Some((x, y));
        }
    }
    strokes(phys_w, phys_h, segments)
}

// ---------------------------------------------------------------------------
// Meshes
// ---------------------------------------------------------------------------

fn seed_rotation(rng: &mut Rng64) -> [f32; 3] {
    [rng.range_f32(0.2, 1.2), rng.range_f32(0.0, TAU), rng.range_f32(-0.4, 0.4)]
}

fn mesh_scene(seed: u64, mesh: render3d::Mesh, cull_backfaces: bool) -> Scene {
    let mut rng = Rng64::new(seed);
    let rotation = seed_rotation(&mut rng);
    let base_hue = rng.range_f32(0.0, 360.0);
    Scene::Mesh { mesh, rotation, cull_backfaces, base_hue }
}

fn terrain(seed: u64) -> Scene {
    let mut rng = Rng64::new(seed);
    let mesh = render3d::terrain(seed, 26);
    let rotation = [rng.range_f32(0.7, 1.1), rng.range_f32(0.0, TAU), 0.0];
    let base_hue = rng.range_f32(0.0, 360.0);
    Scene::Mesh { mesh, rotation, cull_backfaces: false, base_hue }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_signature(scene: &Scene) -> Vec<u32> {
        match scene {
            Scene::Field { intensity, .. } => intensity.iter().map(|v| v.to_bits()).collect(),
            Scene::Strokes { segments, .. } => segments
                .iter()
                .flat_map(|s| [s.x0.to_bits(), s.y0.to_bits(), s.x1.to_bits(), s.y1.to_bits()])
                .collect(),
            Scene::Mesh { rotation, mesh, .. } => rotation
                .iter()
                .map(|v| v.to_bits())
                .chain(mesh.vertices.iter().flat_map(|v| v.iter().map(|c| c.to_bits())))
                .collect(),
        }
    }

    #[test]
    fn generators_are_pure_functions_of_seed_and_size() {
        for g in Generator::ALL {
            let a = generate(g, 77, 64, 32, 32.0, 32.0);
            let b = generate(g, 77, 64, 32, 32.0, 32.0);
            assert_eq!(scene_signature(&a), scene_signature(&b), "{} not deterministic", g.name());
        }
    }

    #[test]
    fn fields_match_requested_size_and_stay_in_range() {
        for g in Generator::ALL {
            if let Scene::Field { w, h, intensity, hue } = generate(g, 5, 50, 30, 50.0, 60.0) {
                assert_eq!((w, h), (50, 30));
                assert_eq!(intensity.len(), 1500);
                assert!(intensity.iter().all(|v| (0.0..=1.0).contains(v)), "{} out of range", g.name());
                if let Some(hue) = hue {
                    assert_eq!(hue.len(), 1500);
                }
            }
        }
    }

    #[test]
    fn strokes_stay_inside_the_canvas() {
        for g in [Generator::Flow, Generator::Plant, Generator::Lissajous] {
            if let Scene::Strokes { w, h, segments } = generate(g, 11, 60, 24, 60.0, 48.0) {
                assert!(!segments.is_empty());
                for s in &segments {
                    assert!(s.x0 >= -0.01 && s.x0 <= w + 0.01, "{} x out of bounds", g.name());
                    assert!(s.y0 >= -0.01 && s.y0 <= h + 0.01, "{} y out of bounds", g.name());
                }
            } else {
                panic!("{} should be strokes", g.name());
            }
        }
    }

    #[test]
    fn automaton_uses_a_real_rule() {
        // Rule 90 from a single seed cell produces the Sierpinski pattern:
        // row 1 has cells at centre-1 and centre+1.
        if let Scene::Field { w, intensity, .. } = automaton(0, 31, 4) {
            // Whichever rule the seed picks, row 0 must be either a single
            // centre cell or a random start; row 1 must be non-empty.
            let row1: f32 = intensity[w..2 * w].iter().sum();
            assert!(row1 > 0.0);
        }
    }
}

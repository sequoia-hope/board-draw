//! Triangles for the capsule shader: every vertex carries the segment it belongs to and its
//! radius, and the fragment shader cuts the quad to that capsule. A negative radius marks a
//! plain fill.

use crate::{Prim, Rgba, Xf};

/// Floats per vertex: position (2), segment ends (2 + 2), radius (1), colour (4).
pub const STRIDE: usize = 11;

/// A growing vertex buffer. `aa` is the anti-aliasing margin a capsule's quad is grown by, one
/// screen pixel in millimetres.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub v: Vec<f32>,
    pub aa: f32,
}

impl Mesh {
    pub fn new(aa: f32) -> Mesh {
        Mesh { v: Vec::new(), aa }
    }

    pub fn with_capacity(aa: f32, floats: usize) -> Mesh {
        Mesh { v: Vec::with_capacity(floats), aa }
    }

    /// Vertices so far.
    pub fn len(&self) -> usize {
        self.v.len() / STRIDE
    }

    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }

    pub fn into_vec(self) -> Vec<f32> {
        self.v
    }

    pub fn vertex(&mut self, p: [f32; 2], a: [f32; 2], b: [f32; 2], r: f32, c: Rgba) {
        self.v.extend_from_slice(&[p[0], p[1], a[0], a[1], b[0], b[1], r, c[0], c[1], c[2], c[3]]);
    }

    /// A capsule of radius `r` from `a` to `b`: one quad, grown by the margin.
    pub fn segment(&mut self, a: [f32; 2], b: [f32; 2], r: f32, c: Rgba) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        let (ux, uy) = if len > 0.0 { (dx / len, dy / len) } else { (1.0, 0.0) };
        let e = r + self.aa;
        let (nx, ny) = (-uy * e, ux * e);
        let (tx, ty) = (ux * e, uy * e);
        let p0 = [a[0] - tx + nx, a[1] - ty + ny];
        let p1 = [a[0] - tx - nx, a[1] - ty - ny];
        let p2 = [b[0] + tx + nx, b[1] + ty + ny];
        let p3 = [b[0] + tx - nx, b[1] + ty - ny];
        for p in [p0, p1, p2, p2, p1, p3] {
            self.vertex(p, a, b, r, c);
        }
    }

    /// A disc of radius `r`: a capsule with both ends at `at`.
    pub fn disc(&mut self, at: [f32; 2], r: f32, c: Rgba) {
        self.segment(at, at, r, c);
    }

    pub fn polyline(&mut self, points: &[f32], r: f32, c: Rgba) {
        if points.len() == 2 {
            self.disc([points[0], points[1]], r, c);
        }
        for w in points.windows(4).step_by(2) {
            self.segment([w[0], w[1]], [w[2], w[3]], r, c);
        }
    }

    /// Dashes `dash` long with gaps as long, the pattern running on across corners.
    /// Each step ends either the current dash or gap (`left` drops to exactly zero) or the
    /// segment (`rest` does), so it always finishes; dashes too fine to see are drawn solid.
    pub fn dashed_polyline(&mut self, points: &[f32], r: f32, dash: f32, c: Rgba) {
        let total: f32 = points.windows(4).step_by(2).map(|w| ((w[2] - w[0]).powi(2) + (w[3] - w[1]).powi(2)).sqrt()).sum();
        if !(dash > 0.0 && dash.is_finite() && total.is_finite()) || total / dash > 10_000.0 {
            self.polyline(points, r, c);
            return;
        }
        let (mut on, mut left) = (true, dash);
        for w in points.windows(4).step_by(2) {
            let (a, b) = ([w[0], w[1]], [w[2], w[3]]);
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let at = |s: f32| [a[0] + (b[0] - a[0]) * s / len, a[1] + (b[1] - a[1]) * s / len];
            let (mut t, mut rest) = (0.0f32, len);
            while rest > 0.0 {
                let take = left.min(rest);
                if on {
                    self.segment(at(t), at(t + take), r, c);
                }
                t += take;
                rest -= take;
                left -= take;
                if left <= 0.0 {
                    on = !on;
                    left = dash;
                }
            }
        }
    }

    /// A closed ring through the points.
    pub fn ring(&mut self, points: &[f32], r: f32, c: Rgba) {
        let n = points.len() / 2;
        for i in 0..n {
            let j = (i + 1) % n;
            self.segment([points[2 * i], points[2 * i + 1]], [points[2 * j], points[2 * j + 1]], r, c);
        }
    }

    /// A closed ring, each side dashed on its own: dashes about `dash` long, evenly spread.
    pub fn dashed_ring(&mut self, points: &[f32], r: f32, dash: f32, c: Rgba) {
        let n = points.len() / 2;
        for i in 0..n {
            let j = (i + 1) % n;
            let (a, b) = ([points[2 * i], points[2 * i + 1]], [points[2 * j], points[2 * j + 1]]);
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let dashes = ((len / (dash * 1.8)).floor() as usize).max(1);
            for k in 0..dashes {
                let (t0, t1) = (k as f32 / dashes as f32, (k as f32 + 0.55) / dashes as f32);
                let at = |t: f32| [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                self.segment(at(t0), at(t1), r, c);
            }
        }
    }

    /// A fan from the first corner, each corner moved by `o`: for convex shapes.
    pub fn fill(&mut self, points: &[f32], o: [f32; 2], c: Rgba) {
        let n = points.len() / 2;
        let at = |i: usize| [points[2 * i] + o[0], points[2 * i + 1] + o[1]];
        for i in 1..n.saturating_sub(1) {
            for p in [at(0), at(i), at(i + 1)] {
                self.vertex(p, p, p, -1.0, c);
            }
        }
    }

    /// A convex polygon placed by `xf`.
    pub fn fill_xf(&mut self, points: &[f32], xf: &Xf, c: Rgba) {
        if xf.plain() {
            self.fill(points, xf.t, c);
        } else {
            self.fill(&xf.points(points), [0.0, 0.0], c);
        }
    }

    /// A shape placed by `xf`.
    pub fn prim(&mut self, p: &Prim, xf: &Xf, c: Rgba) {
        match p {
            Prim::Circle([x, y, r]) => self.disc(xf.apply([*x, *y]), *r, c),
            Prim::Poly(g) => self.fill_xf(g, xf, c),
        }
    }
}

/// Sixteen points round a circle.
pub fn circle_points(c: [f32; 2], r: f32) -> Vec<f32> {
    (0..16)
        .flat_map(|k| {
            let t = std::f32::consts::TAU * k as f32 / 16.0;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba = [1.0; 4];

    fn quads(m: &Mesh) -> usize {
        m.v.len() / (6 * STRIDE)
    }

    #[test]
    fn a_segment_is_one_quad_grown_by_its_radius_and_the_margin() {
        let mut m = Mesh::new(0.1);
        m.segment([0.0, 0.0], [2.0, 0.0], 0.5, WHITE);
        assert_eq!(m.len(), 6);
        // First corner: behind `a` and to its left by r + aa.
        assert_eq!(&m.v[..7], &[-0.6, 0.6, 0.0, 0.0, 2.0, 0.0, 0.5]);
        assert_eq!(&m.v[7..11], &WHITE);
    }

    #[test]
    fn a_fill_is_a_fan_with_a_negative_radius() {
        let mut m = Mesh::new(0.1);
        m.fill(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0], [10.0, 0.0], WHITE);
        assert_eq!(m.len(), 6);
        assert!(m.v.chunks_exact(STRIDE).all(|v| v[6] == -1.0 && v[0] == v[2] && v[0] == v[4] && v[0] >= 10.0));
    }

    #[test]
    fn dashes_run_on_across_corners() {
        // 2 mm then 1 mm at a 0.5 mm dash: on, off, on, off | on, off.
        let mut m = Mesh::new(0.0);
        m.dashed_polyline(&[0.0, 0.0, 2.0, 0.0, 2.0, 1.0], 0.1, 0.5, WHITE);
        assert_eq!(quads(&m), 3);
        // 0.25 mm then 0.75 mm: the first dash breaks over the corner and carries on 0.25 mm.
        let mut m = Mesh::new(0.0);
        m.dashed_polyline(&[0.0, 0.0, 0.25, 0.0, 0.25, 0.75], 0.1, 0.5, WHITE);
        assert_eq!(quads(&m), 2);
        assert_eq!(&m.v[6 * STRIDE + 4..6 * STRIDE + 6], &[0.25, 0.25], "second piece ends 0.25 mm up");
    }

    #[test]
    fn dashing_always_finishes() {
        let awkward = [0.1f32, 0.3, 1.0 / 3.0, 1e-3, 0.007, 12.345];
        for &dash in &awkward {
            let mut points = vec![0.0f32, 0.0];
            let mut x = 0.0f32;
            for k in 0..300 {
                x += 0.1 + k as f32 * 0.0137;
                points.extend([x, (k % 7) as f32 * 0.31]);
            }
            let mut m = Mesh::new(0.0);
            m.dashed_polyline(&points, 0.05, dash, WHITE);
            let total: f32 = points.windows(4).step_by(2).map(|w| ((w[2] - w[0]).powi(2) + (w[3] - w[1]).powi(2)).sqrt()).sum();
            assert!(quads(&m) <= (total / dash) as usize + 300 + 2, "dash {dash}: {} quads", quads(&m));
        }
    }

    #[test]
    fn dashes_too_fine_or_broken_are_drawn_solid() {
        let line = [0.0f32, 0.0, 1000.0, 0.0, 1000.0, 5.0];
        for dash in [0.0, -1.0, f32::NAN, f32::INFINITY, 1e-6] {
            let mut m = Mesh::new(0.0);
            m.dashed_polyline(&line, 0.1, dash, WHITE);
            assert_eq!(quads(&m), 2, "dash {dash}");
        }
        let mut m = Mesh::new(0.0);
        m.dashed_polyline(&[0.0, 0.0, f32::INFINITY, 0.0], 0.1, 0.5, WHITE);
        assert_eq!(quads(&m), 1);
    }
}

//! The scene as triangles, in passes: poured copper, areas, tracks, pads, then holes, vias and
//! the board's edge. Layers are drawn back to front with the active one last, and items in id
//! order within a pass.

use std::collections::HashMap;

use crate::{
    AreaKind, BODY, COURTYARD, DIMMED, DRILL, Geom, Item, LARGE_POLYGON, Mesh, OUTLINE, PAD, Pose, Rgba, Scene, Style, UNPLATED_HOLE, VIA,
    Xf, with_alpha,
};

/// One drawing step of a part's item, kept so that moving the part redoes only these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Op {
    /// An area's polygon `tile`, filled.
    Fill {
        tile: usize,
        color: Rgba,
    },
    /// An area's polygon `tile`, as its edge `r` wide.
    Edge {
        tile: usize,
        r: f32,
        color: Rgba,
    },
    Track {
        color: Rgba,
    },
    Pad {
        pad: usize,
        color: Rgba,
    },
    /// A pad stack's hole.
    Hole {
        color: Rgba,
    },
    Via {
        color: Rgba,
    },
    ViaHole,
    Outline {
        poly: usize,
    },
}

/// Where one part's step sits in a drawing's vertices, in floats.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub part: i32,
    pub start: usize,
    pub len: usize,
    item: u32,
    op: Op,
}

/// A built mesh and what it was built from, so it can be brought up to date cheaply.
#[derive(Clone, Debug)]
pub struct Drawing {
    /// [`crate::STRIDE`] floats per vertex, ready for the GPU.
    pub vertices: Vec<f32>,
    pub px: f32,
    pub mask: u64,
    style: Style,
    revision: u64,
    spans: Vec<Span>,
    /// Each part drawn: the pose it was drawn at and its spans.
    parts: HashMap<i32, (Pose, Vec<usize>)>,
}

impl Drawing {
    /// Spans of part steps, for an app that wants to see what moves.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }
}

impl Scene {
    /// Appends the board to `m`, the layers whose bits are set in `mask` (bit 0 the front). `m.aa`
    /// is one screen pixel in millimetres.
    pub fn draw_into(&self, m: &mut Mesh, mask: u64) {
        self.emit(m, mask, None);
    }

    /// The board as a mesh, with what it takes to update it.
    pub fn draw(&self, px: f32, mask: u64) -> Drawing {
        let mut m = Mesh::with_capacity(px, self.items.len() * 64 * crate::STRIDE);
        let mut spans = Vec::new();
        self.emit(&mut m, mask, Some(&mut spans));
        let mut parts: HashMap<i32, (Pose, Vec<usize>)> = HashMap::new();
        for (i, s) in spans.iter().enumerate() {
            parts.entry(s.part).or_insert_with(|| (self.pose(s.part), Vec::new())).1.push(i);
        }
        Drawing { vertices: m.v, px, mask, style: self.style.clone(), revision: self.revision, spans, parts }
    }

    /// Brings a drawing up to date for `px` and `mask`. When only poses changed, and no part
    /// changed side, only the moved parts' vertices are rewritten; otherwise it is rebuilt.
    /// Returns whether the vertices changed.
    pub fn redraw(&self, d: &mut Drawing, px: f32, mask: u64) -> bool {
        if d.revision != self.revision || d.style != self.style || d.px != px || d.mask != mask {
            *d = self.draw(px, mask);
            return true;
        }
        match self.patch(d, px) {
            Some(changed) => changed,
            None => {
                *d = self.draw(px, mask);
                true
            }
        }
    }

    /// Rewrites moved parts' vertices in place, or `None` when the drawing must be rebuilt.
    fn patch(&self, d: &mut Drawing, px: f32) -> Option<bool> {
        let mut scratch = Mesh::new(px);
        let mut changed = false;
        let Drawing { vertices, spans, parts, .. } = d;
        for (&part, (drawn, list)) in parts.iter_mut() {
            let now = self.pose(part);
            if now == *drawn {
                continue;
            }
            if now.bottom != drawn.bottom {
                return None;
            }
            let xf = now.transform();
            for &i in list.iter() {
                let s = spans[i];
                scratch.v.clear();
                self.emit_op(&mut scratch, &self.items[&s.item], &xf, s.op);
                if scratch.v.len() != s.len {
                    return None;
                }
                vertices[s.start..s.start + s.len].copy_from_slice(&scratch.v);
            }
            *drawn = now;
            changed = true;
        }
        Some(changed)
    }

    fn emit(&self, m: &mut Mesh, mask: u64, mut spans: Option<&mut Vec<Span>>) {
        let order = self.back_to_front();
        let shown = |layer: usize| self.shown(layer, mask);
        let style = &self.style;
        let mut step = |m: &mut Mesh, id: u32, item: &Item, xf: &Xf, op: Op| {
            let start = m.v.len();
            self.emit_op(m, item, xf, op);
            if item.part != 0
                && let Some(spans) = spans.as_deref_mut()
            {
                spans.push(Span { part: item.part, start, len: m.v.len() - start, item: id, op });
            }
        };

        if style.show_pours {
            for &layer in &order {
                if !shown(layer) {
                    continue;
                }
                for f in &self.fills {
                    if f.layer == layer {
                        m.fill(&f.polygon, [0.0, 0.0], self.tint(with_alpha(self.drawn_color(layer), 0.5), f.net));
                    }
                }
            }
        }
        for (&id, item) in &self.items {
            let Geom::Area { kind, tiles } = &item.geom else {
                continue;
            };
            // Outlines from the file are stale once poured copper exists.
            if *kind == AreaKind::Pour && (!style.show_pours || !self.fills.is_empty()) {
                continue;
            }
            let pose = self.pose_of(item);
            let xf = self.xf(item.part);
            for (tile, (layer, g)) in tiles.iter().enumerate() {
                let layer = self.placed_layer(*layer, pose.bottom);
                if !shown(layer) {
                    continue;
                }
                let color = match kind {
                    AreaKind::Body => BODY,
                    AreaKind::Courtyard => COURTYARD,
                    AreaKind::Pour => with_alpha(self.drawn_color(layer), 0.22),
                    AreaKind::Keepout | AreaKind::ViaKeepout => with_alpha(self.drawn_color(layer), 0.05),
                };
                let op = if *kind == AreaKind::Courtyard {
                    Op::Edge { tile, r: m.aa * 0.7, color }
                } else if g.len() / 2 > LARGE_POLYGON {
                    // A fan fills only convex shapes; a large outline is drawn as its edge.
                    let edge = if *kind == AreaKind::Pour { 0.45 } else { 0.3 };
                    Op::Edge { tile, r: m.aa * 0.7, color: with_alpha(color, edge) }
                } else {
                    Op::Fill { tile, color }
                };
                step(m, id, item, &xf, op);
            }
        }
        for &layer in &order {
            if !shown(layer) {
                continue;
            }
            let color = self.drawn_color(layer);
            for (&id, item) in &self.items {
                if let Geom::Track { layer: l, .. } = &item.geom
                    && self.placed_layer(*l, self.pose_of(item).bottom) == layer
                {
                    step(m, id, item, &self.xf(item.part), Op::Track { color: self.tint(color, item.net) });
                }
            }
        }
        for &layer in &order {
            if !shown(layer) {
                continue;
            }
            for (&id, item) in &self.items {
                let Geom::Pad { pads, drill, .. } = &item.geom else {
                    continue;
                };
                let bottom = self.pose_of(item).bottom;
                // As KiCad draws them: a through-hole pad in gold, a surface pad in its layer's colour.
                let through = drill.is_some() && pads.len() > 1;
                let color = if through {
                    if style.high_contrast && layer != style.active_layer {
                        [PAD[0] * DIMMED, PAD[1] * DIMMED, PAD[2] * DIMMED, 1.0]
                    } else {
                        PAD
                    }
                } else {
                    with_alpha(self.drawn_color(layer), 1.0)
                };
                let xf = self.xf(item.part);
                for (pad, (l, _)) in pads.iter().enumerate() {
                    if self.placed_layer(*l, bottom) == layer {
                        step(m, id, item, &xf, Op::Pad { pad, color: self.tint(color, item.net) });
                    }
                }
            }
        }
        for (&id, item) in &self.items {
            let xf = self.xf(item.part);
            match &item.geom {
                Geom::Via { shape: Some(_), drill, .. } => {
                    step(m, id, item, &xf, Op::Via { color: self.tint(VIA, item.net) });
                    if drill.is_some() {
                        step(m, id, item, &xf, Op::ViaHole);
                    }
                }
                Geom::Pad { drill: Some(_), pads, .. } => {
                    let bottom = self.pose_of(item).bottom;
                    if pads.is_empty() || pads.iter().any(|(l, _)| shown(self.placed_layer(*l, bottom))) {
                        step(m, id, item, &xf, Op::Hole { color: if pads.is_empty() { UNPLATED_HOLE } else { DRILL } });
                    }
                }
                Geom::Outline { polys } => {
                    for poly in 0..polys.len() {
                        step(m, id, item, &xf, Op::Outline { poly });
                    }
                }
                _ => {}
            }
        }
    }

    fn pose_of(&self, item: &Item) -> Pose {
        if item.part == 0 { Pose::default() } else { self.pose(item.part) }
    }

    /// One step. Items of no part draw their points as they are; a part's are placed by `xf`.
    fn emit_op(&self, m: &mut Mesh, item: &Item, xf: &Xf, op: Op) {
        let board = item.part == 0;
        let place = |g: &[f32]| if board { g.to_vec() } else { xf.points(g) };
        let at = |p: [f32; 2]| if board { p } else { xf.apply(p) };
        match (&item.geom, op) {
            (Geom::Area { tiles, .. }, Op::Fill { tile, color }) => m.fill_xf(&tiles[tile].1, xf, color),
            (Geom::Area { tiles, .. }, Op::Edge { tile, r, color }) => m.ring(&xf.points(&tiles[tile].1), r, color),
            (Geom::Track { width, points, .. }, Op::Track { color }) => m.polyline(&place(points), width / 2.0, color),
            (Geom::Pad { pads, .. }, Op::Pad { pad, color }) => m.prim(&pads[pad].1, xf, color),
            (Geom::Pad { at: centre, drill: Some(d), .. }, Op::Hole { color }) => m.disc(xf.apply(*centre), *d, color),
            (Geom::Via { shape: Some(shape), .. }, Op::Via { color }) => m.prim(shape, xf, color),
            (Geom::Via { at: centre, drill: Some(d), .. }, Op::ViaHole) => m.disc(at(*centre), *d, DRILL),
            (Geom::Outline { polys }, Op::Outline { poly }) => m.ring(&place(&polys[poly]), m.aa, OUTLINE),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    fn board() -> Scene {
        let mut s = Scene::new();
        s.set_layers(&["F.Cu", "B.Cu"]);
        s
    }

    fn square(x: f32, y: f32, h: f32) -> Prim {
        Prim::Poly(vec![x - h, y - h, x + h, y - h, x + h, y + h, x - h, y + h])
    }

    /// A part `id` of two pads, one through-hole, with a body.
    fn add_part(s: &mut Scene, id: i32, first_item: u32) {
        s.set_part(id, &format!("U{id}"));
        s.insert(first_item, Item::new(3, id, Geom::Pad { at: [-1.0, 0.0], pads: vec![(0, square(-1.0, 0.0, 0.4))], drill: None }));
        s.insert(
            first_item + 1,
            Item::new(
                4,
                id,
                Geom::Pad {
                    at: [1.0, 0.0],
                    pads: vec![(0, Prim::Circle([1.0, 0.0, 0.5])), (1, Prim::Circle([1.0, 0.0, 0.5]))],
                    drill: Some(0.2),
                },
            ),
        );
        s.insert(
            first_item + 2,
            Item::new(0, id, Geom::Area { kind: AreaKind::Body, tiles: vec![(0, vec![-1.5, -0.7, 1.5, -0.7, 1.5, 0.7, -1.5, 0.7])] }),
        );
    }

    #[test]
    fn a_track_is_a_quad_per_segment_and_a_square_pad_two_triangles() {
        let mut s = board();
        s.insert(1, Item::new(3, 0, Geom::Track { layer: 0, width: 0.2, points: vec![0.0, 0.0, 5.0, 0.0, 5.0, 5.0] }));
        s.insert(2, Item::new(3, 7, Geom::Pad { at: [2.0, 2.0], pads: vec![(0, square(2.0, 2.0, 0.5))], drill: None }));
        let mut m = Mesh::new(0.01);
        s.draw_into(&mut m, u64::MAX);
        assert_eq!(m.v.len(), (2 * 6 + 6) * STRIDE);
    }

    #[test]
    fn hidden_and_masked_layers_draw_nothing() {
        let mut s = board();
        s.insert(1, Item::new(3, 0, Geom::Track { layer: 0, width: 0.2, points: vec![0.0, 0.0, 5.0, 0.0] }));
        assert!(!s.draw(0.01, 0b01).vertices.is_empty());
        assert!(s.draw(0.01, 0b10).vertices.is_empty());
        s.set_layer_visible(0, false);
        assert!(s.draw(0.01, u64::MAX).vertices.is_empty());
    }

    #[test]
    fn poured_copper_replaces_the_files_outlines() {
        let mut s = board();
        s.style.show_pours = true;
        s.insert(2, Item::new(3, 0, Geom::Area { kind: AreaKind::Pour, tiles: vec![(0, vec![0.0, 0.0, 1.0, 0.0, 1.0, 1.0])] }));
        assert_eq!(s.draw(0.01, u64::MAX).vertices.len(), 3 * STRIDE);
        s.set_fills(vec![Fill { net: 3, layer: 0, polygon: vec![0.0, 0.0, 2.0, 0.0, 2.0, 2.0, 0.0, 2.0] }]);
        assert_eq!(s.draw(0.01, u64::MAX).vertices.len(), 6 * STRIDE);
        s.style.show_pours = false;
        assert!(s.draw(0.01, u64::MAX).vertices.is_empty());
    }

    #[test]
    fn a_moved_part_draws_its_items_moved() {
        let mut s = board();
        add_part(&mut s, 7, 10);
        let before = s.draw(0.01, u64::MAX).vertices;
        s.set_pose(7, Pose::at(10.0, -2.0));
        let after = s.draw(0.01, u64::MAX).vertices;
        assert_eq!(before.len(), after.len());
        for (a, b) in before.chunks_exact(STRIDE).zip(after.chunks_exact(STRIDE)) {
            assert!((b[0] - a[0] - 10.0).abs() < 1e-5 && (b[1] - a[1] + 2.0).abs() < 1e-5);
            assert_eq!(a[6..], b[6..], "radius and colour stay");
        }
    }

    #[test]
    fn patching_moved_parts_gives_the_mesh_a_rebuild_does() {
        let mut s = board();
        for p in 1..=20 {
            add_part(&mut s, p, p as u32 * 10);
            s.set_pose(p, Pose::at(p as f32 * 4.0, 0.0));
        }
        s.insert(1000, Item::new(3, 0, Geom::Track { layer: 0, width: 0.2, points: vec![0.0, 0.0, 50.0, 0.0] }));
        s.insert(1001, Item::new(0, 0, Geom::Outline { polys: vec![vec![0.0, -5.0, 90.0, -5.0, 90.0, 5.0, 0.0, 5.0]] }));
        let mut d = s.draw(0.02, u64::MAX);
        let revision = s.revision();
        for (k, p) in (1..=20).enumerate() {
            s.set_pose(p, Pose { x: p as f32 * 4.0 + 0.3, y: k as f32 * 0.1, rotation: 15.0 * k as f32, bottom: false });
        }
        assert_eq!(s.revision(), revision, "moving parts is not a change to rebuild for");
        assert!(s.redraw(&mut d, 0.02, u64::MAX));
        assert_eq!(d.vertices, s.draw(0.02, u64::MAX).vertices);
        assert!(!s.redraw(&mut d, 0.02, u64::MAX), "nothing moved since");
    }

    #[test]
    fn a_part_turned_to_the_bottom_puts_its_pads_on_the_bottom_layer_mirrored() {
        let mut s = board();
        add_part(&mut s, 7, 10);
        let front = s.draw(0.01, 0b01).vertices.len();
        let back = s.draw(0.01, 0b10).vertices.len();
        let mut d = s.draw(0.01, 0b10);
        s.set_pose(7, Pose { bottom: true, ..Pose::default() });
        assert!(s.redraw(&mut d, 0.01, 0b10));
        assert_eq!(d.vertices, s.draw(0.01, 0b10).vertices);
        // Flipped, the surface pad and the body are on the back.
        assert!(d.vertices.len() > back);
        assert!(s.draw(0.01, 0b01).vertices.len() < front);
        // The surface pad was at x = -1; mirrored it is at x = +1.
        assert_eq!(s.part_box(7), Some([-1.5, -0.7, 1.5, 0.7]));
        let pad = s.draw(0.01, 0b10).vertices;
        assert!(pad.chunks_exact(STRIDE).any(|v| v[6] < 0.0 && (v[0] - 1.4).abs() < 1e-6));
    }

    #[test]
    fn high_contrast_dims_all_but_the_active_layer_and_draws_it_last() {
        let mut s = board();
        s.insert(1, Item::new(3, 0, Geom::Track { layer: 0, width: 0.2, points: vec![0.0, 0.0, 1.0, 0.0] }));
        s.insert(2, Item::new(3, 0, Geom::Track { layer: 1, width: 0.2, points: vec![0.0, 0.0, 1.0, 0.0] }));
        s.style.active_layer = 0;
        s.style.high_contrast = true;
        let v = s.draw(0.01, u64::MAX).vertices;
        // Back first, dimmed; then the front at full colour.
        assert!((v[7] - kicad_color(1, 2, "B.Cu")[0] * DIMMED).abs() < 1e-6);
        assert_eq!(v[v.len() - STRIDE + 7], kicad_color(0, 2, "F.Cu")[0]);
    }

    #[test]
    fn adding_or_removing_a_whole_part_costs_one_rebuild() {
        let mut s = board();
        for p in 1..=10 {
            add_part(&mut s, p, p as u32 * 10);
        }
        let mut d = s.draw(0.02, u64::MAX);
        // A keep-out part with no pads, added and later taken away whole.
        let keepout = vec![(
            500,
            Item::new(0, 0, Geom::Area { kind: AreaKind::Keepout, tiles: vec![(0, vec![0.0, 0.0, 3.0, 0.0, 3.0, 3.0, 0.0, 3.0])] }),
        )];
        s.insert_part(99, "KO1", Pose::at(40.0, 0.0), keepout);
        assert_eq!(s.item(500).map(|i| i.part), Some(99), "items take the part's id");
        assert!(s.redraw(&mut d, 0.02, u64::MAX));
        assert_eq!(d.vertices, s.draw(0.02, u64::MAX).vertices);
        assert!(!s.redraw(&mut d, 0.02, u64::MAX), "one rebuild, then nothing to do");
        // Moving it afterwards is a patch again.
        s.set_pose(99, Pose::at(41.0, 0.0));
        assert!(s.redraw(&mut d, 0.02, u64::MAX));
        assert_eq!(d.vertices, s.draw(0.02, u64::MAX).vertices);

        let with = d.vertices.len();
        let before = s.items().count();
        s.remove_part(3);
        assert_eq!(s.items().count(), before - 3, "its items go with it");
        assert!(s.part(3).is_none() && s.part_box(3).is_none() && s.part_items(3).next().is_none());
        assert!(s.redraw(&mut d, 0.02, u64::MAX));
        assert!(d.vertices.len() < with);
        assert_eq!(d.vertices, s.draw(0.02, u64::MAX).vertices);
        assert!(!s.redraw(&mut d, 0.02, u64::MAX));
    }
}

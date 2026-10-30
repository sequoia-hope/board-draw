//! Picking: the part, net or item under a point, and a part's extent as placed.

use crate::{Geom, Item, PICK_RADIUS, Scene, Xf};

fn area(b: &[f32; 4]) -> f32 {
    (b[2] - b[0]) * (b[3] - b[1])
}

fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) };
    ((p[0] - a[0] - t * dx).powi(2) + (p[1] - a[1] - t * dy).powi(2)).sqrt()
}

/// The distance from `p` to the point `at` placed by `xf`.
fn dist_to(p: [f32; 2], at: [f32; 2], xf: &Xf) -> f32 {
    if xf.plain() {
        ((p[0] - at[0] - xf.t[0]).powi(2) + (p[1] - at[1] - xf.t[1]).powi(2)).sqrt()
    } else {
        let a = xf.apply(at);
        ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt()
    }
}

impl Scene {
    /// The part's extent over its pads, body and courtyard as placed, as x0, y0, x1, y1.
    pub fn part_box(&self, part: i32) -> Option<[f32; 4]> {
        let e = self.extents.get(&part)?;
        let b = e.bounds;
        // A part with nothing to measure has an empty (inverted) box.
        (b[0] <= b[2]).then_some(())?;
        let xf = self.xf(part);
        if xf.plain() {
            let t = xf.t;
            return Some([b[0] + t[0], b[1] + t[1], b[2] + t[0], b[3] + t[1]]);
        }
        let mut out = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for [x, y, r] in &e.discs {
            let [x, y] = xf.apply([*x, *y]);
            out = [out[0].min(x - r), out[1].min(y - r), out[2].max(x + r), out[3].max(y + r)];
        }
        Some(out)
    }

    /// The part a click at the point means: the first of [`Scene::part_ids_at`].
    pub fn part_at(&self, x: f32, y: f32) -> Option<i32> {
        self.part_ids_at(x, y).first().copied()
    }

    /// Every pickable part whose box ([`Scene::part_box`]) holds the point, in picking order:
    /// parts on the front before parts on the back, then the smallest first, by courtyard area
    /// or, for a part without a courtyard, by box area; then by id. So a small part sitting on
    /// a large part's courtyard is picked, and a part underneath never takes a click from one
    /// on top. Parts on a hidden side, or with all their layers hidden, are left out.
    pub fn part_ids_at(&self, x: f32, y: f32) -> Vec<i32> {
        let mut hits: Vec<(bool, f32, i32)> = self
            .parts
            .keys()
            .filter_map(|&p| self.part_box(p).map(|b| (p, b)))
            .filter(|(_, b)| x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3])
            .filter(|(p, _)| self.part_pickable(*p))
            .map(|(p, b)| (self.part_on_back(p), self.courtyard_area(p).unwrap_or_else(|| area(&b)), p))
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        hits.into_iter().map(|(_, _, p)| p).collect()
    }

    /// The copper item nearest the point within `reach` millimetres, of those `want` accepts:
    /// tracks on visible layers by their edge, pads and vias as discs of [`PICK_RADIUS`] about
    /// their centres. The first in id order wins a tie.
    pub fn nearest(&self, x: f32, y: f32, reach: f32, want: impl Fn(&Item) -> bool) -> Option<(u32, &Item)> {
        let p = [x, y];
        let mut best: Option<(f32, u32, &Item)> = None;
        for (&id, item) in &self.items {
            if !want(item) {
                continue;
            }
            let mut consider = |d: f32| {
                if d <= reach && best.is_none_or(|(b, _, _)| d < b) {
                    best = Some((d, id, item));
                }
            };
            let xf = self.xf(item.part);
            match &item.geom {
                Geom::Track { layer, width, points } if self.visible(self.placed_layer(*layer, self.pose(item.part).bottom)) => {
                    let placed;
                    let points = if item.part == 0 {
                        points
                    } else {
                        placed = xf.points(points);
                        &placed
                    };
                    for w in points.windows(4).step_by(2) {
                        consider(seg_dist(p, [w[0], w[1]], [w[2], w[3]]) - width / 2.0);
                    }
                }
                Geom::Via { at, .. } | Geom::Pad { at, .. } => consider(dist_to(p, *at, &xf) - PICK_RADIUS),
                _ => {}
            }
        }
        best.map(|(_, id, item)| (id, item))
    }

    /// The net of the copper nearest the point, within `reach` millimetres.
    pub fn net_at(&self, x: f32, y: f32, reach: f32) -> Option<i32> {
        self.nearest(x, y, reach, |it| it.net != 0).map(|(_, it)| it.net)
    }

    /// The copper item nearest the point, within `reach` millimetres, whatever its net.
    pub fn item_at(&self, x: f32, y: f32, reach: f32) -> Option<u32> {
        self.nearest(x, y, reach, |_| true).map(|(id, _)| id)
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    fn scene() -> Scene {
        let mut s = Scene::new();
        s.set_layers(&["F.Cu", "B.Cu"]);
        s.set_part(7, "U1");
        s.insert(
            2,
            Item::new(
                3,
                7,
                Geom::Pad { at: [2.0, 2.0], pads: vec![(0, Prim::Poly(vec![1.5, 1.5, 2.5, 1.5, 2.5, 2.5, 1.5, 2.5]))], drill: None },
            ),
        );
        s.insert(1, Item::new(5, 0, Geom::Track { layer: 0, width: 0.2, points: vec![0.0, 0.0, 5.0, 0.0, 5.0, 5.0] }));
        s
    }

    #[test]
    fn parts_are_picked_through_their_pads_and_follow_their_pose() {
        let mut s = scene();
        assert_eq!(s.part_box(7), Some([1.5, 1.5, 2.5, 2.5]));
        assert_eq!(s.part_at(2.0, 2.0), Some(7));
        assert_eq!(s.part_at(4.0, 4.0), None);
        s.set_pose(7, Pose::at(2.0, 2.0));
        assert_eq!(s.part_at(4.0, 4.0), Some(7));
        assert_eq!(s.part_at(2.0, 2.0), None);
    }

    #[test]
    fn a_turned_part_is_picked_where_it_turned_to() {
        let mut s = Scene::new();
        s.set_layers(&["F.Cu", "B.Cu"]);
        s.set_part(1, "R1");
        // A long thin part along x, about its origin.
        s.insert(
            1,
            Item::new(
                0,
                1,
                Geom::Pad { at: [0.0, 0.0], pads: vec![(0, Prim::Poly(vec![-3.0, -0.5, 3.0, -0.5, 3.0, 0.5, -3.0, 0.5]))], drill: None },
            ),
        );
        assert_eq!(s.part_at(2.5, 0.0), Some(1));
        s.set_pose(1, Pose { x: 10.0, y: 0.0, rotation: 90.0, bottom: false });
        assert_eq!(s.part_at(12.5, 0.0), None);
        assert_eq!(s.part_at(10.0, 2.5), Some(1));
        let b = s.part_box(1).unwrap();
        assert!((b[0] - 9.5).abs() < 1e-5 && (b[3] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn the_smallest_part_under_the_point_wins() {
        let mut s = scene();
        s.set_part(8, "big");
        s.insert(9, Item::new(0, 8, Geom::Area { kind: AreaKind::Body, tiles: vec![(0, vec![0.0, 0.0, 10.0, 0.0, 10.0, 10.0])] }));
        assert_eq!(s.part_at(2.0, 2.0), Some(7));
        assert_eq!(s.part_at(8.0, 1.0), Some(8));
    }

    #[test]
    fn the_nearest_copper_names_its_net_and_item() {
        let mut s = scene();
        assert_eq!(s.net_at(2.5, 0.05, 0.2), Some(5));
        assert_eq!(s.net_at(2.5, 3.0, 0.2), None);
        assert_eq!(s.net_at(2.0, 2.1, 0.2), Some(3));
        assert_eq!(s.item_at(2.0, 2.1, 0.2), Some(2));
        s.set_pose(7, Pose::at(-2.0, 0.0));
        assert_eq!(s.item_at(0.0, 2.0, 0.2), Some(2));
        s.set_layer_visible(0, false);
        assert_eq!(s.net_at(2.5, 0.05, 0.2), None, "tracks on hidden layers are not picked");
    }

    #[test]
    fn removing_an_item_shrinks_its_part() {
        let mut s = scene();
        s.insert(3, Item::new(3, 7, Geom::Pad { at: [6.0, 2.0], pads: vec![(0, Prim::Circle([6.0, 2.0, 0.5]))], drill: None }));
        assert_eq!(s.part_box(7), Some([1.5, 1.5, 6.5, 2.5]));
        s.remove(3);
        assert_eq!(s.part_box(7), Some([1.5, 1.5, 2.5, 2.5]));
        s.remove(2);
        assert_eq!(s.part_box(7), None);
    }

    fn rect(x: f32, y: f32, hw: f32, hh: f32) -> Vec<f32> {
        vec![x - hw, y - hh, x + hw, y - hh, x + hw, y + hh, x - hw, y + hh]
    }

    /// A QFN with a 7 mm courtyard, and a 0402 sitting on that courtyard just off its pads.
    fn qfn_and_0402() -> Scene {
        let mut s = Scene::new();
        s.set_layers(&["F.Cu", "In1.Cu", "B.Cu"]);
        let mut qfn = vec![(10, Item::new(0, 0, Geom::Area { kind: AreaKind::Courtyard, tiles: vec![(0, rect(0.0, 0.0, 3.5, 3.5))] }))];
        for k in 0..8 {
            let x = -2.1 + k as f32 * 0.6;
            qfn.push((
                11 + k,
                Item::new(k as i32 + 1, 0, Geom::Pad { at: [x, -2.4], pads: vec![(0, Prim::Poly(rect(x, -2.4, 0.15, 0.4)))], drill: None }),
            ));
        }
        s.insert_part(1, "U1", Pose::default(), qfn);
        s.insert_part(
            2,
            "C7",
            Pose::at(2.8, 2.8),
            vec![
                (30, Item::new(1, 0, Geom::Pad { at: [-0.5, 0.0], pads: vec![(0, Prim::Poly(rect(-0.5, 0.0, 0.25, 0.3)))], drill: None })),
                (31, Item::new(2, 0, Geom::Pad { at: [0.5, 0.0], pads: vec![(0, Prim::Poly(rect(0.5, 0.0, 0.25, 0.3)))], drill: None })),
                (32, Item::new(0, 0, Geom::Area { kind: AreaKind::Courtyard, tiles: vec![(0, rect(0.0, 0.0, 0.95, 0.45))] })),
            ],
        );
        s
    }

    #[test]
    fn a_small_part_on_a_large_ones_courtyard_is_picked_first() {
        let s = qfn_and_0402();
        assert_eq!(s.courtyard_area(1), Some(49.0));
        assert_eq!(s.part_at(2.8, 2.8), Some(2));
        assert_eq!(s.part_ids_at(2.8, 2.8), vec![2, 1]);
        assert_eq!(s.part_ids_at(0.0, 0.0), vec![1]);
        assert_eq!(s.part_ids_at(9.0, 9.0), Vec::<i32>::new());
    }

    #[test]
    fn a_part_on_the_back_never_takes_a_click_from_one_on_the_front() {
        let mut s = qfn_and_0402();
        // A tiny part under the QFN, on the back by its layer and by its pose.
        s.insert_part(
            3,
            "R1",
            Pose::default(),
            vec![(40, Item::new(0, 0, Geom::Pad { at: [0.0, 0.0], pads: vec![(2, Prim::Circle([0.0, 0.0, 0.2]))], drill: None }))],
        );
        s.insert_part(
            4,
            "R2",
            Pose { bottom: true, ..Pose::default() },
            vec![(41, Item::new(0, 0, Geom::Pad { at: [0.0, 0.0], pads: vec![(0, Prim::Circle([0.0, 0.0, 0.3]))], drill: None }))],
        );
        assert!(!s.part_on_back(1) && s.part_on_back(3) && s.part_on_back(4));
        assert_eq!(s.part_ids_at(0.0, 0.0), vec![1, 3, 4]);
        // With the front hidden the back's parts are picked, the smallest first.
        s.style.hide_front = true;
        assert_eq!(s.part_ids_at(0.0, 0.0), vec![3, 4]);
        assert_eq!(s.part_at(2.8, 2.8), None);
        s.style.hide_front = false;
        s.style.hide_back = true;
        assert_eq!(s.part_ids_at(0.0, 0.0), vec![1]);
        // Hiding the layers a part is on hides it from picking too.
        s.style.hide_back = false;
        s.set_layer_visible(2, false);
        assert_eq!(s.part_ids_at(0.0, 0.0), vec![1]);
        s.set_layer_visible(0, false);
        assert_eq!(s.part_ids_at(0.0, 0.0), Vec::<i32>::new());
    }

    #[test]
    fn a_through_hole_part_is_on_the_front() {
        let mut s = Scene::new();
        s.set_layers(&["F.Cu", "B.Cu"]);
        let pads = vec![(0, Prim::Circle([0.0, 0.0, 0.8])), (1, Prim::Circle([0.0, 0.0, 0.8]))];
        s.insert_part(1, "J1", Pose::default(), vec![(1, Item::new(0, 0, Geom::Pad { at: [0.0, 0.0], pads, drill: Some(0.5) }))]);
        assert!(!s.part_on_back(1));
    }
}

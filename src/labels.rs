//! Part references to write over the board. The app draws the text itself, on a canvas over
//! the WebGL one; this says what to write, where, and how big.

use crate::Scene;

/// Text height as a share of the part's shorter side.
const HEIGHT_SHARE: f32 = 0.45;
/// Rough width of a character as a share of the text height, for a sans-serif face.
const CHAR_WIDTH: f32 = 0.62;
/// The width of the part the text may take.
const WIDTH_SHARE: f32 = 0.9;
/// No label is suggested taller than this, in millimetres, however large the part.
pub const LABEL_MAX_MM: f32 = 2.5;

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub part: i32,
    /// The reference, such as `U3`.
    pub name: String,
    /// The centre of the part's box, in board millimetres.
    pub at: [f32; 2],
    /// Suggested text height in board millimetres: as large as fits the part's box, up to
    /// [`LABEL_MAX_MM`].
    pub size: f32,
}

impl Scene {
    /// Labels for the view: each named, pickable part whose suggested text would be at least
    /// `min_px` tall at `px_per_mm` screen pixels per millimetre, plus every part in
    /// `selected`, whose text is raised to `min_px` if it would be smaller. In part id order.
    pub fn labels(&self, px_per_mm: f32, min_px: f32, selected: &[i32]) -> Vec<Label> {
        let mut out = Vec::new();
        for (id, part) in self.parts() {
            if part.name.is_empty() || !self.part_pickable(id) {
                continue;
            }
            let Some(b) = self.part_box(id) else { continue };
            let (w, h) = (b[2] - b[0], b[3] - b[1]);
            let chars = part.name.chars().count().max(1) as f32;
            let size = (h.min(w) * HEIGHT_SHARE).min(w * WIDTH_SHARE / (chars * CHAR_WIDTH)).min(LABEL_MAX_MM);
            let chosen = selected.contains(&id);
            let size = if size * px_per_mm >= min_px {
                size
            } else if chosen && px_per_mm > 0.0 {
                min_px / px_per_mm
            } else {
                continue;
            };
            out.push(Label { part: id, name: part.name.clone(), at: [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0], size });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    fn pad(x: f32, y: f32, hw: f32, hh: f32) -> Geom {
        Geom::Pad {
            at: [x, y],
            pads: vec![(0, Prim::Poly(vec![x - hw, y - hh, x + hw, y - hh, x + hw, y + hh, x - hw, y + hh]))],
            drill: None,
        }
    }

    fn scene() -> Scene {
        let mut s = Scene::new();
        s.set_layers(&["F.Cu", "B.Cu"]);
        // A 10 x 10 mm part and a 1 x 0.5 mm one.
        s.insert_part(1, "U1", Pose::at(0.0, 0.0), vec![(1, Item::new(0, 0, pad(0.0, 0.0, 5.0, 5.0)))]);
        s.insert_part(2, "R12", Pose::at(20.0, 0.0), vec![(2, Item::new(0, 0, pad(0.0, 0.0, 0.5, 0.25)))]);
        s
    }

    #[test]
    fn labels_fit_their_part_and_small_ones_drop_out_when_zoomed_out() {
        let s = scene();
        let close = s.labels(100.0, 6.0, &[]);
        assert_eq!(close.len(), 2);
        assert_eq!(close[0].at, [0.0, 0.0]);
        assert_eq!(close[0].size, LABEL_MAX_MM, "a large part's label is capped");
        assert_eq!(close[1].at, [20.0, 0.0]);
        // Three characters across 1 mm.
        assert!(close[1].size < 0.5 && close[1].size * 3.0 * 0.62 <= 0.9 + 1e-6);
        let far = s.labels(5.0, 6.0, &[]);
        assert_eq!(far.iter().map(|l| l.part).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn selected_parts_are_always_labelled_at_a_readable_size() {
        let s = scene();
        let far = s.labels(5.0, 6.0, &[2]);
        assert_eq!(far.len(), 2);
        assert!((far[1].size * 5.0 - 6.0).abs() < 1e-5);
    }

    #[test]
    fn hidden_sides_and_layers_have_no_labels() {
        let mut s = scene();
        s.style.hide_front = true;
        assert!(s.labels(100.0, 1.0, &[1]).is_empty());
        s.style.hide_front = false;
        s.set_layer_visible(0, false);
        assert!(s.labels(100.0, 1.0, &[]).is_empty());
    }
}

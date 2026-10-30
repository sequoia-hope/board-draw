//! JS bindings: a [`Board`] holds a scene and its drawing, and hands the drawing's vertices to
//! `renderer.js`. Items come in as JSON:
//!
//! ```json
//! {"net": 3, "part": 7, "track": {"layer": 0, "width": 0.2, "points": [0, 0, 5, 0]}}
//! {"net": 3, "part": 7, "pad": {"at": [1, 0], "pads": [[0, {"circle": [1, 0, 0.5]}]], "drill": 0.2}}
//! {"net": 3, "via": {"at": [4, 4], "shape": {"circle": [4, 4, 0.3]}, "drill": 0.15}}
//! {"outline": {"polys": [[0, 0, 50, 0, 50, 30, 0, 30]]}}
//! {"part": 7, "area": {"kind": "courtyard", "tiles": [[0, [-2, -1, 2, -1, 2, 1, -2, 1]]]}}
//! ```
//!
//! A shape is `{"circle": [x, y, r]}` or `{"poly": [x0, y0, ...]}`; area kinds are `keepout`,
//! `via-keepout`, `pour`, `body` and `courtyard`.

use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::{AreaKind, Drawing, Fill, Geom, Item, Pose, Prim, Scene};

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum PrimJson {
    Circle([f32; 3]),
    Poly(Vec<f32>),
}

impl From<PrimJson> for Prim {
    fn from(p: PrimJson) -> Prim {
        match p {
            PrimJson::Circle(c) => Prim::Circle(c),
            PrimJson::Poly(g) => Prim::Poly(g),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum KindJson {
    Keepout,
    ViaKeepout,
    Pour,
    Body,
    Courtyard,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum GeomJson {
    Track { layer: usize, width: f32, points: Vec<f32> },
    Via { at: [f32; 2], shape: Option<PrimJson>, drill: Option<f32> },
    Pad { at: [f32; 2], pads: Vec<(usize, PrimJson)>, drill: Option<f32> },
    Outline { polys: Vec<Vec<f32>> },
    Area { kind: KindJson, tiles: Vec<(usize, Vec<f32>)> },
}

#[derive(Deserialize)]
struct ItemJson {
    #[serde(default)]
    net: i32,
    #[serde(default)]
    part: i32,
    #[serde(flatten)]
    geom: GeomJson,
}

#[derive(Deserialize)]
struct FillJson {
    net: i32,
    layer: usize,
    polygon: Vec<f32>,
}

fn item(json: &str) -> Result<Item, JsError> {
    let it: ItemJson = serde_json::from_str(json)?;
    let geom = match it.geom {
        GeomJson::Track { layer, width, points } => Geom::Track { layer, width, points },
        GeomJson::Via { at, shape, drill } => Geom::Via { at, shape: shape.map(Prim::from), drill },
        GeomJson::Pad { at, pads, drill } => Geom::Pad { at, pads: pads.into_iter().map(|(l, p)| (l, p.into())).collect(), drill },
        GeomJson::Outline { polys } => Geom::Outline { polys },
        GeomJson::Area { kind, tiles } => Geom::Area {
            kind: match kind {
                KindJson::Keepout => AreaKind::Keepout,
                KindJson::ViaKeepout => AreaKind::ViaKeepout,
                KindJson::Pour => AreaKind::Pour,
                KindJson::Body => AreaKind::Body,
                KindJson::Courtyard => AreaKind::Courtyard,
            },
            tiles,
        },
    };
    Ok(Item { net: it.net, part: it.part, geom })
}

#[wasm_bindgen]
pub struct Board {
    scene: Scene,
    drawing: Option<Drawing>,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Board {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Board {
        Board { scene: Scene::new(), drawing: None }
    }

    /// Layer names, front to back, in KiCad's colours.
    pub fn set_layers(&mut self, names: Vec<String>) {
        self.scene.set_layers(&names);
    }

    pub fn set_layer_visible(&mut self, layer: usize, visible: bool) {
        self.scene.set_layer_visible(layer, visible);
    }

    pub fn set_layer_color(&mut self, layer: usize, r: f32, g: f32, b: f32, a: f32) {
        self.scene.set_layer_color(layer, [r, g, b, a]);
    }

    pub fn set_net(&mut self, net: i32, name: String) {
        self.scene.nets.insert(net, name);
    }

    pub fn net_name(&self, net: i32) -> String {
        self.scene.nets.get(&net).cloned().unwrap_or_default()
    }

    /// Adds or replaces item `id`, from JSON as the module's docs show.
    pub fn set_item(&mut self, id: u32, json: &str) -> Result<(), JsError> {
        self.scene.insert(id, item(json)?);
        Ok(())
    }

    pub fn remove_item(&mut self, id: u32) {
        self.scene.remove(id);
    }

    /// Poured copper: `[{"net": 3, "layer": 0, "polygon": [...]}, ...]`, convex polygons.
    pub fn set_fills(&mut self, json: &str) -> Result<(), JsError> {
        let fills: Vec<FillJson> = serde_json::from_str(json)?;
        self.scene.set_fills(fills.into_iter().map(|f| Fill { net: f.net, layer: f.layer, polygon: f.polygon }).collect());
        Ok(())
    }

    /// Clears items, parts and fills, keeping layers, nets and style.
    pub fn clear(&mut self) {
        self.scene.clear();
    }

    pub fn set_part(&mut self, id: i32, name: &str) {
        self.scene.set_part(id, name);
    }

    pub fn remove_part(&mut self, id: i32) {
        self.scene.remove_part(id);
    }

    /// Places a part: degrees anticlockwise, mirrored first when on the bottom.
    pub fn set_pose(&mut self, id: i32, x: f32, y: f32, rotation: f32, bottom: bool) {
        self.scene.set_pose(id, Pose { x, y, rotation, bottom });
    }

    /// Places many parts at once: five floats each, `id, x, y, rotation, bottom (0 or 1)`.
    pub fn set_poses(&mut self, poses: &[f32]) {
        for p in poses.chunks_exact(5) {
            self.scene.set_pose(p[0] as i32, Pose { x: p[1], y: p[2], rotation: p[3], bottom: p[4] != 0.0 });
        }
    }

    /// `[x, y, rotation, bottom]`.
    pub fn pose(&self, id: i32) -> Vec<f32> {
        let p = self.scene.pose(id);
        vec![p.x, p.y, p.rotation, if p.bottom { 1.0 } else { 0.0 }]
    }

    pub fn set_active_layer(&mut self, layer: usize) {
        self.scene.style.active_layer = layer;
    }

    pub fn set_high_contrast(&mut self, on: bool) {
        self.scene.style.high_contrast = on;
    }

    pub fn set_hot_net(&mut self, net: i32) {
        self.scene.style.hot_net = (net != 0).then_some(net);
    }

    pub fn set_show_pours(&mut self, on: bool) {
        self.scene.style.show_pours = on;
    }

    /// The vertices for `Renderer.upload`: `px` is one screen pixel in millimetres, `mask` a
    /// bit per layer from the front. Kept between calls; moving parts rewrites only theirs.
    pub fn mesh(&mut self, px: f32, mask: u32) -> Vec<f32> {
        self.update(px, mask);
        self.drawing.as_ref().map(|d| d.vertices.clone()).unwrap_or_default()
    }

    /// Brings the kept drawing up to date without copying it out; returns whether it changed.
    /// With [`Board::vertices_ptr`] and [`Board::vertices_len`] JS can view the vertices in
    /// the module's memory, `new Float32Array(memory.buffer, ptr, len)`, valid until the next
    /// call into the board.
    pub fn update(&mut self, px: f32, mask: u32) -> bool {
        let mask = u64::from(mask);
        match &mut self.drawing {
            Some(d) => self.scene.redraw(d, px, mask),
            None => {
                self.drawing = Some(self.scene.draw(px, mask));
                true
            }
        }
    }

    pub fn vertices_ptr(&self) -> *const f32 {
        self.drawing.as_ref().map_or(std::ptr::null(), |d| d.vertices.as_ptr())
    }

    /// Floats, not vertices.
    pub fn vertices_len(&self) -> usize {
        self.drawing.as_ref().map_or(0, |d| d.vertices.len())
    }

    pub fn part_at(&self, x: f32, y: f32) -> Option<i32> {
        self.scene.part_at(x, y)
    }

    pub fn net_at(&self, x: f32, y: f32, reach: f32) -> Option<i32> {
        self.scene.net_at(x, y, reach)
    }

    pub fn item_at(&self, x: f32, y: f32, reach: f32) -> Option<u32> {
        self.scene.item_at(x, y, reach)
    }

    /// `[x0, y0, x1, y1]` as placed, or nothing for a part with no pads, body or courtyard.
    pub fn part_box(&self, part: i32) -> Option<Vec<f32>> {
        self.scene.part_box(part).map(|b| b.to_vec())
    }
}

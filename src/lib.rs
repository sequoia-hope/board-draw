//! A printed circuit board as something to draw: layers, tracks, pads, vias, areas and parts in
//! board millimetres, turned into triangles for a WebGL2 shader that cuts every quad to an
//! anti-aliased capsule, with the picking a pointer needs.
//!
//! - [`Scene`] holds the board. Items owned by a part are stored in the part's own frame once,
//!   and the part's [`Pose`] places them, so moving parts never rebuilds the scene.
//! - [`Scene::draw`] builds a [`Drawing`]; [`Scene::redraw`] brings it up to date, rewriting
//!   only the vertices of parts that moved when nothing else changed.
//! - [`Scene::draw_into`] appends the board to a [`Mesh`], to which an app adds its own
//!   overlays with the same primitives.
//! - [`Scene::part_at`], [`Scene::net_at`], [`Scene::item_at`] and [`Scene::part_box`] pick.
//!
//! The vertex format is [`STRIDE`] floats: position, the segment's two ends, its radius and an
//! RGBA colour (premultiplied by the shader). A negative radius is a plain fill.

use std::collections::{BTreeMap, BTreeSet, HashMap};

mod draw;
mod hit;
mod mesh;
#[cfg(feature = "wasm")]
pub mod wasm;

pub use draw::{Drawing, Span};
pub use mesh::{Mesh, STRIDE, circle_points};

pub type Rgba = [f32; 4];

// KiCad's default colour theme.
/// Through-hole pads, whatever the layer.
pub const PAD: Rgba = [0.89, 0.72, 0.18, 1.0];
pub const VIA: Rgba = [0.86, 0.86, 0.86, 1.0];
pub const DRILL: Rgba = [0.0, 0.03, 0.07, 1.0];
pub const UNPLATED_HOLE: Rgba = [0.1, 0.77, 0.82, 1.0];
pub const OUTLINE: Rgba = [0.82, 0.82, 0.8, 1.0];
pub const BODY: Rgba = [1.0, 1.0, 1.0, 0.05];
pub const COURTYARD: Rgba = [1.0, 0.15, 0.89, 0.7];
/// How bright the layers other than the active one stay in high-contrast mode.
pub const DIMMED: f32 = 0.22;
/// Areas with more corners than this are drawn as their edge, since a fan fills only convex shapes.
pub const LARGE_POLYGON: usize = 24;
/// The distance within which a pad or via counts as hit, about its centre, in millimetres.
pub const PICK_RADIUS: f32 = 0.3;

/// A shape in millimetres: a disc `[x, y, r]` or a convex polygon `[x0, y0, x1, y1, ...]`.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    Circle([f32; 3]),
    Poly(Vec<f32>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaKind {
    Keepout,
    ViaKeepout,
    /// A zone's outline as the board file has it, drawn until poured copper ([`Fill`]) exists.
    Pour,
    /// A part's body, drawn faintly whatever the layer.
    Body,
    /// A part's courtyard, drawn as its edge.
    Courtyard,
}

/// What an item is. Polylines and polygons are flat `[x0, y0, x1, y1, ...]`.
#[derive(Clone, Debug, PartialEq)]
pub enum Geom {
    /// A track: capsules `width` wide joining the points.
    Track { layer: usize, width: f32, points: Vec<f32> },
    /// A via: its copper `shape` on every layer and its hole, of radius `drill`.
    Via { at: [f32; 2], shape: Option<Prim>, drill: Option<f32> },
    /// A pad stack centred on `at`: copper on some layers, and a hole of radius `drill` if it
    /// is drilled. With no pads and a drill it is an unplated hole.
    Pad { at: [f32; 2], pads: Vec<(usize, Prim)>, drill: Option<f32> },
    /// The board's edge: closed rings.
    Outline { polys: Vec<Vec<f32>> },
    /// Polygons on layers: rule areas, zone outlines, bodies and courtyards.
    Area { kind: AreaKind, tiles: Vec<(usize, Vec<f32>)> },
}

/// Something on the board. When `part` is not 0 the item is the part's and its geometry is in
/// the part's frame, placed on the board by the part's pose.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub net: i32,
    pub part: i32,
    pub geom: Geom,
}

impl Item {
    pub fn new(net: i32, part: i32, geom: Geom) -> Item {
        Item { net, part, geom }
    }
}

/// Where a part's frame sits on the board: rotated `rotation` degrees anticlockwise (from +x
/// towards +y), mirrored in x first when on the `bottom`, then moved to `x, y`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub bottom: bool,
}

impl Pose {
    pub fn at(x: f32, y: f32) -> Pose {
        Pose { x, y, ..Pose::default() }
    }

    /// The transform, worked out once per part rather than per point.
    pub fn transform(&self) -> Xf {
        let turned = self.rotation.rem_euclid(360.0) != 0.0;
        let (s, c) = if turned { self.rotation.to_radians().sin_cos() } else { (0.0, 1.0) };
        Xf { t: [self.x, self.y], c, s, mirror: self.bottom, plain: !turned && !self.bottom }
    }
}

/// A pose as a transform. A pose that only moves adds its offset, so an unturned part draws
/// exactly the floats `point + offset` gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub t: [f32; 2],
    c: f32,
    s: f32,
    mirror: bool,
    plain: bool,
}

impl Xf {
    pub const IDENTITY: Xf = Xf { t: [0.0, 0.0], c: 1.0, s: 0.0, mirror: false, plain: true };

    /// Whether the transform only moves, without turning or mirroring.
    pub fn plain(&self) -> bool {
        self.plain
    }

    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        if self.plain {
            return [p[0] + self.t[0], p[1] + self.t[1]];
        }
        let x = if self.mirror { -p[0] } else { p[0] };
        [self.c * x - self.s * p[1] + self.t[0], self.s * x + self.c * p[1] + self.t[1]]
    }

    /// A flat point list, transformed.
    pub fn points(&self, g: &[f32]) -> Vec<f32> {
        g.chunks_exact(2).flat_map(|c| self.apply([c[0], c[1]])).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub color: Rgba,
    pub visible: bool,
    /// The layer a part's geometry on this layer goes to when the part is on the bottom.
    pub flip: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub name: String,
    pub pose: Pose,
}

/// Poured copper: a convex polygon of a zone's fill.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub net: i32,
    pub layer: usize,
    pub polygon: Vec<f32>,
}

/// How the board is drawn, apart from what is on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    /// The layer drawn on top, and in high-contrast mode the only one drawn bright.
    pub active_layer: usize,
    pub high_contrast: bool,
    /// A net drawn lighter wherever it is.
    pub hot_net: Option<i32>,
    /// Whether zones are drawn: as poured, or before they are, as their outlines.
    pub show_pours: bool,
}

/// A part's extent in its own frame, kept up to date as its items come and go.
#[derive(Clone, Debug)]
struct Extent {
    /// x0, y0, x1, y1 over its pads, body and courtyard.
    bounds: [f32; 4],
    /// Discs `[x, y, r]` whose union's box is the extent however the part is turned.
    discs: Vec<[f32; 3]>,
}

impl Extent {
    fn new() -> Extent {
        Extent { bounds: [f32::MAX, f32::MAX, f32::MIN, f32::MIN], discs: Vec::new() }
    }

    fn grow(&mut self, x: f32, y: f32) {
        let b = &mut self.bounds;
        *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }

    fn add(&mut self, geom: &Geom) {
        match geom {
            Geom::Pad { pads, .. } => {
                for (_, p) in pads {
                    match p {
                        Prim::Circle([x, y, r]) => {
                            self.grow(x - r, y - r);
                            self.grow(x + r, y + r);
                            self.discs.push([*x, *y, *r]);
                        }
                        Prim::Poly(g) => self.add_points(g),
                    }
                }
            }
            Geom::Area { kind: AreaKind::Body | AreaKind::Courtyard, tiles } => {
                for (_, g) in tiles {
                    self.add_points(g);
                }
            }
            _ => {}
        }
    }

    fn add_points(&mut self, g: &[f32]) {
        for c in g.chunks_exact(2) {
            self.grow(c[0], c[1]);
            self.discs.push([c[0], c[1], 0.0]);
        }
    }
}

/// The board to draw. Items are drawn in id order within each pass, layers back to front with
/// the active one last.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    layers: Vec<Layer>,
    pub nets: HashMap<i32, String>,
    items: BTreeMap<u32, Item>,
    parts: BTreeMap<i32, Part>,
    /// The items each part owns.
    owned: HashMap<i32, BTreeSet<u32>>,
    extents: HashMap<i32, Extent>,
    fills: Vec<Fill>,
    pub style: Style,
    /// Counts every change but a pose, so a drawing knows when moving parts is all it must redo.
    revision: u64,
}

/// KiCad's default copper colours: F.Cu red, B.Cu blue, the inner layers in its order. `layer`
/// counts from the front, of `count` layers.
pub fn kicad_color(layer: usize, count: usize, name: &str) -> Rgba {
    let last = count.saturating_sub(1);
    let rgb = |r: f32, g: f32, b: f32| [r / 255.0, g / 255.0, b / 255.0, 0.9];
    if name == "F.Cu" || layer == 0 {
        rgb(200.0, 52.0, 52.0)
    } else if name == "B.Cu" || layer == last {
        rgb(77.0, 127.0, 196.0)
    } else {
        [
            rgb(127.0, 200.0, 127.0),
            rgb(206.0, 125.0, 44.0),
            rgb(79.0, 203.0, 203.0),
            rgb(219.0, 98.0, 139.0),
            rgb(167.0, 165.0, 198.0),
            rgb(40.0, 204.0, 217.0),
        ][(layer - 1) % 6]
    }
}

pub fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], a]
}

impl Scene {
    pub fn new() -> Scene {
        Scene::default()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn changed(&mut self) {
        self.revision += 1;
    }

    // Layers.

    /// Sets the layers, front to back, in KiCad's colours, all visible. A part on the bottom
    /// takes `F.x` to `B.x` and back, and the copper stack end to end.
    pub fn set_layers<S: AsRef<str>>(&mut self, names: &[S]) {
        let n = names.len();
        let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
        self.layers = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let flip = if name.ends_with(".Cu") {
                    n - 1 - i
                } else if let Some(rest) = name.strip_prefix("F.") {
                    names.iter().position(|o| o.strip_prefix("B.") == Some(rest)).unwrap_or(i)
                } else if let Some(rest) = name.strip_prefix("B.") {
                    names.iter().position(|o| o.strip_prefix("F.") == Some(rest)).unwrap_or(i)
                } else {
                    i
                };
                Layer { name: name.to_string(), color: kicad_color(i, n, name), visible: true, flip }
            })
            .collect();
        self.changed();
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn set_layer(&mut self, index: usize, layer: Layer) {
        if let Some(l) = self.layers.get_mut(index) {
            *l = layer;
            self.changed();
        }
    }

    pub fn set_layer_visible(&mut self, index: usize, visible: bool) {
        if let Some(l) = self.layers.get_mut(index)
            && l.visible != visible
        {
            l.visible = visible;
            self.changed();
        }
    }

    pub fn set_layer_color(&mut self, index: usize, color: Rgba) {
        if let Some(l) = self.layers.get_mut(index) {
            l.color = color;
            self.changed();
        }
    }

    /// A layer's colour; a layer the scene does not have gets KiCad's colour for its place.
    pub fn layer_color(&self, layer: usize) -> Rgba {
        match self.layers.get(layer) {
            Some(l) => l.color,
            None => kicad_color(layer, self.layers.len(), ""),
        }
    }

    /// Whether a layer is visible; layers the scene does not have are.
    pub fn visible(&self, layer: usize) -> bool {
        self.layers.get(layer).is_none_or(|l| l.visible)
    }

    /// Whether a layer is drawn under `mask`, a bit per layer from the front.
    pub fn shown(&self, layer: usize, mask: u64) -> bool {
        self.visible(layer) && layer < 64 && mask & (1 << layer) != 0
    }

    /// A layer's colour as drawn now: dimmed in high-contrast mode unless it is the active one.
    pub fn drawn_color(&self, layer: usize) -> Rgba {
        let c = self.layer_color(layer);
        if self.style.high_contrast && layer != self.style.active_layer { [c[0] * DIMMED, c[1] * DIMMED, c[2] * DIMMED, c[3]] } else { c }
    }

    /// The colour lightened when `net` is the hot net.
    pub fn tint(&self, color: Rgba, net: i32) -> Rgba {
        if net != 0 && self.style.hot_net == Some(net) {
            [(color[0] + 1.0) / 2.0, (color[1] + 1.0) / 2.0, (color[2] + 1.0) / 2.0, 1.0]
        } else {
            color
        }
    }

    /// Layers in drawing order: back to front, the active one last.
    pub fn back_to_front(&self) -> Vec<usize> {
        let active = self.style.active_layer;
        let mut order: Vec<usize> = (0..self.layers.len().max(1)).rev().filter(|l| *l != active).collect();
        order.push(active);
        order
    }

    /// The layer a part's `layer` is on the board, for a part on the bottom or not.
    pub fn placed_layer(&self, layer: usize, bottom: bool) -> usize {
        if bottom { self.layers.get(layer).map_or(layer, |l| l.flip) } else { layer }
    }

    // Items.

    /// Adds an item, or replaces the one with the same id.
    pub fn insert(&mut self, id: u32, item: Item) {
        self.remove(id);
        if item.part != 0 {
            self.owned.entry(item.part).or_default().insert(id);
            self.extents.entry(item.part).or_insert_with(Extent::new).add(&item.geom);
        }
        self.items.insert(id, item);
        self.changed();
    }

    pub fn remove(&mut self, id: u32) -> Option<Item> {
        let item = self.items.remove(&id)?;
        if item.part != 0 {
            let owned = self.owned.entry(item.part).or_default();
            owned.remove(&id);
            let mut extent = Extent::new();
            for i in owned.iter() {
                extent.add(&self.items[i].geom);
            }
            self.extents.insert(item.part, extent);
        }
        self.changed();
        Some(item)
    }

    pub fn item(&self, id: u32) -> Option<&Item> {
        self.items.get(&id)
    }

    /// Every item, in id order.
    pub fn items(&self) -> impl Iterator<Item = (u32, &Item)> {
        self.items.iter().map(|(id, it)| (*id, it))
    }

    /// The ids of a part's items, in order.
    pub fn part_items(&self, part: i32) -> impl Iterator<Item = u32> + '_ {
        self.owned.get(&part).into_iter().flatten().copied()
    }

    /// Clears items, parts and fills, keeping the layers, nets and style.
    pub fn clear(&mut self) {
        self.items.clear();
        self.parts.clear();
        self.owned.clear();
        self.extents.clear();
        self.fills.clear();
        self.changed();
    }

    // Poured copper.

    pub fn set_fills(&mut self, fills: Vec<Fill>) {
        self.fills = fills;
        self.changed();
    }

    pub fn fills(&self) -> &[Fill] {
        &self.fills
    }

    // Parts.

    /// Adds or renames a part, keeping its pose if it has one.
    pub fn set_part(&mut self, id: i32, name: &str) {
        let part = self.parts.entry(id).or_default();
        if part.name != name {
            part.name = name.to_string();
        }
        self.changed();
    }

    pub fn part(&self, id: i32) -> Option<&Part> {
        self.parts.get(&id)
    }

    pub fn parts(&self) -> impl Iterator<Item = (i32, &Part)> {
        self.parts.iter().map(|(id, p)| (*id, p))
    }

    pub fn remove_part(&mut self, id: i32) -> Option<Part> {
        let part = self.parts.remove(&id);
        self.changed();
        part
    }

    /// Moves a part. This is cheap: nothing is rebuilt, and [`Scene::redraw`] rewrites only the
    /// part's own vertices. A part not yet added is added without a name.
    pub fn set_pose(&mut self, id: i32, pose: Pose) {
        self.parts.entry(id).or_default().pose = pose;
    }

    /// The part's pose; a part the scene does not know stands where its items say.
    pub fn pose(&self, id: i32) -> Pose {
        self.parts.get(&id).map(|p| p.pose).unwrap_or_default()
    }

    /// The transform placing the part's items; items of no part stay where they are.
    pub fn xf(&self, part: i32) -> Xf {
        if part == 0 { Xf::IDENTITY } else { self.pose(part).transform() }
    }

    /// Puts every part back at the identity pose.
    pub fn reset_poses(&mut self) {
        for part in self.parts.values_mut() {
            part.pose = Pose::default();
        }
    }

    /// Whether any of the part's pads is on a layer drawn under `mask`.
    pub fn part_shown(&self, part: i32, mask: u64) -> bool {
        let bottom = self.pose(part).bottom;
        self.part_items(part).any(|id| {
            matches!(&self.items[&id].geom, Geom::Pad { pads, .. } if pads.iter().any(|(l, _)| self.shown(self.placed_layer(*l, bottom), mask)))
        })
    }
}

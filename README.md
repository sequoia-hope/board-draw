# board-draw

Draws printed circuit boards in the browser with WebGL2. It is shared by
[UltraRoute Studio](https://github.com/sequoia-hope/ultraroute) and Circuit Composer, so both
apps draw a board the same way. MIT licensed.

It has two halves:

- **A Rust crate.** It holds a scene in board millimetres, builds the mesh, and does hit testing.
  It compiles to WASM and has JS bindings behind the `wasm` feature.
- **`js/renderer.js`.** A WebGL2 renderer of about 120 lines. Each vertex carries a segment's two
  ends and its radius, and the fragment shader cuts the quad to an anti-aliased capsule. A
  negative radius means a plain fill.

## The scene

```rust
use board_draw::{Scene, Item, Geom, Prim, AreaKind, Pose, Fill, Mesh};

let mut s = Scene::new();
s.set_layers(&["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"]);   // front to back, KiCad colours
s.nets.insert(3, "GND".into());

// Board items: part 0.
s.insert(1, Item::new(3, 0, Geom::Track { layer: 0, width: 0.2, points: vec![0.0, 0.0, 5.0, 0.0] }));
s.insert(2, Item::new(3, 0, Geom::Via { at: [5.0, 0.0], shape: Some(Prim::Circle([5.0, 0.0, 0.3])), drill: Some(0.15) }));
s.insert(3, Item::new(0, 0, Geom::Outline { polys: vec![vec![-10.0, -10.0, 30.0, -10.0, 30.0, 20.0, -10.0, 20.0]] }));

// A part: its items are in the part's own frame, placed by its pose.
s.set_part(7, "U1");
s.insert(10, Item::new(3, 7, Geom::Pad { at: [-1.0, 0.0], pads: vec![(0, Prim::Poly(vec![-1.3, -0.3, -0.7, -0.3, -0.7, 0.3, -1.3, 0.3]))], drill: None }));
s.insert(11, Item::new(0, 7, Geom::Area { kind: AreaKind::Courtyard, tiles: vec![(0, vec![-2.0, -1.0, 2.0, -1.0, 2.0, 1.0, -2.0, 1.0])] }));
s.set_pose(7, Pose { x: 12.0, y: 4.0, rotation: 90.0, bottom: false });
```

| Piece | What it is |
| --- | --- |
| `Layer` | name, colour, visibility, and the layer it flips to. `set_layers` pairs `F.x` with `B.x` and reverses the copper stack. |
| `Geom::Track` | capsules `width` wide along a polyline on one layer |
| `Geom::Pad` | a pad stack centred on `at`: shapes on layers, and a hole of radius `drill`. With no pads and a drill, it is an unplated hole. |
| `Geom::Via` | a copper shape on every layer, and a hole |
| `Geom::Outline` | the board's edge, as closed rings |
| `Geom::Area` | polygons on layers. The kinds are `Keepout`, `ViaKeepout`, `Pour` (a zone outline, drawn until poured copper exists), `Body` and `Courtyard`. |
| `Fill` | poured copper, as convex polygons (`set_fills`) |
| `Part` | a name and a `Pose`: `x`, `y`, `rotation` in degrees anticlockwise, `bottom`. Items with `part != 0` are in that part's frame. |
| `Style` | `active_layer` (drawn last, and the only bright layer in `high_contrast`), `hot_net` (drawn lighter), `show_pours`, and `hide_front` / `hide_back` (sides turned off for picking and labels) |

### Adding and removing whole parts

`insert_part(id, name, pose, items)` adds a part with all its items in one call. It replaces
anything the part had, and each item takes the part's id. `remove_part(id)` removes the part
together with every item it owns.

Neither call is meant for every frame. Each one marks the scene changed, so the next `redraw`
does one full rebuild (about 1.2 ms for 200 parts), however many items came or went. Use them
for things like adding a fixed keep-out part at runtime. Use `set_pose` for anything that
moves.

Shapes (`Prim`) are a disc `[x, y, r]` or a convex polygon `[x0, y0, x1, y1, ...]`. Polygons
with more than 24 corners are drawn as their edge.

## Drawing

Every drawing call takes two arguments:

- `px`: one screen pixel in millimetres. Capsules grow by `px` to leave room for anti-aliasing.
- `mask`: one bit per layer, starting at the front.

```rust
let mut drawing = s.draw(px, mask);     // vertices in drawing.vertices
s.set_pose(7, new_pose);                // cheap: no rebuild
s.redraw(&mut drawing, px, mask);       // rewrites only the moved parts' vertices
```

`redraw` rebuilds the whole drawing when anything other than a pose has changed since it was
built: items, layers, fills, style, `px` or `mask`. It also rebuilds when a part changes side.
Otherwise it re-emits only the vertices of the parts that moved, in place. A patched drawing is
byte-for-byte the drawing a rebuild gives, and a test checks that.

To draw your own overlays on top, build the board into a `Mesh` and add to it with the same
primitives: `segment`, `disc`, `polyline`, `dashed_polyline`, `ring`, `dashed_ring` and `fill`.

```rust
let mut m = Mesh::new(px);
s.draw_into(&mut m, mask);
m.ring(&box_corners, px * 1.5, [0.5, 0.8, 0.7, 1.0]);
let vertices = m.into_vec();
```

The passes run in this order, with layers back to front and the active layer last:

1. poured copper;
2. areas;
3. tracks;
4. pads (through-hole pads in gold);
5. holes, vias and the board's edge.

Within a pass, items are drawn in id order.

## Hit testing

| Call | Returns |
| --- | --- |
| `part_box(part)` | the part's extent over its pads, body and courtyard, as placed: `[x0, y0, x1, y1]` |
| `part_at(x, y)` | the part a click means: the first of `part_ids_at` |
| `part_ids_at(x, y)` | every pickable part whose box holds the point, in picking order |
| `net_at(x, y, reach)` | the net of the nearest copper |
| `item_at(x, y, reach)` | the id of the nearest copper item |

For `net_at` and `item_at`:

- a track counts by its edge, and only on a visible layer;
- a pad or via counts as a disc of 0.3 mm about its centre.

Part extents are cached in the part's own frame. A pose change is therefore free for picking.

**Picking order.** Parts are ordered by these rules, in turn:

1. Parts on the front come before parts on the back.
2. Smaller parts come first. A part is measured by its courtyard area, or by its box area if it
   has no courtyard.
3. Ties go by part id.

So a 0402 sitting on a QFN's courtyard is picked before the QFN, and a part on the back never
takes a click from a part on the front.

A part is on the back when everything it has on layers sits on back layers (`B.x`, or the
last layer when no names say `B.`). That covers both a part posed `bottom: true` and a part
whose pads are given on `B.Cu`. A through-hole part counts as front.

A part is left out of picking when its side is hidden (`style.hide_front` / `hide_back`), or
when every layer it is on is hidden.

## Labels

There is no text in WebGL. The app writes references itself, on a 2D canvas over the WebGL one.
`labels(px_per_mm, min_px, selected)` says what to write, where and how big:

- It gives one `Label` per named, pickable part: `{ part, name, at, size }`.
- `at` is the centre of the part's box, in board millimetres.
- `size` is a suggested text height in millimetres. It is as large as fits the box and at most
  2.5 mm.
- A part is left out when its text would be shorter than `min_px` at `px_per_mm`, unless it is
  in `selected`. A selected part is always included, and its text is raised to `min_px`.

To draw a label, put its centre on screen with your view transform and set the font to
`size * px_per_mm` pixels.

## Timing

`cargo run --release --example pose_update` builds a board of 200 parts. Each part has 16
pads, a body and a courtyard. The board also has 2,000 tracks and 300 vias, 52,800 vertices in
all. Native timings on a Ryzen desktop:

| Step | Time |
| --- | --- |
| Draw from scratch | 1.2 ms |
| Set every part's pose | 0.005 ms |
| Turn and move every part, then `redraw` | 0.35 ms |
| `part_at` with every part turned | 0.04 ms |

The same board built through the WASM `Board` and run in Node 22 (without the vias):

| Step | Time |
| --- | --- |
| `set_poses` for all 200 parts, then `update` | 0.48 ms |
| Rebuild from scratch | 1.2 ms |
| Copy the vertices out with `mesh()` | 0.8 ms |

`update` with a view over the module's memory avoids that copy.

## From JS

Build with `wasm-pack build --target web -- --features wasm`. This gives a `Board` class:

```js
import init, { Board } from './pkg/board_draw.js';
import { Renderer } from 'board-draw';

await init();
const board = new Board();
board.set_layers(['F.Cu', 'B.Cu']);
board.set_part(7, 'U1');
board.set_item(10, JSON.stringify({ net: 3, part: 7, pad: { at: [-1, 0], pads: [[0, { circle: [-1, 0, 0.4] }]] } }));
board.set_item(11, JSON.stringify({ part: 7, area: { kind: 'courtyard', tiles: [[0, [-2, -1, 2, -1, 2, 1, -2, 1]]] } }));
board.set_poses(new Float32Array([7, 12, 4, 90, 0]));   // id, x, y, rotation, bottom: per part

const renderer = new Renderer(canvas);
renderer.upload(0, board.mesh(1 / zoom, 0xffffffff));      // kept between calls; patched on moves
renderer.clear([0, 16 / 255, 35 / 255]);
renderer.draw(0, { cx, cy, zoom }, { x: 0, y: 0, w: canvas.width, h: canvas.height }, false);
board.part_at(x, y); board.part_ids_at(x, y); board.net_at(x, y, 0.3); board.item_at(x, y, 0.3); board.part_box(7);
board.set_side_hidden(true, true);                          // back side off for picking and labels
JSON.parse(board.labels(zoom, 7, new Int32Array([7])));     // [{id, name, x, y, size}]
board.add_part(20, 'KO1', 30, 5, 0, false, JSON.stringify([[100, { area: { kind: 'keepout', tiles: [[0, [0, 0, 3, 0, 3, 3, 0, 3]]] } }]]));
board.remove_part(20);                                      // with its items
```

`board.update(px, mask)` refreshes the drawing without copying it. You can then view it with
`new Float32Array(memory.buffer, board.vertices_ptr(), board.vertices_len())`. The view is valid
until the next call into the board.

The `wasm.rs` module docs list the item JSON format.

The renderer's API:

- `upload(slot, vertices)`: one mesh per slot.
- `clear(rgb)`.
- `draw(slot, view, rect, mirror)`. The view is `{cx, cy, zoom}` in millimetres and pixels per
  millimetre. The rect is the pane in device pixels, measured from the bottom left. `mirror`
  shows the board from below.

A WebGPU renderer could later sit behind the same three calls.

## Pinning it

Both apps pin one commit, the same one on the Rust side and the JS side.

**Rust.** Add a Cargo git dependency by revision:

```toml
board-draw = { git = "https://github.com/sequoia-hope/board-draw", rev = "<commit>" }
```

**JS.** Add an npm git dependency at the same commit, in `package.json`:

```json
"board-draw": "github:sequoia-hope/board-draw#<commit>"
```

Then import it in your code:

```js
import { Renderer } from 'board-draw';
```

The package has no build step. Both kinds of dependency fetch over HTTPS from the public repo,
so CI builds (GitHub Pages included) need no keys.

To move to a new commit, change both pins together. Then run `cargo update -p board-draw` and
`npm install`.

To work on board-draw and an app together, point both pins at a local checkout:

- In Cargo, use `[patch."https://github.com/sequoia-hope/board-draw"] board-draw = { path = "../board-draw" }`.
- In npm, run `npm link` or install from `file:../board-draw`.

## Tests

```
cargo test
cargo check --features wasm --target wasm32-unknown-unknown
cargo run --release --example pose_update
```

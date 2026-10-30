//! Times moving every part of a 200-part board against drawing it from scratch.
//!
//!     cargo run --release --example pose_update

use std::time::Instant;

use board_draw::{AreaKind, Geom, Item, Pose, Prim, Scene};

fn main() {
    let mut s = Scene::new();
    s.set_layers(&["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"]);
    let mut id = 0u32;
    let mut next = || {
        id += 1;
        id
    };
    let parts = 200;
    for p in 1..=parts {
        s.set_part(p, &format!("U{p}"));
        // Sixteen pads round a 4 mm body, a courtyard and a body outline.
        for k in 0..16 {
            let (x, y) = (-1.75 + (k % 8) as f32 * 0.5, if k < 8 { -1.6 } else { 1.6 });
            let pad = Prim::Poly(vec![x - 0.15, y - 0.3, x + 0.15, y - 0.3, x + 0.15, y + 0.3, x - 0.15, y + 0.3]);
            s.insert(next(), Item::new(k + 1, p, Geom::Pad { at: [x, y], pads: vec![(0, pad)], drill: None }));
        }
        let body = vec![-2.0, -1.2, 2.0, -1.2, 2.0, 1.2, -2.0, 1.2];
        s.insert(next(), Item::new(0, p, Geom::Area { kind: AreaKind::Body, tiles: vec![(0, body)] }));
        let court = vec![-2.3, -2.1, 2.3, -2.1, 2.3, 2.1, -2.3, 2.1];
        s.insert(next(), Item::new(0, p, Geom::Area { kind: AreaKind::Courtyard, tiles: vec![(0, court)] }));
        s.set_pose(p, Pose::at((p % 20) as f32 * 6.0, (p / 20) as f32 * 6.0));
    }
    for t in 0..2000 {
        let (x, y) = ((t % 50) as f32 * 2.4, (t / 50) as f32 * 1.5);
        s.insert(
            next(),
            Item::new(
                t % 40,
                0,
                Geom::Track { layer: (t % 4) as usize, width: 0.2, points: vec![x, y, x + 1.0, y + 0.5, x + 2.0, y + 0.5] },
            ),
        );
    }
    for v in 0..300 {
        let at = [(v % 30) as f32 * 4.0, (v / 30) as f32 * 6.0 + 3.0];
        s.insert(next(), Item::new(v % 40, 0, Geom::Via { at, shape: Some(Prim::Circle([at[0], at[1], 0.3])), drill: Some(0.15) }));
    }

    let (px, mask) = (0.02, u64::MAX);
    let rounds = 200;
    let start = Instant::now();
    let mut d = s.draw(px, mask);
    for _ in 1..rounds {
        d = s.draw(px, mask);
    }
    let full = start.elapsed().as_secs_f64() / rounds as f64;
    let vertices = d.vertices.len() / board_draw::STRIDE;

    let start = Instant::now();
    for r in 0..rounds {
        for p in 1..=parts {
            let old = s.pose(p);
            s.set_pose(p, Pose { x: old.x + 0.01, rotation: (r as f32) * 3.0, ..old });
        }
        s.redraw(&mut d, px, mask);
    }
    let moved = start.elapsed().as_secs_f64() / rounds as f64;
    assert_eq!(d.vertices, s.draw(px, mask).vertices, "patched equals rebuilt");

    let start = Instant::now();
    let mut hits = 0;
    for k in 0..1000 {
        hits += s.part_at((k % 120) as f32, (k / 10) as f32 * 0.6).is_some() as usize;
    }
    let pick = start.elapsed().as_secs_f64() / 1000.0;

    let start = Instant::now();
    for r in 0..rounds {
        for p in 1..=parts {
            s.set_pose(p, Pose { x: r as f32, ..s.pose(p) });
        }
    }
    let poses = start.elapsed().as_secs_f64() / rounds as f64;
    println!("{parts} parts, {} items, {vertices} vertices", s.items().count());
    println!("draw from scratch:              {:8.3} ms", full * 1e3);
    println!("move every part (poses only):   {:8.3} ms", poses * 1e3);
    println!("move every part, turned, + redraw: {:8.3} ms", moved * 1e3);
    println!("part_at with every part turned: {:8.3} ms ({hits} hits)", pick * 1e3);
}

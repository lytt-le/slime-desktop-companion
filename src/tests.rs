//! Headless scenario tests for the pixel slime. Each also writes a contact sheet of
//! rendered frames to `target/snapshots/<name>.bmp`.

use std::collections::HashSet;

use crate::draw::{draw_body, draw_goo};
use crate::physics::{v2, Bounds, Platform, V2};
use crate::sim::{Antic, Kind, World, FRAME};
use crate::Eyes;

const R0: f32 = 56.0;
const G: f32 = 2400.0;
const CELL: f32 = 3.0;
const W: usize = 640;
const H: usize = 400;

fn plats() -> Vec<Platform> {
    vec![Platform { x0: 0.0, x1: W as f32, y: 380.0, id: 0 }, Platform { x0: 120.0, x1: 360.0, y: 220.0, id: 42 }]
}

fn world_at(center: V2) -> World {
    let mut w = World::new(&Bounds { left: 0.0, top: 0.0, right: W as f32, bottom: H as f32 }, CELL, R0, G);
    w.seed(12345);
    w.set_platforms(&plats());
    w.spawn(center);
    w
}

struct Sheet {
    frames: Vec<Vec<u32>>,
    eyes: Option<Eyes>,
}

impl Sheet {
    fn new() -> Sheet {
        Sheet { frames: Vec::new(), eyes: None }
    }

    fn capture(&mut self, w: &World) {
        let eyes = self.eyes.get_or_insert_with(|| Eyes::new(w));
        eyes.update(w, V2::ZERO, FRAME, 0.5);
        let mut frame = vec![0u32; W * H];
        let full = (0, 0, w.w - 1, w.h - 1);
        draw_goo(&mut frame, W, w, full);
        let mut body = vec![0u32; W * H];
        draw_body(&mut body, W, w, full, &eyes.blocks(w));
        for (d, &s) in frame.iter_mut().zip(&body) {
            let a = s >> 24;
            if a > 0 {
                let mix = |sc: u32, dc: u32| (sc + dc * (255 - a) / 255).min(255);
                *d = (mix(a, *d >> 24) << 24)
                    | (mix(s >> 16 & 255, *d >> 16 & 255) << 16)
                    | (mix(s >> 8 & 255, *d >> 8 & 255) << 8)
                    | mix(s & 255, *d & 255);
            }
        }
        self.frames.push(frame);
    }

    fn save(&self, name: &str) {
        let cols = 3;
        let rows = self.frames.len().div_ceil(cols).max(1);
        let (sw, sh) = (W * cols, H * rows);
        let mut img = vec![[0u8; 3]; sw * sh];
        for (k, f) in self.frames.iter().enumerate() {
            let (bx, by) = ((k % cols) * W, (k / cols) * H);
            for y in 0..H {
                for x in 0..W {
                    let checker = if ((x / 16) + (y / 16)) % 2 == 0 { 58.0 } else { 70.0 };
                    let mut bg = [checker, checker, checker + 8.0];
                    for p in plats() {
                        if y as f32 >= p.y && y as f32 <= p.y + 14.0 && x as f32 >= p.x0 && x as f32 <= p.x1 {
                            bg = [120.0, 110.0, 150.0];
                        }
                    }
                    let px = f[y * W + x];
                    let a = (px >> 24) as f32 / 255.0;
                    let c = [(px & 255) as f32, (px >> 8 & 255) as f32, (px >> 16 & 255) as f32];
                    img[(by + y) * sw + bx + x] =
                        [(c[0] + bg[0] * (1.0 - a)) as u8, (c[1] + bg[1] * (1.0 - a)) as u8, (c[2] + bg[2] * (1.0 - a)) as u8];
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/snapshots");
        std::fs::create_dir_all(&dir).unwrap();
        let row_bytes = (sw * 3).div_ceil(4) * 4;
        let mut out = Vec::with_capacity(54 + row_bytes * sh);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((54 + row_bytes * sh) as u32).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
        out.extend_from_slice(&(sw as i32).to_le_bytes());
        out.extend_from_slice(&(-(sh as i32)).to_le_bytes());
        out.extend_from_slice(&[1, 0, 24, 0]);
        out.extend_from_slice(&[0u8; 24]);
        for y in 0..sh {
            for x in 0..sw {
                out.extend_from_slice(&img[y * sw + x]);
            }
            out.resize(out.len() + row_bytes - sw * 3, 0);
        }
        std::fs::write(dir.join(format!("{name}.bmp")), out).unwrap();
    }
}

fn run(w: &mut World, frames: usize, sheet: &mut Sheet, every: usize) {
    for f in 0..frames {
        w.step(FRAME);
        if every > 0 && f % every == every - 1 {
            sheet.capture(w);
        }
    }
}

fn body_cells(w: &World) -> HashSet<(i32, i32)> {
    w.ents.iter().filter(|e| e.kind == Kind::Slime).map(|e| (e.x, e.y)).collect()
}

/// Sizes of 8-connected pieces of slime, largest first.
fn pieces(w: &World) -> Vec<usize> {
    let cells = body_cells(w);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &c in &cells {
        if !seen.insert(c) {
            continue;
        }
        let mut stack = vec![c];
        let mut n = 0;
        while let Some((x, y)) = stack.pop() {
            n += 1;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let q = (x + dx, y + dy);
                    if cells.contains(&q) && seen.insert(q) {
                        stack.push(q);
                    }
                }
            }
        }
        out.push(n);
    }
    out.sort_unstable_by(|a, b| b.cmp(a));
    out
}

fn extent(w: &World) -> (f32, f32) {
    let (x0, y0, x1, y1) = w.body_bounds().unwrap();
    (((x1 - x0 + 1) as f32) * CELL, ((y1 - y0 + 1) as f32) * CELL)
}

#[test]
fn lands_and_rests_still_as_a_mound() {
    let mut w = world_at(v2(240.0, 100.0));
    let n0 = w.slime_count();
    let mut sheet = Sheet::new();
    sheet.capture(&w);
    run(&mut w, 240, &mut sheet, 20);
    // How many cells shuffle around once settled?
    let before = body_cells(&w);
    run(&mut w, 60, &mut sheet, 0);
    let after = body_cells(&w);
    let moved = before.symmetric_difference(&after).count() / 2;
    eprintln!("PX dome {}", w.debug_dome());
    {
        let (x0, y0, x1, y1) = w.body_bounds().unwrap();
        let mut prof = Vec::new();
        for y in (y0..=y1).step_by(3) {
            prof.push((x0..=x1).filter(|&x| w.at(x, y) == Some(Kind::Slime)).count());
        }
        eprintln!("PX profile top->bottom widths {:?}", prof);
    }
    sheet.save("land");
    let (ew, eh) = extent(&w);
    eprintln!(
        "PX rest {ew:.0}x{eh:.0} ratio {:.2} cells {} -> {} moved/s {} pieces {:?} support {:?}",
        eh / ew,
        n0,
        w.slime_count(),
        moved,
        &pieces(&w)[..pieces(&w).len().min(3)],
        w.support()
    );
    assert_eq!(w.support(), Some(42));
    assert!(eh / ew > 0.3 && eh / ew < 0.75, "not a mound: {ew}x{eh}");
    assert!(moved < (n0 / 20).max(10), "boiling at rest: {moved} cells/s");
    assert_eq!(pieces(&w).len(), 1);
}

#[test]
fn tips_off_a_corner() {
    let mut w = world_at(v2(360.0 + R0 * 0.4, 150.0));
    let mut sheet = Sheet::new();
    sheet.capture(&w);
    run(&mut w, 240, &mut sheet, 12);
    sheet.save("corner");
    assert_eq!(w.support(), Some(0), "should end up on the floor");
}

#[test]
fn violent_shake_and_throw_stays_together() {
    let mut w = world_at(v2(240.0, 150.0));
    let mut sheet = Sheet::new();
    run(&mut w, 90, &mut sheet, 0);
    let n = w.slime_count();
    w.start_grab(w.centroid());
    let mut worst_holes = 0usize;
    let mut worst = 1.0f32;
    for i in 0..40 {
        let t = i as f32 * 0.35;
        w.drag_to(v2(300.0 + t.sin() * 90.0, 140.0 + (t * 1.7).cos() * 60.0));
        w.step(FRAME);
        worst_holes = worst_holes.max(holes(&w));
        let p = pieces(&w);
        worst = worst.min(p[0] as f32 / w.slime_count() as f32);
        if i % 8 == 0 {
            sheet.capture(&w);
        }
    }
    w.release(v2(3500.0, -900.0));
    for f in 0..240 {
        w.step(FRAME);
        let p = pieces(&w);
        worst = worst.min(p[0] as f32 / w.slime_count() as f32);
        if f % 16 == 0 {
            sheet.capture(&w);
        }
    }
    sheet.save("violent");
    let (ew, eh) = extent(&w);
    eprintln!("PX holes while shaking: {worst_holes}");
    eprintln!(
        "PX violent worst main share {worst:.2}, final {ew:.0}x{eh:.0} cells {} (start {n}) pieces {:?}",
        w.slime_count(),
        &pieces(&w)[..pieces(&w).len().min(3)]
    );
    assert!(worst > 0.8, "fell apart: main piece {worst:.2}");
    assert!(eh / ew > 0.3, "ended flat: {ew}x{eh}");
    assert_eq!(pieces(&w).len(), 1);
}

#[test]
fn slide_stops() {
    let mut w = world_at(v2(500.0, 330.0));
    let mut sheet = Sheet::new();
    run(&mut w, 90, &mut sheet, 0);
    w.start_grab(w.centroid());
    w.release(v2(-2500.0, 0.0));
    let x0 = w.centroid().x;
    run(&mut w, 120, &mut sheet, 10);
    let x1 = w.centroid().x;
    run(&mut w, 120, &mut sheet, 0);
    let x2 = w.centroid().x;
    sheet.save("slide");
    eprintln!("PX slide travelled {:.0}px, then {:.1}px more", x0 - x1, (x2 - x1).abs());
    assert!((x2 - x1).abs() < 6.0, "kept sliding");
}

#[test]
fn hops_toward_a_click() {
    let mut w = world_at(v2(240.0, 150.0));
    let mut sheet = Sheet::new();
    run(&mut w, 120, &mut sheet, 0);
    let start = w.centroid();
    assert!(w.jump_toward(v2(520.0, 250.0)), "couldn't jump");
    let mut top = start.y;
    for f in 0..90 {
        w.step(FRAME);
        top = top.min(w.centroid().y);
        if f < 40 && f % 4 == 0 { eprintln!("PX jf{f} c={:?} {}", w.centroid(), w.debug_jelly()); }
        if f % 8 == 0 {
            sheet.capture(&w);
        }
    }
    sheet.save("jump");
    eprintln!("PX jump rose {:.0}px, landed x {:.0}", start.y - top, w.centroid().x);
    assert!(top < start.y - 30.0);
    assert!((w.centroid().x - 520.0).abs() < 90.0, "landed at {}", w.centroid().x);
}

#[test]
fn grows_and_sheds_goo() {
    let mut w = world_at(v2(240.0, 150.0));
    let mut sheet = Sheet::new();
    run(&mut w, 90, &mut sheet, 0);
    let n1 = w.slime_count();
    w.set_scale(1.6);
    run(&mut w, 180, &mut sheet, 60);
    let n2 = w.slime_count();
    eprintln!("PX grow {n1} -> {n2} cells");
    assert!(n2 as f32 > n1 as f32 * 2.0, "didn't grow: {n1} -> {n2}");
    // Dragging sheds goo that falls and sticks.
    w.start_grab(w.centroid());
    let mut goo = 0;
    for i in 0..60 {
        w.drag_to(v2(240.0 + i as f32 * 4.0, 120.0));
        w.step(FRAME);
        goo = goo.max(w.ents.iter().filter(|e| e.kind == Kind::Goo).count());
    }
    w.release(V2::ZERO);
    run(&mut w, 60, &mut sheet, 30);
    sheet.save("grow_goo");
    eprintln!("PX goo cells {goo}");
    assert!(goo > 0, "no goo shed");
}

#[test]
fn perf() {
    let mut w = world_at(v2(240.0, 100.0));
    let t = std::time::Instant::now();
    for _ in 0..120 {
        w.step(FRAME);
    }
    let sim = t.elapsed() / 120;
    let mut buf = vec![0u32; W * H];
    let b = w.body_bounds().unwrap();
    let eyes = Eyes::new(&w).blocks(&w);
    let t = std::time::Instant::now();
    for _ in 0..60 {
        draw_body(&mut buf, W, &w, b, &eyes);
    }
    eprintln!("PX perf: sim {:?}/frame, draw {:?}/frame, {} cells", sim, t.elapsed() / 60, w.slime_count());
}

/// Empty cells enclosed by slime (5+ of their 8 neighbours are body).
fn holes(w: &World) -> usize {
    let Some((x0, y0, x1, y1)) = w.body_bounds() else { return 0 };
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if w.at(x, y).is_some() {
                continue;
            }
            let around = (-1..=1)
                .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                .filter(|&(dx, dy)| matches!(w.at(x + dx, y + dy), Some(Kind::Slime | Kind::Bubble)))
                .count();
            if around >= 5 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn straddling_a_window_edge_drops_through() {
    // Body spawned across the platform line, as after being thrown up through a window.
    let mut w = world_at(v2(240.0, 225.0));
    let mut sheet = Sheet::new();
    sheet.capture(&w);
    run(&mut w, 120, &mut sheet, 20);
    sheet.save("straddle");
    assert_eq!(w.support(), Some(0), "stuck on the edge it straddled");
    assert_eq!(pieces(&w).len(), 1);
}

#[test]
fn realistic_throw_looks_solid() {
    // Mirrors tools/e2e.ps1 "throw": a 400 ms drag then an 80 ms flick and release.
    let mut w = world_at(v2(200.0, 330.0));
    let mut sheet = Sheet::new();
    run(&mut w, 90, &mut sheet, 0);
    let start = w.centroid();
    w.start_grab(start);
    let frames = 24;
    for i in 1..=frames {
        let t = i as f32 / frames as f32;
        w.drag_to(start + (v2(330.0, 150.0) - start) * t);
        w.step(FRAME);
        if i % 6 == 0 {
            sheet.capture(&w);
        }
    }
    for i in 1..=5 {
        w.drag_to(v2(330.0, 150.0) + v2(80.0, -14.0) * (i as f32 / 5.0));
        w.step(FRAME);
    }
    sheet.capture(&w);
    w.release(v2(1200.0, -200.0));
    let mut worst_holes = 0;
    for f in 0..30 {
        w.step(FRAME);
        worst_holes = worst_holes.max(holes(&w));
        if f % 3 == 0 {
            sheet.capture(&w);
        }
    }
    sheet.save("throw_real");
    let (x0, y0, x1, y1) = w.body_bounds().unwrap();
    let mut kinds = [0usize; 4];
    for y in y0..=y1 {
        for x in x0..=x1 {
            match w.at(x, y) {
                Some(Kind::Slime) => kinds[0] += 1,
                Some(Kind::Bubble) => kinds[1] += 1,
                Some(Kind::Goo) => kinds[2] += 1,
                None => kinds[3] += 1,
            }
        }
    }
    eprintln!("PX throw_real holes after release {worst_holes}, bbox kinds slime/bubble/goo/empty {kinds:?}");
}

#[test]
fn never_wanders_into_gaps_between_monitors() {
    // A short monitor on the left, a tall one on the right: below the left one is a gap.
    let mut w = world_at(v2(520.0, 200.0));
    w.set_monitors(&[(0, 0, 400, 260), (400, 0, 640, 400)]);
    w.set_platforms(&[Platform { x0: 400.0, x1: 640.0, y: 380.0, id: 0 }]);
    let mut sheet = Sheet::new();
    run(&mut w, 90, &mut sheet, 0);
    w.start_grab(w.centroid());
    w.release(v2(-1500.0, 200.0));
    run(&mut w, 180, &mut sheet, 15);
    sheet.save("monitors");
    let in_gap = w.ents.iter().filter(|e| e.kind == Kind::Slime && e.x < 133 && e.y >= 87).count();
    eprintln!("PX gap cells {in_gap}, centroid {:?}", w.centroid());
    assert_eq!(in_gap, 0, "slime got into the gap below the left monitor");
}

#[test]
fn rides_a_moving_window() {
    let mut w = world_at(v2(240.0, 150.0));
    let mut sheet = Sheet::new();
    run(&mut w, 120, &mut sheet, 0);
    assert_eq!(w.support(), Some(42));
    let y0 = w.centroid().y;
    // The window moves down 6 px a frame, as the app sees it: support is judged before
    // the platforms are redrawn, then the body rides along.
    let mut top = 220.0;
    for f in 0..20 {
        let support = w.support();
        top += 6.0;
        w.set_platforms(&[Platform { x0: 0.0, x1: W as f32, y: 380.0, id: 0 }, Platform { x0: 120.0, x1: 360.0, y: top, id: 42 }]);
        if support == Some(42) {
            w.ride(42, v2(0.0, 6.0));
        }
        w.step(FRAME);
        if f % 5 == 0 {
            sheet.capture(&w);
        }
    }
    let moved = w.centroid().y - y0;
    eprintln!("PX ride moved {moved:.0}px with the window (120)");
    assert_eq!(w.support(), Some(42), "fell off the moving window");
    assert!((moved - 120.0).abs() < 12.0, "didn't keep up: {moved}");
    // And back up again, 5 px a frame (doesn't divide evenly into cells).
    for f in 0..24 {
        let support = w.support();
        top -= 5.0;
        w.set_platforms(&[Platform { x0: 0.0, x1: W as f32, y: 380.0, id: 0 }, Platform { x0: 120.0, x1: 360.0, y: top, id: 42 }]);
        if support == Some(42) {
            w.ride(42, v2(0.0, -5.0));
        }
        w.step(FRAME);
        if f % 6 == 0 {
            sheet.capture(&w);
        }
    }
    sheet.save("ride");
    assert_eq!(w.support(), Some(42), "fell through on the way up");
}

#[test]
fn landing_wobbles_like_jelly() {
    let mut w = world_at(v2(500.0, 120.0));
    let mut sheet = Sheet::new();
    let mut heights = Vec::new();
    for f in 0..150 {
        w.step(FRAME);
        if f % 3 == 0 {
            let (_, eh) = extent(&w);
            heights.push(eh as i32);
            if (40..80).contains(&f) { eprintln!("PX j{f} h{eh:.0} {}", w.debug_jelly()); }
        }
        if (18..60).contains(&f) && f % 3 == 0 {
            sheet.capture(&w);
        }
    }
    sheet.save("jelly_land");
    let (ew, eh) = extent(&w);
    eprintln!("PX jelly heights every 3 frames: {heights:?}");
    eprintln!("PX jelly rest {ew:.0}x{eh:.0}");
    // It should squash below its rest height and spring back above it at least once.
    let rest = *heights.last().unwrap();
    let min = *heights.iter().skip(5).min().unwrap();
    let after_min = heights.iter().position(|&h| h == min).unwrap();
    let rebound = heights[after_min..].iter().max().unwrap();
    assert!(min < rest - 6, "no squash on landing: min {min} rest {rest}");
    assert!(*rebound > rest, "no spring-back: rebound {rebound} rest {rest}");
}

/// Each idle antic plays out, does what it says, and leaves him whole and settled.
#[test]
fn idle_antics() {
    let mut sheet = Sheet::new();
    let settle = |w: &mut World, sheet: &mut Sheet| {
        run(w, 150, sheet, 0);
        assert!(w.resting(), "not resting before the antic");
    };
    let done = |w: &mut World, name: &str| {
        assert_eq!(w.antic(), None, "{name} never finished");
        let p = pieces(w);
        assert!(p.len() == 1 || p[1] < 5, "{name} tore him apart: {p:?}");
    };

    // Walking along the floor: goes somewhere, a bit, and turns at the screen edge.
    let mut w = world_at(v2(500.0, 330.0));
    settle(&mut w, &mut sheet);
    let x0 = w.centroid().x;
    assert!(w.start_antic(Antic::Walk(-1.0)));
    let mut far = 0.0f32;
    for f in 0..300 {
        w.step(FRAME);
        far = far.max((w.centroid().x - x0).abs());
        if f % 20 == 0 {
            sheet.capture(&w);
        }
    }
    eprintln!("PX walk went {far:.0}px");
    done(&mut w, "walk");
    assert!(far > 20.0 && far < 400.0, "walked {far}px");
    // Walking on a window turns round at its edge instead of falling off.
    let mut w = world_at(v2(240.0, 150.0));
    settle(&mut w, &mut sheet);
    assert!(w.centroid().y < 220.0, "fell off the window before walking");
    assert!(w.start_antic(Antic::Walk(-1.0)));
    for f in 0..300 {
        w.step(FRAME);
        if f % 30 == 0 {
            sheet.capture(&w);
        }
    }
    assert!(w.centroid().y < 220.0, "walked off the window");

    // Hops: three little ones, landing each time.
    let mut w = world_at(v2(300.0, 330.0));
    settle(&mut w, &mut sheet);
    let start = w.centroid();
    assert!(w.start_antic(Antic::Hop(1.0, 3)));
    let (mut top, mut landings, mut air) = (start.y, 0, false);
    for f in 0..400 {
        w.step(FRAME);
        top = top.min(w.centroid().y);
        if air && w.resting() {
            landings += 1;
        }
        air = !w.resting();
        if f % 10 == 0 {
            sheet.capture(&w);
        }
    }
    eprintln!("PX hops rose {:.0}px, moved {:.0}px, {landings} landings", start.y - top, w.centroid().x - start.x);
    done(&mut w, "hop");
    assert!(start.y - top > 10.0 && start.y - top < 80.0, "hop height {}", start.y - top);
    assert!(landings >= 3, "only {landings} landings");
    assert!(w.centroid().x > start.x + 20.0);

    // Flatten, stretch: the jelly goes low, then tall.
    let mut w = world_at(v2(500.0, 330.0));
    settle(&mut w, &mut sheet);
    let (_, h0) = extent(&w);
    assert!(w.start_antic(Antic::Flatten));
    let mut low = h0;
    for f in 0..60 {
        w.step(FRAME);
        low = low.min(extent(&w).1);
        if f % 10 == 0 {
            sheet.capture(&w);
        }
    }
    run(&mut w, 120, &mut sheet, 20);
    done(&mut w, "flatten");
    assert!(w.start_antic(Antic::Stretch));
    let mut high = h0;
    for f in 0..90 {
        w.step(FRAME);
        high = high.max(extent(&w).1);
        if f % 10 == 0 {
            sheet.capture(&w);
        }
    }
    run(&mut w, 60, &mut sheet, 0);
    done(&mut w, "stretch");
    eprintln!("PX rest height {h0:.0}, flat {low:.0}, tall {high:.0}");
    assert!(low < h0 * 0.75, "didn't flatten: {low} vs {h0}");
    assert!(high > h0 * 1.15, "didn't stretch: {high} vs {h0}");

    // Wobble: leans both ways.
    assert!(w.start_antic(Antic::Wobble));
    let (mut left, mut right) = (0.0f32, 0.0f32);
    for f in 0..120 {
        w.step(FRAME);
        let (_, lean) = w.jelly();
        left = left.min(lean);
        right = right.max(lean);
        if f % 8 == 0 {
            sheet.capture(&w);
        }
    }
    eprintln!("PX wobble lean {left:.2}..{right:.2}");
    done(&mut w, "wobble");
    assert!(left < -0.08 && right > 0.08, "barely wobbled: {left}..{right}");

    // Bubbles: a burst at once.
    let bubbles = |w: &World| w.ents.iter().filter(|e| e.kind == Kind::Bubble).count();
    let b0 = bubbles(&w);
    assert!(w.start_antic(Antic::Bubbles));
    let b1 = bubbles(&w);
    for f in 0..60 {
        w.step(FRAME);
        if f % 6 == 0 {
            sheet.capture(&w);
        }
    }
    eprintln!("PX bubbles {b0} -> {b1}");
    assert!(b1 >= b0 + 5, "only {b0} -> {b1} bubbles");

    // Nothing starts while he's held, and grabbing stops one.
    w.start_grab(w.centroid());
    assert!(!w.start_antic(Antic::Wobble));
    w.release(V2::ZERO);
    run(&mut w, 150, &mut sheet, 0);
    assert!(w.start_antic(Antic::Walk(1.0)));
    w.start_grab(w.centroid());
    assert_eq!(w.antic(), None);
    sheet.save("antics");
}

/// Only a window covering a whole monitor, taskbar and all, without a title bar, counts
/// as fullscreen: not a maximized one.
#[test]
fn spots_fullscreen_windows() {
    use crate::desktop::fullscreen_on;
    use windows_sys::Win32::Foundation::RECT;
    let rect = |left, top, right, bottom| RECT { left, top, right, bottom };
    let mons = [rect(0, 0, 1920, 1080), rect(1920, -278, 3000, 1642)];
    let fs = |r: RECT, outer: RECT, captioned| fullscreen_on(&r, &outer, &mons, captioned);
    // F11 on either monitor (as Firefox on the real desktop: frame and window exactly the screen).
    assert_eq!(fs(rect(0, 0, 1920, 1080), rect(0, 0, 1920, 1080), false), Some(0));
    assert_eq!(fs(rect(1920, -278, 3000, 1642), rect(1920, -278, 3000, 1642), false), Some(1));
    // Maximized (as Discord on the real desktop): title bar, and the window hangs 8 px past
    // the edges; with the taskbar hidden its frame can cover the whole screen.
    assert_eq!(fs(rect(1920, -278, 3000, 1594), rect(1912, -286, 3008, 1602), true), None);
    assert_eq!(fs(rect(0, 0, 1920, 1080), rect(-8, -8, 1928, 1088), false), None);
    assert_eq!(fs(rect(0, 0, 1920, 1080), rect(0, 0, 1920, 1080), true), None);
    // Filling just the work area, or nearly the screen.
    assert_eq!(fs(rect(0, 0, 1920, 1032), rect(0, 0, 1920, 1032), false), None);
    assert_eq!(fs(rect(10, 0, 1920, 1080), rect(10, 0, 1920, 1080), false), None);
    // Spanning both.
    assert_eq!(fs(rect(0, -278, 3000, 1642), rect(0, -278, 3000, 1642), false), None);
}

/// He can leap from one monitor to the other, both ways, on the real two-monitor layout
/// (a landscape screen beside a taller portrait one).
#[test]
fn leaps_between_monitors() {
    let mons = [(0, 0, 1920, 1080), (1920, -278, 3000, 1642)];
    let bounds = Bounds { left: 0.0, top: -278.0, right: 3000.0, bottom: 1642.0 };
    let mut w = World::new(&bounds, CELL, R0, G);
    w.seed(9);
    w.set_monitors(&mons);
    w.set_platforms(&[Platform { x0: 0.0, x1: 1920.0, y: 1032.0, id: 0 }, Platform { x0: 1920.0, x1: 3000.0, y: 1642.0, id: 0 }]);
    w.spawn(v2(700.0, 900.0));
    let start = w.slime_count();
    let mut sheet = Sheet::new();
    run(&mut w, 120, &mut sheet, 0);
    for (to, on) in [(v2(2460.0, 1642.0 - R0), 1), (v2(960.0, 1032.0 - R0), 0), (v2(2460.0, 1642.0 - R0), 1)] {
        // (What main.rs's leap_gate works out for this layout.)
        let gate_x = if on == 1 { 1920.0 + R0 * 1.6 } else { 1920.0 - R0 * 1.6 };
        assert!(w.leap_to(to, Some(v2(gate_x, 1032.0 - R0 * 1.5)), 0.0), "couldn't leap");
        let mut top = f32::MAX;
        for _ in 0..240 {
            w.step(FRAME);
            top = top.min(w.centroid().y);
        }
        let c = w.centroid();
        let (l, t, r, b) = mons[on];
        eprintln!("PX leapt to {c:?} (aimed {to:?}), peak y {top:.0}");
        assert!(c.x >= l as f32 && c.x < r as f32 && c.y >= t as f32 && c.y < b as f32, "landed at {c:?}, not on monitor {on}");
        assert!(w.resting(), "not settled");
        assert!(w.slime_count() * 10 > start * 7, "lost too much on the way ({} of {start})", w.slime_count());
    }
}

/// Lists what the real desktop counts as fullscreen right now (nothing, normally).
#[test]
#[ignore]
fn real_fullscreen_windows() {
    let scan = crate::desktop::scan(std::ptr::null_mut());
    for &(h, m) in &scan.fullscreen {
        let mut t = [0u16; 128];
        let n = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW(h as _, t.as_mut_ptr(), 128) }.max(0) as usize;
        eprintln!("PX fullscreen on monitor {m}: {}", String::from_utf16_lossy(&t[..n]));
    }
    eprintln!("PX {} fullscreen of {} monitors", scan.fullscreen.len(), scan.monitors.len());
}

/// Only a hard throw into a wall makes him cross, and only a hard shaking makes him
/// dizzy: ordinary throws, bumps, carrying and moderate shaking don't.
#[test]
fn notices_wall_hits_and_shaking() {
    use crate::fx::{ANGRY_HIT, DIZZY_SHAKE};
    // Flung sideways through the air at the screen edge (the fastest throw is 3500 px/s).
    let throw = |speed: f32| {
        let mut w = world_at(v2(250.0, 330.0));
        let mut sheet = Sheet::new();
        run(&mut w, 120, &mut sheet, 0);
        w.start_grab(w.centroid());
        for k in 0..10 {
            w.drag_to(v2(250.0, 330.0 - k as f32 * 15.0));
            w.step(FRAME);
        }
        w.take_wall_hit();
        w.release(v2(-speed, -speed * 0.2));
        let mut hit = 0.0f32;
        for _ in 0..90 {
            w.step(FRAME);
            hit = hit.max(w.take_wall_hit());
        }
        hit
    };
    let hits: Vec<(f32, f32)> = [1000.0, 2000.0, 2500.0, 3000.0, 3500.0].iter().map(|&sp| (sp, throw(sp))).collect();
    eprintln!("PX wall hits (throw px/s, hit): {hits:?}");
    for &(sp, hit) in &hits {
        assert_eq!(hit > ANGRY_HIT, sp >= 3000.0, "a {sp} px/s throw scored {hit}");
    }

    // Held and shaken side to side (shakes a second, px each way), or carried along.
    let shake = |hz: f32, amp: f32| {
        let mut w = world_at(v2(320.0, 330.0));
        let mut sheet = Sheet::new();
        run(&mut w, 120, &mut sheet, 0);
        let at = w.centroid();
        w.start_grab(at);
        let mut most = 0.0f32;
        for f in 0..120 {
            let t = f as f32 * FRAME;
            let p = if hz > 0.0 {
                at + v2((t * hz * std::f32::consts::TAU).sin() * amp, -40.0)
            } else {
                at + v2(t * 200.0, -40.0 + (t * 3.0).sin() * 20.0)
            };
            w.drag_to(p);
            w.step(FRAME);
            most = most.max(w.shake());
        }
        most
    };
    let shakes: Vec<((f32, f32), f32)> =
        [(0.0, 0.0), (2.0, 120.0), (3.0, 80.0), (4.0, 80.0), (4.0, 150.0), (6.0, 100.0), (6.0, 200.0)].iter().map(|&(hz, amp)| ((hz, amp), shake(hz, amp))).collect();
    eprintln!("PX shake ((Hz, px), meter): {shakes:?}");
    for &((hz, amp), m) in &shakes {
        assert_eq!(m > DIZZY_SHAKE, hz * amp >= 600.0, "shaking {hz} Hz x {amp} px scored {m}");
    }
}

/// A frame of the body with its effects layer (Z's, smoke, the pin) on top.
fn capture_with_fx(w: &World, fx: &crate::fx::Fx, sheet: &mut Sheet) {
    let mut frame = vec![0u32; W * H];
    let full = (0, 0, w.w - 1, w.h - 1);
    let mut eyes = Eyes::new(w);
    eyes.update(w, V2::ZERO, FRAME, 0.5);
    let mut face = eyes.blocks(w);
    fx.style_eyes(&mut face);
    draw_body(&mut frame, W, w, full, &face);
    let mut layer = vec![0u32; 600 * 600];
    if let Some((x, y, lw, lh)) = fx.draw(&mut layer, 600, 600, w) {
        for yy in 0..lh {
            for xx in 0..lw {
                let (fx_, fy) = (x + xx as i32, y + yy as i32);
                let s = layer[yy * 600 + xx];
                let a = s >> 24;
                if a > 0 && fx_ >= 0 && fy >= 0 && (fx_ as usize) < W && (fy as usize) < H {
                    let d = &mut frame[fy as usize * W + fx_ as usize];
                    let mix = |sc: u32, dc: u32| (sc + dc * (255 - a) / 255).min(255);
                    *d = (mix(a, *d >> 24) << 24) | (mix(s >> 16 & 255, *d >> 16 & 255) << 16) | (mix(s >> 8 & 255, *d >> 8 & 255) << 8) | mix(s & 255, *d & 255);
                }
            }
        }
    }
    sheet.frames.push(frame);
}

/// Pinned while held, he stays hanging on the pin when let go (dripping, as when carried,
/// without draining away), until grabbed again, when the pin pops out and falls away.
#[test]
fn pinned_to_the_screen() {
    use crate::fx::Fx;
    let mut w = world_at(v2(320.0, 330.0));
    let mut sheet = Sheet::new();
    run(&mut w, 120, &mut sheet, 0);
    // Picked up by the top and lifted up to the middle of the screen.
    let at = w.centroid() - v2(0.0, R0 * 0.5);
    w.start_grab(at);
    let pin_at = v2(400.0, 150.0);
    for k in 1..=20 {
        w.drag_to(at + (pin_at - at) * (k as f32 / 20.0));
        w.step(FRAME);
    }
    assert!(w.pin());
    // Let go (main.rs ignores the release while pinned) and wait.
    let start = w.slime_count();
    let mut fx = Fx::new(3);
    let mut dripped = 0;
    for f in 0..1200 {
        dripped = dripped.max(w.ents.iter().filter(|e| e.kind == Kind::Goo).count());
        w.step(FRAME);
        fx.pin = w.pin_point();
        fx.update(FRAME, &w);
        if f % 200 == 199 {
            capture_with_fx(&w, &fx, &mut sheet);
        }
    }
    let c = w.centroid();
    eprintln!("PX pinned at {pin_at:?}: centroid {c:?}, up to {dripped} goo cells, slime {start} -> {}", w.slime_count());
    assert!((c.x - pin_at.x).abs() < R0 && c.y > pin_at.y - R0 && c.y < pin_at.y + R0 * 2.0, "fell off the pin: {c:?}");
    assert!(dripped > 0, "didn't drip while pinned");
    assert!(w.slime_count() * 10 > start * 8, "drained away: {} of {start}", w.slime_count());
    let p = w.pin_point().unwrap();
    assert!((p - pin_at).len() < 4.0, "pin moved to {p:?}");
    let mut layer = vec![0u32; 600 * 600];
    assert!(fx.draw(&mut layer, 600, 600, &w).is_some(), "no pin drawn");
    // Grabbed again: the pin pops out and falls away.
    fx.pull_pin(p, w.scale);
    w.pinned = false;
    w.start_grab(w.centroid());
    w.release(V2::ZERO);
    assert!(!w.pinned && w.pin_point().is_none());
    for _ in 0..120 {
        w.step(FRAME);
        fx.update(FRAME, &w);
    }
    assert!(!fx.visible(), "the pulled pin never went");
    sheet.save("pinned");
}

/// Keepy uppies: dropped onto a cursor that follows him about (as a player would), he
/// keeps bouncing off it without ever landing, with the count beside him; take the
/// cursor away and he lands.
#[test]
fn keepy_uppies() {
    use crate::fx::Fx;
    // Off the right of the raised platform, well above the floor.
    let mut w = world_at(v2(480.0, 120.0));
    let mut sheet = Sheet::new();
    let mut fx = Fx::new(5);
    let mut cursor = v2(480.0, 330.0);
    let mut bounces = 0;
    let mut highest = f32::MAX;
    for f in 0..600 {
        // Sliding under him, but no faster than a hand would.
        cursor.x += (w.centroid().x - cursor.x).clamp(-8.0, 8.0);
        if w.keepy_up(cursor) {
            bounces += 1;
        }
        w.step(FRAME);
        assert!(!w.resting(), "landed after {bounces} bounces (frame {f})");
        if bounces > 2 {
            highest = highest.min(w.centroid().y);
        }
        fx.keepy = (bounces > 0).then_some((bounces, 1.0, bounces > 5));
        if f % 60 == 30 {
            capture_with_fx(&w, &fx, &mut sheet);
        }
    }
    let c = w.centroid();
    eprintln!("KEEPY {bounces} bounces in 10 s, highest centroid y {highest:.0}, now {c:?}");
    assert!(bounces >= 8, "only {bounces} bounces in 10 s");
    assert!(highest < cursor.y - R0 * 2.5 && highest > 0.0, "bounces peaked at y {highest}");
    let mut layer = vec![0u32; 600 * 600];
    assert!(fx.draw(&mut layer, 600, 600, &w).is_some(), "no count drawn");
    // The cursor goes; he comes down and stays down.
    for _ in 0..240 {
        assert!(!w.keepy_up(v2(100.0, 100.0)));
        w.step(FRAME);
    }
    assert!(w.resting(), "never landed once the cursor left");
    capture_with_fx(&w, &fx, &mut sheet);
    sheet.save("keepy");
}

/// His moods, drawn: cross brows, spinning dizzy eyes, asleep with Z's, smoking.
#[test]
fn moods_render() {
    use crate::fx::Fx;
    let mut sheet = Sheet::new();
    let shot = capture_with_fx;
    let mut w = world_at(v2(320.0, 330.0));
    run(&mut w, 150, &mut sheet, 0);
    let mut fx = Fx::new(5);
    shot(&w, &fx, &mut sheet);
    fx.hit_wall(crate::fx::ANGRY_HIT + 1.0);
    assert!(fx.angry > 0.0);
    shot(&w, &fx, &mut sheet);
    fx.angry = 0.0;
    fx.shaken(crate::fx::DIZZY_SHAKE + 1.0);
    for f in 0..12 {
        fx.update(FRAME, &w);
        w.sway(fx.sway(FRAME));
        w.step(FRAME);
        if f % 4 == 0 {
            shot(&w, &fx, &mut sheet);
        }
    }
    fx.dizzy = 0.0;
    fx.fall_asleep(60.0);
    for f in 0..300 {
        w.rest_bias = fx.rest_bias();
        fx.update(FRAME, &w);
        w.step(FRAME);
        if f % 60 == 59 {
            shot(&w, &fx, &mut sheet);
        }
    }
    assert!(fx.visible(), "no Z's");
    assert!(fx.wake());
    w.rest_bias = 0.0;
    fx.light_up(40.0, 1.0);
    for f in 0..600 {
        fx.update(FRAME, &w);
        w.step(FRAME);
        if f % 60 == 59 {
            shot(&w, &fx, &mut sheet);
        }
    }
    assert!(fx.busy());
    // Picked up: the cigarette goes, the last wisp drifts off.
    fx.stub_out(&w);
    assert!(!fx.busy());
    for _ in 0..400 {
        fx.update(FRAME, &w);
    }
    assert!(!fx.visible(), "smoke never cleared");
    sheet.save("moods");
}

#[test]
fn speech_bubble_renders() {
    use crate::font::{to_ascii, wrap};
    use crate::ui::{draw_bubble, BUBBLE_COLS};
    let lines = wrap(&to_ascii("Hi! I'm Slime \u{2014} the quick brown fox jumps over the lazy dog. 0123456789 ?!.,:;()"), BUBBLE_COLS);
    let mut sheet = Sheet::new();
    let mut frame = vec![0u32; W * H];
    let total: usize = lines.iter().map(|l| l.len() + 1).sum();
    let mut bubble = vec![0u32; W * H];
    let (bw, bh) = draw_bubble(&mut bubble, W, &lines, total, 0.5, 2);
    for y in 0..bh {
        frame[(20 + y) * W + 20..(20 + y) * W + 20 + bw].copy_from_slice(&bubble[y * W..y * W + bw]);
    }
    sheet.frames.push(frame);
    sheet.save("ui");
    assert!(bw > 100 && bh > 40);
    let max = BUBBLE_COLS * crate::font::ADVANCE - 1;
    assert!(lines.iter().all(|l| crate::font::text_width(l) <= max));
}

#[test]
fn settings_panel_renders_and_hits() {
    use crate::brain::{KeySource, SessionState};
    use crate::settings::{self, Settings};
    use crate::ui::{chat_lines, draw_panel, panel_hit, panel_size, slider_at, Hit, PanelView, Section, SECTION_N, TAB_CHAT, TAB_LOOK};
    const TAB_SETTINGS: usize = 1;
    let chat: Vec<(bool, String)> = vec![
        (true, "what's a slime mold?".into()),
        (false, "A slime mold is a brainless blob that can still solve mazes to find food. My kind of hero! *wobble*".into()),
        (true, "can you remember things?".into()),
    ];
    let p = 2;
    let (pw, ph) = panel_size(p);
    assert!(pw <= W && ph <= 2 * H, "panel {pw}x{ph} too big");
    // Panels are taller than a sheet frame: draw into a double-height frame, split in two.
    let mut sheet = Sheet::new();
    let mut cfg = Settings::default();
    let view = |cfg: &Settings,
                tab: usize,
                key: Option<&(String, KeySource)>,
                session: &SessionState,
                hover: Option<Hit>,
                chat: &[(bool, String)],
                shot: Option<crate::ui::ShotView>| {
        let (key_text, status) = crate::panel_texts(cfg, key, session, true);
        let mut frame = vec![0u32; W * 2 * H];
        let v = PanelView {
            tab,
            chat,
            thinking: chat.last().is_some_and(|m| m.0),
            input: if chat.is_empty() || chat.len() == 1 { "" } else { "and what about jelly" },
            cursor_on: shot.is_none(),
            brain_line: "Brain: Claude plan, Haiku",
            shot,
            size: crate::size_to_slider(cfg.size),
            size_normal: crate::size_to_slider(1.0),
            size_pct: (cfg.size * 100.0).round() as u32,
            jump: cfg.jump,
            facts: cfg.facts,
            idle: cfg.idle,
            web: cfg.web,
            keepy: cfg.keepy,
            keepy_best: cfg.keepy_best,
            provider: cfg.provider.label(),
            model: cfg.model().1,
            key: &key_text,
            key_buttons: cfg.provider.needs_key(),
            key_clearable: key.is_some(),
            status: &status,
            look_labels: settings::LOOK.map(|l| l.1),
            look: cfg.look,
            look_normal: settings::LOOK_DEFAULT,
            swatches: settings::swatch_colours(),
            swatch_on: settings::SWATCHES.iter().position(|&(h, v)| cfg.look[0] == h && cfg.look[1] == v),
            rainbow: cfg.rainbow,
            nootropic: cfg.nootropic,
            collapsed: cfg.collapsed,
            hover,
        };
        draw_panel(&mut frame, W, &v, p);
        frame
    };
    let mut frames = Vec::new();
    // A stand-in screenshot thumbnail.
    let (tw, th) = (88, 44);
    let thumb: Vec<u32> =
        (0..tw * th).map(|i| 0xFF00_0000 | (((i % tw) * 255 / tw) as u32) << 16 | 0x30 << 8 | ((i / tw) * 255 / th) as u32).collect();
    let shot = || Some(crate::ui::ShotView { label: "Slime mold - Wikipedia - Google Chrome", thumb: &thumb, w: tw, h: th });
    frames.push(view(&cfg, TAB_CHAT, None, &SessionState::Ready, Some(Hit::Shoot), &[], None));
    frames.push(view(&cfg, TAB_CHAT, None, &SessionState::Ready, Some(Hit::Tab(1)), &chat, None));
    frames.push(view(&cfg, TAB_CHAT, None, &SessionState::Ready, Some(Hit::DropShot), &chat[..1], shot()));
    frames.push(view(&cfg, TAB_SETTINGS, None, &SessionState::Ready, Some(Hit::Jump), &chat, None));
    cfg.cycle_provider(-1); // DeepSeek
    cfg.size = 1.6;
    let key = ("sk-0123456789abcdef9Qa1".to_string(), KeySource::File);
    frames.push(view(&cfg, TAB_SETTINGS, Some(&key), &SessionState::Closed, Some(Hit::Tab(TAB_LOOK)), &chat, None));
    (cfg.look[0], cfg.look[1]) = settings::SWATCHES[3];
    cfg.look[3] = 0.85;
    cfg.look[5] = 0.1;
    frames.push(view(&cfg, TAB_LOOK, None, &SessionState::Closed, Some(Hit::Tune(4)), &chat, None));
    cfg.nootropic = true;
    cfg.keepy = true;
    cfg.keepy_best = 23;
    frames.push(view(&cfg, TAB_SETTINGS, None, &SessionState::Closed, None, &chat, None));
    cfg.rainbow = true;
    frames.push(view(&cfg, TAB_LOOK, None, &SessionState::Closed, None, &chat, None));
    cfg.nootropic = false;
    frames.push(view(&cfg, TAB_LOOK, None, &SessionState::Closed, None, &chat, None));
    // Sections closed: Features on Settings (hovering Brain's title), Colour on Customisation.
    cfg.collapsed = [true, false, true, false];
    frames.push(view(&cfg, TAB_SETTINGS, None, &SessionState::Closed, Some(Hit::Title(Section::Brain)), &chat, None));
    frames.push(view(&cfg, TAB_LOOK, None, &SessionState::Closed, None, &chat, None));
    for f in frames {
        sheet.frames.push(f[..W * H].to_vec());
        sheet.frames.push(f[W * H..].to_vec());
    }
    sheet.save("panel");

    // Every control is where it's drawn.
    let open = [false; SECTION_N];
    let at_c = |tab: usize, collapsed: &[bool; SECTION_N], x: usize, y: usize| {
        panel_hit(x as i32 * p as i32, y as i32 * p as i32, tab, true, false, collapsed, p)
    };
    let at = |tab: usize, x: usize, y: usize| at_c(tab, &open, x, y);
    assert_eq!(at(TAB_CHAT, 204, 5), Some(Hit::Close));
    assert_eq!(at(TAB_CHAT, 15, 20), Some(Hit::Tab(0)));
    assert_eq!(at(TAB_CHAT, 45, 20), Some(Hit::Tab(1)));
    assert_eq!(at(TAB_CHAT, 95, 20), Some(Hit::Tab(2)));
    assert_eq!(at(TAB_CHAT, 130, 20), None); // past the last tab
    assert_eq!(at(TAB_CHAT, 100, 100), None); // the log
    assert_eq!(at(TAB_CHAT, 100, 200), None); // the input line
    assert_eq!(at(TAB_CHAT, 100, 178), Some(Hit::Shoot));
    assert_eq!(at(TAB_CHAT, 180, 216), Some(Hit::ClearChat));
    assert_eq!(at(TAB_CHAT, 60, 216), None); // the brain line
    assert_eq!(at(1, 180, 216), Some(Hit::Quit)); // only on the Chat tab
    let with_shot = |x: usize, y: usize| panel_hit(x as i32 * 2, y as i32 * 2, TAB_CHAT, false, true, &open, p);
    assert_eq!(with_shot(100, 178), None); // the thumbnail and label
    assert_eq!(with_shot(200, 178), Some(Hit::DropShot));
    const S: usize = TAB_SETTINGS;
    assert_eq!(at(S, 100, 35), Some(Hit::Title(Section::Features))); // the whole title row
    assert_eq!(at(S, 30, 47), Some(Hit::Jump));
    assert_eq!(at(S, 30, 59), Some(Hit::Facts));
    assert_eq!(at(S, 30, 71), Some(Hit::Idle));
    assert_eq!(at(S, 30, 83), Some(Hit::Keepy));
    assert_eq!(at(S, 100, 97), Some(Hit::Title(Section::Brain)));
    assert_eq!(at(S, 62, 109), Some(Hit::ProviderPrev));
    assert_eq!(at(S, 150, 109), Some(Hit::ProviderNext));
    assert_eq!(at(S, 150, 122), Some(Hit::ModelNext));
    assert_eq!(at(S, 30, 135), Some(Hit::Web));
    assert_eq!(at(S, 140, 148), Some(Hit::PasteKey));
    assert_eq!(at(S, 180, 148), Some(Hit::ClearKey));
    assert_eq!(at(S, 100, 161), None); // status line
    assert_eq!(at(S, 20, 216), Some(Hit::Reset));
    assert_eq!(at(S, 190, 216), Some(Hit::Quit));
    assert_eq!(panel_hit(140 * 2, 148 * 2, S, false, false, &open, p), None); // no key buttons for the Claude plan
    // A closed section shows only its title, and the rest moves up.
    let shut = [true, false, true, false];
    assert_eq!(at_c(S, &shut, 30, 47), Some(Hit::Title(Section::Brain)));
    assert_eq!(at_c(S, &shut, 62, 61), Some(Hit::ProviderPrev));
    assert_eq!(at_c(TAB_LOOK, &shut, 100, 50), Some(Hit::Title(Section::Colour)));
    assert_eq!(at_c(TAB_LOOK, &shut, 100, 64), Some(Hit::Title(Section::Feel)));
    assert_eq!(at_c(TAB_LOOK, &shut, 120, 76), Some(Hit::Tune(3)));
    assert!(!(0..230).any(|y| at_c(TAB_LOOK, &shut, 120, y) == Some(Hit::Tune(0))), "a closed section's slider is still clickable");
    // The log shows the newest lines, with a thinking "..." while waiting.
    let lines = chat_lines(&chat, true);
    assert!(lines[0].1.starts_with("> what's") && lines.last().unwrap().1 == "...");
    let long: Vec<(bool, String)> = (0..30).map(|i| (i % 2 == 0, format!("message {i}"))).collect();
    let lines = chat_lines(&long, false);
    assert!(lines.len() <= 14 && lines.last().unwrap().1 == "message 29");
    assert_eq!(at(TAB_LOOK, 100, 35), Some(Hit::Slider)); // size, now on Customisation
    assert_eq!(at(TAB_LOOK, 100, 50), Some(Hit::Title(Section::Colour)));
    assert_eq!(at(TAB_LOOK, 10, 63), Some(Hit::Swatch(0)));
    assert_eq!(at(TAB_LOOK, 200, 63), Some(Hit::Swatch(8)));
    assert_eq!(at(TAB_LOOK, 120, 77), Some(Hit::Tune(0)));
    assert_eq!(at(TAB_LOOK, 120, 101), Some(Hit::Tune(2)));
    assert_eq!(at(TAB_LOOK, 100, 115), Some(Hit::Title(Section::Feel)));
    assert_eq!(at(TAB_LOOK, 120, 127), Some(Hit::Tune(3)));
    assert_eq!(at(TAB_LOOK, 120, 199), Some(Hit::Tune(9)));
    assert_eq!(at(TAB_LOOK, 100, 216), Some(Hit::ResetLook));
    // Sliders span their range and snap to the standard setting.
    assert_eq!(crate::slider_to_size(slider_at(Hit::Slider, 0, p)), crate::settings::MIN_SIZE);
    assert_eq!(crate::slider_to_size(slider_at(Hit::Slider, 10_000, p)), crate::settings::MAX_SIZE);
    assert_eq!(crate::slider_to_size(crate::size_to_slider(1.02)), 1.0);
    assert_eq!(slider_at(Hit::Tune(5), 10_000, p), 1.0);
    let mut c = Settings::default();
    crate::set_slider(&mut c, Hit::Tune(5), 0.51);
    assert_eq!(c.look[5], 0.5);
}

/// The Customisation sliders really change him: no goo with Splatter at 0, lots at the
/// top; runny spreads flatter than thick; and colours follow the palette.
#[test]
fn customisation_changes_the_slime() {
    use crate::settings::Settings;
    let shed_with = |splatter: f32| {
        let mut cfg = Settings::default();
        cfg.look[5] = splatter;
        let mut w = world_at(v2(240.0, 150.0));
        w.tune = cfg.tuning();
        run(&mut w, 60, &mut Sheet::new(), 0);
        let c = w.centroid();
        w.start_grab(c);
        for k in 0..90 {
            let t = k as f32 * 0.12;
            w.drag_to(c + v2(t.sin() * 120.0, (t * 1.3).cos() * 60.0 - 60.0));
            w.step(FRAME);
        }
        // Then a hard drop.
        w.release(v2(0.0, 1500.0));
        let mut most = 0;
        for _ in 0..40 {
            w.step(FRAME);
            most = most.max(w.ents.iter().filter(|e| e.kind == Kind::Goo).count());
        }
        most
    };
    let (none, normal, lots) = (shed_with(0.0), shed_with(0.5), shed_with(1.0));
    eprintln!("PX goo shed: splatter 0 -> {none}, 0.5 -> {normal}, 1 -> {lots}");
    assert_eq!(none, 0);
    assert!(lots as f32 > normal as f32 * 1.5 && lots >= 20, "{lots} vs {normal}");

    let mut sheet = Sheet::new();
    let shape = |runny: f32, hue: f32, sheet: &mut Sheet| {
        let mut cfg = Settings::default();
        cfg.look[3] = runny;
        cfg.look[0] = hue;
        let mut w = world_at(v2(240.0, 150.0));
        w.tune = cfg.tuning();
        w.palette = cfg.palette();
        run(&mut w, 240, sheet, 0);
        sheet.capture(&w);
        extent(&w)
    };
    let thick = shape(0.0, 0.6, &mut sheet);
    let normal = shape(0.5, crate::draw::BASE_HUE, &mut sheet);
    let runny = shape(1.0, 0.9, &mut sheet);
    sheet.save("custom");
    eprintln!("PX extent (w, h): thick {thick:?}, normal {normal:?}, runny {runny:?}");
    assert!(thick.1 / thick.0 > normal.1 / normal.0 && normal.1 / normal.0 > runny.1 / runny.0, "runny isn't flatter");
}

/// The user's real layout: a 1920x1080 primary and a portrait 1080x1920 screen to its
/// right, reaching 278px higher and 562px lower. Below the primary is a big gap.
#[test]
fn survives_the_real_two_monitor_desktop() {
    let mons = [(0, 0, 1920, 1080), (1920, -278, 3000, 1642)];
    let bounds = Bounds { left: 0.0, top: -278.0, right: 3000.0, bottom: 1642.0 };
    let mut w = World::new(&bounds, CELL, R0, G);
    w.seed(7);
    w.set_monitors(&mons);
    w.set_platforms(&[Platform { x0: 0.0, x1: 1920.0, y: 1032.0, id: 0 }, Platform { x0: 1920.0, x1: 3000.0, y: 1642.0, id: 0 }]);
    w.spawn(v2(2400.0, 200.0));
    let start = w.slime_count();
    let on_screen = |p: V2| mons.iter().any(|&(l, t, r, b)| p.x >= l as f32 && p.x < r as f32 && p.y >= t as f32 && p.y < b as f32);
    let mut seed = 12345u32;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    // Drawing buffers the size main.rs uses.
    let mut body = vec![0u32; 1400 * 1400];
    let mut goo = vec![0u32; 3008 * 1928];
    for round in 0..60 {
        // Grab, drag somewhere visible, fling in any direction (up to the max throw).
        let at = w.centroid();
        w.start_grab(at);
        let to = if round % 3 == 0 { v2(1900.0 + rnd() * 60.0, 900.0 + rnd() * 700.0) } else { v2(rnd() * 3000.0, -200.0 + rnd() * 1800.0) };
        let to = if on_screen(to) { to } else { v2(to.x.max(1925.0), to.y) };
        for k in 1..=10 {
            w.drag_to(at + (to - at) * (k as f32 / 10.0));
            w.step(FRAME);
        }
        let ang = rnd() * std::f32::consts::TAU;
        w.release(v2(ang.cos(), ang.sin()) * (800.0 + rnd() * 2700.0));
        for f in 0..150 {
            w.step(FRAME);
            if f % 10 == 0 && rnd() < 0.2 {
                w.jump_toward(v2(rnd() * 3000.0, rnd() * 1600.0 - 200.0));
            }
        }
        if let Some(b) = w.body_bounds() {
            draw_body(&mut body, 1400, &w, (b.0 - 1, b.1 - 1, b.2 + 1, b.3 + 1), &[]);
        }
        if let Some(b) = w.goo_bounds() {
            let (gw, _) = draw_goo(&mut goo, 3008, &w, b);
            assert!(gw <= 3008);
        }
        let off: usize = w.ents.iter().filter(|e| e.kind == Kind::Slime && !on_screen(w.to_screen(v2(e.x as f32 + 0.5, e.y as f32 + 0.5)))).count();
        let c = w.centroid();
        assert!(on_screen(c), "round {round}: centroid {c:?} off every monitor");
        assert_eq!(off, 0, "round {round}: {off} slime cells off-screen");
        assert!(w.slime_count() * 2 > start, "round {round}: lost most of the slime ({} of {start})", w.slime_count());
    }
}

/// The slime rendered by the real pixel renderer into an `s` x `s` square, on a grid of
/// `grid` cells across. Straight (not premultiplied) BGRA.
fn icon_image(s: usize, grid: usize) -> Vec<u32> {
    let cell = (s / grid).max(1) as f32;
    let sf = s as f32;
    let mut w = World::new(&Bounds { left: 0.0, top: 0.0, right: sf, bottom: sf }, cell, sf * 0.43, 0.0);
    w.seed(3);
    w.spawn(v2(sf * 0.5, sf * 0.62));
    let mut out = vec![0u32; s * s];
    if let Some(b) = w.body_bounds() {
        let eyes = Eyes::new(&w).blocks(&w);
        let mut tmp = vec![0u32; s * s];
        let (bw, bh) = draw_body(&mut tmp, s, &w, b, &eyes);
        let (ox, oy) = (b.0.max(0) as usize * cell as usize, b.1.max(0) as usize * cell as usize);
        // Centre it horizontally.
        let ox2 = (s.saturating_sub(bw)) / 2;
        let _ = ox;
        for y in 0..bh.min(s - oy) {
            for x in 0..bw.min(s - ox2) {
                let p = tmp[y * s + x];
                let a = p >> 24;
                let un = |c: u32| if a == 0 { 0 } else { (c * 255 / a).min(255) };
                out[(y + oy) * s + x + ox2] = (a << 24) | (un(p >> 16 & 255) << 16) | (un(p >> 8 & 255) << 8) | un(p & 255);
            }
        }
    }
    // Drop lone nubs where the gumdrop's rounding sticks out a single cell.
    let cs = cell as usize;
    let g = s / cs;
    let solid = |out: &Vec<u32>, cx: i32, cy: i32| {
        cx >= 0 && cy >= 0 && (cx as usize) < g && (cy as usize) < g && out[(cy as usize * cs + cs / 2) * s + cx as usize * cs + cs / 2] >> 24 > 0
    };
    let lone: Vec<(usize, usize)> = (0..g * g)
        .map(|i| (i % g, i / g))
        .filter(|&(x, y)| {
            let (x, y) = (x as i32, y as i32);
            solid(&out, x, y) && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().filter(|d| solid(&out, x + d.0, y + d.1)).count() < 2
        })
        .collect();
    for (x, y) in lone {
        for yy in y * cs..(y + 1) * cs {
            out[yy * s + x * cs..yy * s + (x + 1) * cs].fill(0);
        }
    }
    out
}

/// Previews icon candidates in target/snapshots/icons.bmp.
#[test]
#[ignore]
fn icon_candidates() {
    let mut sheet = Sheet::new();
    for grid in [16usize] {
        let mut frame = vec![0u32; W * H];
        let mut x0 = 4;
        for s in [16usize, 32, 48, 256] {
            let img = icon_image(s, grid.min(s));
            let zoom = if s <= 48 { 4 } else { 1 };
            for y in 0..s * zoom {
                for x in 0..s * zoom {
                    if x0 + x < W && 4 + y < H {
                        let p = img[(y / zoom) * s + x / zoom];
                        let a = p >> 24;
                        let pm = |c: u32| c * a / 255;
                        frame[(4 + y) * W + x0 + x] = (a << 24) | (pm(p >> 16 & 255) << 16) | (pm(p >> 8 & 255) << 8) | pm(p & 255);
                    }
                }
            }
            x0 += s * zoom + 6;
        }
        sheet.frames.push(frame);
    }
    sheet.save("icons");
}

/// Writes `assets/slime.ico` (the exes' icon, embedded by build.rs) from the real renderer.
/// Run with `cargo test --release write_exe_icon -- --ignored`.
#[test]
#[ignore]
fn write_exe_icon() {
    let sizes = [16usize, 24, 32, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&s| {
            let img = icon_image(s, 16);
            if s >= 256 {
                // Big entries are PNG, as Windows expects.
                return png(&img, s);
            }
            // 32-bit DIB, double height (colour + AND mask), bottom-up.
            let mask_row = s.div_ceil(32) * 4;
            let mut d = Vec::new();
            for v in [40u32, s as u32, 2 * s as u32] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            d.extend_from_slice(&1u16.to_le_bytes());
            d.extend_from_slice(&32u16.to_le_bytes());
            d.extend_from_slice(&0u32.to_le_bytes());
            d.extend_from_slice(&((s * s * 4 + mask_row * s) as u32).to_le_bytes());
            d.extend_from_slice(&[0u8; 16]);
            for y in (0..s).rev() {
                for x in 0..s {
                    d.extend_from_slice(&img[y * s + x].to_le_bytes());
                }
            }
            for y in (0..s).rev() {
                let mut row = vec![0u8; mask_row];
                for x in 0..s {
                    if img[y * s + x] >> 24 == 0 {
                        row[x / 8] |= 0x80 >> (x % 8);
                    }
                }
                d.extend_from_slice(&row);
            }
            d
        })
        .collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len();
    for (&s, d) in sizes.iter().zip(&images) {
        let b = if s >= 256 { 0 } else { s as u8 };
        ico.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(d.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += d.len();
    }
    for d in &images {
        ico.extend_from_slice(d);
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("slime.ico"), ico).unwrap();
}

/// A minimal PNG (RGBA, stored/uncompressed deflate) of straight-alpha BGRA pixels.
fn png(img: &[u32], s: usize) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity(s * (s * 4 + 1));
    for y in 0..s {
        raw.push(0); // no filter
        for x in 0..s {
            let p = img[y * s + x];
            raw.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(s as u32).to_be_bytes());
    ihdr.extend_from_slice(&(s as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Goo comes off in pieces that match the slime and the event: a gentle hop leaves
/// little, a slam throws big blobs (bigger on a bigger slime), and a fast slide lays a
/// thick smear.
#[test]
fn goo_scales_with_size_and_violence() {
    // Size of the largest clump of goo, and the tallest column of stuck goo.
    let measure = |w: &World| {
        let goo: HashSet<(i32, i32)> = w.ents.iter().filter(|e| e.kind == Kind::Goo).map(|e| (e.x, e.y)).collect();
        let mut seen = HashSet::new();
        let mut biggest = 0;
        for &p in &goo {
            if !seen.insert(p) {
                continue;
            }
            let (mut stack, mut n) = (vec![p], 0);
            while let Some((x, y)) = stack.pop() {
                n += 1;
                for q in [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)] {
                    if goo.contains(&q) && seen.insert(q) {
                        stack.push(q);
                    }
                }
            }
            biggest = biggest.max(n);
        }
        let mut cols: std::collections::HashMap<i32, i32> = Default::default();
        for &(x, _) in &goo {
            *cols.entry(x).or_default() += 1;
        }
        (goo.len(), biggest, cols.values().copied().max().unwrap_or(0))
    };
    let mut sheet = Sheet::new();
    // Drop from the top at `vy` px/s; returns (goo cells, biggest clump) soon after landing.
    let slam = |scale: f32, vy: f32, sheet: &mut Sheet| {
        let mut w = world_at(v2(500.0, 120.0));
        w.set_scale(scale);
        run(&mut w, 30, &mut Sheet::new(), 0);
        // A gentle hop starts just above the floor; a slam from up high.
        w.spawn(v2(500.0, if vy < 1000.0 { 330.0 } else { 90.0 }));
        let c = w.centroid();
        w.start_grab(c);
        w.release(v2(0.0, vy));
        let mut best = (0, 0, 0);
        for f in 0..40 {
            w.step(FRAME);
            let m = measure(&w);
            if m.0 > best.0 {
                best = m;
            }
            if vy > 1000.0 && (f == 4 || f == 9 || f == 16) {
                sheet.capture(&w);
            }
        }
        best
    };
    let hop = slam(1.0, 300.0, &mut sheet);
    let hard = slam(1.0, 3000.0, &mut sheet);
    let big = slam(2.0, 3000.0, &mut sheet);
    // Slide along the floor at `vx` px/s; returns the thickest bit of trail.
    let slide = |scale: f32, vx: f32, sheet: &mut Sheet| {
        let mut w = world_at(v2(80.0, 300.0));
        w.set_scale(scale);
        run(&mut w, 90, &mut Sheet::new(), 0);
        let c = w.centroid();
        w.start_grab(c);
        w.release(v2(vx, 0.0));
        for _ in 0..30 {
            w.step(FRAME);
        }
        sheet.capture(&w);
        measure(&w)
    };
    let slow = slide(1.0, 250.0, &mut sheet);
    let fast = slide(1.0, 1800.0, &mut sheet);
    let fast_big = slide(2.0, 1800.0, &mut sheet);
    sheet.save("goo_violence");
    eprintln!("PX slam (cells, biggest clump, tallest column): hop {hop:?}, hard {hard:?}, big {big:?}");
    eprintln!("PX slide: slow {slow:?}, fast {fast:?}, fast big {fast_big:?}");
    assert!(hop.0 < hard.0 / 4, "a gentle hop shouldn't splatter much");
    assert!(hard.1 >= 4, "a slam should throw clumps, not single pixels");
    assert!(big.1 > hard.1, "a bigger slime should throw bigger clumps");
    assert!(fast.2 > slow.2 && fast.2 >= 2, "a fast slide should leave a thick trail");
    assert!(fast_big.2 > fast.2, "a bigger slime should leave a thicker trail");
}

/// Our own windows stay out of screenshots: a bright window put up for the test shows in
/// a plain capture but not when it's listed as ours. (Briefly shows a small magenta box.)
/// A real (but invisible) borderless window covering the main monitor counts as
/// fullscreen, and the slime's windows tuck in behind it and come back on top.
/// Run with `cargo test --release -- --ignored`.
#[test]
#[ignore]
fn hides_behind_a_real_fullscreen_window() {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let class = crate::wide("SlimeFullscreenTest");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: GetModuleHandleW(std::ptr::null()),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let before = crate::desktop::scan(std::ptr::null_mut());
        let m = before.monitors.iter().position(|m| m.left <= 0 && m.right > 0 && m.top <= 0 && m.bottom > 0).unwrap();
        let r = before.monitors[m];
        // Fully transparent, so nothing shows and clicks go through.
        let make = |ex: u32, rect: (i32, i32, i32, i32)| {
            let h = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE | ex,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                rect.0,
                rect.1,
                rect.2,
                rect.3,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                wc.hInstance,
                std::ptr::null(),
            );
            SetLayeredWindowAttributes(h, 0, 0, LWA_ALPHA);
            ShowWindow(h, SW_SHOWNOACTIVATE);
            h
        };
        let fs = make(0, (r.left, r.top, r.right - r.left, r.bottom - r.top));
        let slime = make(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, (r.left + 10, r.top + 10, 50, 50));
        let topmost = |h| GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0;

        let scan = crate::desktop::scan(std::ptr::null_mut());
        let found = scan.fullscreen.contains(&(fs as isize, m));
        crate::tuck_behind(&[slime], Some(fs as isize));
        let tucked = !topmost(slime) && GetWindow(fs, GW_HWNDNEXT) == slime;
        crate::tuck_behind(&[slime], None);
        let back = topmost(slime);
        DestroyWindow(slime);
        DestroyWindow(fs);
        eprintln!("PX fullscreen found {found}, tucked behind {tucked}, back on top {back}");
        assert!(found, "didn't spot it: {:?}", scan.fullscreen);
        assert!(tucked && back);
    }
}

/// Run with `cargo test --release -- --ignored`.
#[test]
#[ignore]
fn screenshots_leave_out_our_windows() {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::CreateSolidBrush;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let class = crate::wide("SlimeShotTest");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: GetModuleHandleW(std::ptr::null()),
            lpszClassName: class.as_ptr(),
            hbrBackground: CreateSolidBrush(0x00FF00FF),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let rect = RECT { left: 40, top: 40, right: 200, bottom: 140 };
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            wc.hInstance,
            std::ptr::null(),
        );
        // A solid magenta box (a colour nothing else on screen is likely to be).
        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        let mut msg: MSG = std::mem::zeroed();
        for _ in 0..20 {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let target = crate::shot::Target { rect, label: "test".into() };
        let magenta = |s: &crate::shot::Shot| {
            let img = &s.thumb;
            img.iter().filter(|&&p| p & 0xFFFFFF == 0xFF00FF).count() as f32 / img.len() as f32
        };
        let seen = crate::shot::capture(&target, &[], (160, 100)).unwrap();
        let hidden = crate::shot::capture(&target, &[hwnd], (160, 100)).unwrap();
        DestroyWindow(hwnd);
        eprintln!("magenta: plain capture {:.2}, with ours excluded {:.2}", magenta(&seen), magenta(&hidden));
        assert!(magenta(&seen) > 0.9, "the test window should be in a plain capture");
        assert!(magenta(&hidden) < 0.1, "our window leaked into the screenshot");
    }
}

/// A picture really reaches Claude through a Claude Code session: the panel, rendered,
/// encoded as PNG and sent as an image, is read back. (Uses the signed-in Claude plan.)
#[test]
#[ignore]
fn claude_code_reads_an_image() {
    use crate::ui::{draw_panel, panel_size, PanelView};
    let p = 2;
    let (pw, ph) = panel_size(p);
    let mut buf = vec![0u32; pw * ph];
    let v = PanelView {
        tab: 0,
        chat: &[],
        thinking: false,
        input: "",
        cursor_on: false,
        brain_line: "",
        shot: None,
        size: 0.25,
        size_normal: 0.25,
        size_pct: 100,
        jump: true,
        facts: true,
        idle: true,
        web: true,
        keepy: false,
        keepy_best: 0,
        provider: "",
        model: "",
        key: "",
        key_buttons: false,
        key_clearable: false,
        status: "",
        look_labels: crate::settings::LOOK.map(|l| l.1),
        look: crate::settings::LOOK_DEFAULT,
        look_normal: crate::settings::LOOK_DEFAULT,
        swatches: crate::settings::swatch_colours(),
        swatch_on: None,
        rainbow: false,
        nootropic: false,
        collapsed: [false; crate::ui::SECTION_N],
        hover: None,
    };
    draw_panel(&mut buf, pw, &v, p);
    // Flatten onto white (the panel has transparent corners).
    let opaque: Vec<u32> = buf
        .iter()
        .map(|&px| {
            let a = px >> 24;
            let mix = |c: u32| c + (255 - a);
            0xFF00_0000 | mix(px >> 16 & 255) << 16 | mix(px >> 8 & 255) << 8 | mix(px & 255)
        })
        .collect();
    let (media_type, bytes) = crate::shot::encode(&opaque, pw, ph).unwrap();
    let image = crate::brain::Image { media_type, base64: crate::shot::base64(&bytes) };
    let answer = crate::brain::ask_cli_once("What word is written in the dark green bar at the very top of this image? One word.", &image);
    eprintln!("Claude read: {answer:?}");
    assert!(answer.unwrap().to_uppercase().contains("SLIME"));
}

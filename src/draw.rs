//! Pixel-art rendering of the cell world: every cell is a solid square of screen pixels.
//! Shading comes from each cell's neighbourhood (outline, lit top rim, depth below the
//! surface) plus a per-cell tint that travels with the cell, so the flow is visible.

use super::sim::{Kind, World};
use crate::physics::V2;

/// An eye: a block of black cells (cell coords of its centre, size in cells), drawn
/// open, shut or otherwise for his mood, maybe with a cross brow.
pub struct EyeBlock {
    pub center: V2,
    pub w: i32,
    pub h: i32,
    pub look: Look,
    /// A slanted brow (cross): 1 slopes down to the right, -1 down to the left, 0 none.
    pub brow: i32,
    /// Size of one pattern pixel, in cells (bigger slime, chunkier details).
    pub s: i32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Look {
    Open,
    /// Closed: a line (asleep).
    Shut,
    /// Half closed (relaxed).
    Lidded,
    /// A spiral, turned this many quarter turns (dizzy).
    Spiral(u8),
}

const SPIRAL: [&[u8; 7]; 7] = [b"#######", b"......#", b".####.#", b".#..#.#", b".#.##.#", b".#....#", b".######"];

impl EyeBlock {
    /// Whether cell (x, y) is dark: part of the eye or its brow.
    pub fn dark(&self, x: i32, y: i32) -> bool {
        let ex0 = (self.center.x - self.w as f32 * 0.5).round() as i32;
        let ey0 = (self.center.y - self.h as f32 * 0.5).round() as i32;
        let s = self.s.max(1);
        let eye = match self.look {
            Look::Open => x >= ex0 && x < ex0 + self.w && y >= ey0 && y < ey0 + self.h,
            Look::Lidded => x >= ex0 && x < ex0 + self.w && y >= ey0 + self.h / 2 && y < ey0 + self.h,
            Look::Shut => {
                let row = ey0 + self.h * 2 / 3;
                x >= ex0 - s && x < ex0 + self.w + s && y >= row && y < row + s
            }
            Look::Spiral(turn) => {
                let (cx, cy) = (self.center.x.round() as i32, self.center.y.round() as i32);
                let (dx, dy) = (x - cx + 7 * s / 2, y - cy + 7 * s / 2);
                let (u, v) = (dx.div_euclid(s), dy.div_euclid(s));
                if dx < 0 || dy < 0 || u > 6 || v > 6 {
                    false
                } else {
                    let (u, v) = match turn % 4 {
                        0 => (u, v),
                        1 => (v, 6 - u),
                        2 => (6 - u, 6 - v),
                        _ => (6 - v, u),
                    };
                    SPIRAL[v as usize][u as usize] == b'#'
                }
            }
        };
        if eye {
            return true;
        }
        if self.brow != 0 {
            // From well out past the outer corner, high above the eye, sloping steeply
            // down to just over its inner corner: a proper scowl.
            let len = self.w + 4 * s;
            let top = ey0 - 5 * s;
            let i = if self.brow > 0 { x - (ex0 - 3 * s) } else { (ex0 + self.w - 1 + 3 * s) - x };
            if (0..len).contains(&i) {
                let yb = top + i * 3 * s / (len - 1).max(1);
                return y >= yb && y < yb + s + 1;
            }
        }
        false
    }
}

type Rgba = (f32, f32, f32, f32);

fn lerp(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t)
}

fn pack((r, g, b, a): Rgba) -> u32 {
    let a = a.clamp(0.0, 1.0);
    let a8 = (a * 255.0 + 0.5) as u32;
    let c = |v: f32| (((v / 255.0).clamp(0.0, 1.0) * a * 255.0 + 0.5) as u32).min(a8);
    (a8 << 24) | (c(r) << 16) | (c(g) << 8) | c(b)
}

type Rgb = (f32, f32, f32);

/// The slime's colours. `Default` is the classic green; `tinted` recolours it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub outline: Rgb,
    pub rim: Rgb,
    pub glint: Rgb,
    pub shallow: Rgb,
    pub deep: Rgb,
    pub bubble: Rgb,
    pub ring: Rgb,
    pub goo: Rgb,
    /// Opacity of the inside of the body (the outline and rim stay solid).
    pub alpha: f32,
}

impl Default for Palette {
    fn default() -> Palette {
        Palette {
            outline: (22.0, 96.0, 34.0),
            rim: (150.0, 240.0, 115.0),
            glint: (235.0, 255.0, 215.0),
            shallow: (92.0, 214.0, 78.0),
            deep: (34.0, 142.0, 52.0),
            bubble: (120.0, 225.0, 105.0),
            ring: (185.0, 250.0, 165.0),
            goo: (70.0, 190.0, 72.0),
            alpha: 0.94,
        }
    }
}

/// Hue of the classic green, as a fraction of the colour wheel.
pub const BASE_HUE: f32 = 0.31;

fn to_hsl((r, g, b): Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max - min < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl((h, s, l): (f32, f32, f32)) -> Rgb {
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        v * 255.0
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// RGB (0..255) of hue `h` (0..1 around the wheel) at saturation `s` and lightness `l`.
pub fn hue_rgb(h: f32, s: f32, l: f32) -> Rgb {
    from_hsl((h, s, l))
}

impl Palette {
    /// The classic palette turned to `hue` (0..1 around the wheel), with saturation
    /// scaled by `vivid` (0 = grey) and the inside's opacity set to `alpha`.
    pub fn tinted(hue: f32, vivid: f32, alpha: f32) -> Palette {
        let base = Palette::default();
        let shift = |c: Rgb| {
            let (h, s, l) = to_hsl(c);
            from_hsl((h + hue - BASE_HUE, (s * vivid).clamp(0.0, 1.0), l))
        };
        Palette {
            outline: shift(base.outline),
            rim: shift(base.rim),
            glint: shift(base.glint),
            shallow: shift(base.shallow),
            deep: shift(base.deep),
            bubble: shift(base.bubble),
            ring: shift(base.ring),
            goo: shift(base.goo),
            alpha: alpha.clamp(0.05, 1.0),
        }
    }
}

/// Per-cell colour variation `n`, spread over the channels in proportion to the colour,
/// so it reads as lighter/darker goo whatever the hue.
fn vary((r, g, b): Rgb, n: f32) -> Rgb {
    let m = r.max(g).max(b).max(1.0);
    (r + n * r / m, g + n * g / m, b + n * b / m)
}

fn body_like(k: Option<Kind>) -> bool {
    matches!(k, Some(Kind::Slime | Kind::Bubble))
}

fn slime_colour(world: &World, x: i32, y: i32, tint: u8, cx: f32, half_w: f32) -> Rgba {
    let pal = &world.palette;
    let up = body_like(world.at(x, y - 1));
    let edge = !up || !body_like(world.at(x, y + 1)) || !body_like(world.at(x - 1, y)) || !body_like(world.at(x + 1, y));
    if !up && !body_like(world.at(x - 1, y - 1)) && body_like(world.at(x + 1, y)) && !body_like(world.at(x - 1, y)) {
        return (pal.glint.0, pal.glint.1, pal.glint.2, 1.0);
    }
    if edge && up {
        return (pal.outline.0, pal.outline.1, pal.outline.2, 1.0);
    }
    if !up {
        // A loose corner cell lying on the floor is part of the outline, not the lit top.
        if !body_like(world.at(x, y + 1)) || !body_like(world.at(x - 1, y)) && !body_like(world.at(x + 1, y)) {
            return (pal.outline.0, pal.outline.1, pal.outline.2, 1.0);
        }
        return (pal.rim.0, pal.rim.1, pal.rim.2, 1.0);
    }
    let mut depth = 0;
    while depth < 14 && body_like(world.at(x, y - depth - 1)) {
        depth += 1;
    }
    let t = depth as f32 / 14.0;
    let n = (tint as f32 - 128.0) / 128.0 * 14.0;
    let (r, g, b) = lerp(pal.shallow, pal.deep, t);
    // Second-row cells catch some light too.
    let lit = if depth == 1 { 0.25 } else { 0.0 };
    let (r, g, b) = lerp(vary((r, g, b), n), pal.rim, lit);
    // Underside in shadow; the side away from the light a little darker.
    let under = if !body_like(world.at(x, y + 2)) { 0.8 } else if !body_like(world.at(x, y + 3)) { 0.9 } else { 1.0 };
    let side = 1.0 - 0.12 * ((x as f32 + 0.5 - cx) / half_w).clamp(0.0, 1.0);
    let k = under * side;
    (r * k, g * k, b * k, pal.alpha)
}

/// Fills a `size`-square block at (`px`, `py`), clipped to `w` x `h`.
fn fill(buf: &mut [u32], stride: usize, px: usize, py: usize, size: usize, w: usize, h: usize, c: u32) {
    if px >= w {
        return;
    }
    for yy in py..(py + size).min(h) {
        let row = &mut buf[yy * stride..yy * stride + w];
        for v in &mut row[px..(px + size).min(w)] {
            *v = c;
        }
    }
}

/// Renders cells `x0..=x1, y0..=y1` of the body (slime, bubbles, eyes) into `buf`.
/// Returns the pixel size written.
pub fn draw_body(
    buf: &mut [u32],
    stride: usize,
    world: &World,
    (x0, y0, x1, y1): (i32, i32, i32, i32),
    eyes: &[EyeBlock],
) -> (usize, usize) {
    let cs = world.cell as usize;
    let pal = &world.palette;
    let w = ((x1 - x0 + 1) as usize * cs).min(stride);
    let h = ((y1 - y0 + 1) as usize * cs).min(buf.len() / stride);
    for row in 0..h {
        buf[row * stride..row * stride + w].fill(0);
    }
    // Shine: a little pixel-art highlight near the top-left of the body.
    let (cx, half_w, shine) = match world.body_bounds() {
        Some((bx0, by0, bx1, by1)) => {
            let bw = bx1 - bx0 + 1;
            let s = (bw / 20).max(1);
            let hx = bx0 + (bw as f32 * 0.22) as i32;
            let top = (by0..=by1).find(|&y| body_like(world.at(hx, y))).unwrap_or(by0);
            let mut cells = Vec::new();
            //  .##     (in blocks of s cells)
            //  ##.
            for (px, py) in [(1, 0), (2, 0), (0, 1), (1, 1)] {
                for yy in 0..s {
                    for xx in 0..s {
                        cells.push((hx + px * s + xx, top + py * s + yy + 1, 1.0f32));
                    }
                }
            }
            // and a smaller sparkle further along.
            for yy in 0..s {
                for xx in 0..s {
                    cells.push((hx + 4 * s + xx, top + yy, 0.7));
                }
            }
            ((bx0 + bx1) as f32 * 0.5 + 0.5, (bw as f32 * 0.5).max(1.0), cells)
        }
        None => (0.0, 1.0, Vec::new()),
    };
    // Bubbles: each is one air cell, drawn with a lighter refraction ring around it.
    let bubbles: Vec<(i32, i32)> = world
        .ents
        .iter()
        .filter(|e| e.kind == Kind::Bubble && e.x >= x0 - 1 && e.x <= x1 + 1 && e.y >= y0 - 1 && e.y <= y1 + 1)
        .map(|e| (e.x, e.y))
        .collect();
    let pops: Vec<(i32, i32, f32)> = world.pops.iter().map(|p| (p.at.x as i32, p.at.y as i32, p.age)).collect();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let px = (x - x0) as usize * cs;
            let py = (y - y0) as usize * cs;
            if px >= w || py >= h {
                continue;
            }
            let Some(kind) = world.at(x, y) else { continue };
            let tint = world.tint_at(x, y);
            let mut c = match kind {
                Kind::Slime => slime_colour(world, x, y, tint, cx, half_w),
                Kind::Bubble => (pal.bubble.0, pal.bubble.1, pal.bubble.2, pal.alpha * 0.9),
                Kind::Goo => continue,
            };
            if kind == Kind::Slime {
                if let Some(&(bx, by)) = bubbles.iter().find(|&&(bx, by)| (bx - x).abs() <= 1 && (by - y).abs() <= 1) {
                    // Ring around the bubble; its top-left catches a glint.
                    let (target, k) = if (x - bx, y - by) == (-1, -1) {
                        (pal.glint, 0.85)
                    } else if x != bx && y != by {
                        (pal.ring, 0.3) // softer corners: rounder bubble
                    } else {
                        (pal.ring, 0.6)
                    };
                    let (r, g, b) = lerp((c.0, c.1, c.2), target, k);
                    c = (r, g, b, c.3);
                }
            }
            if kind == Kind::Slime {
                if let Some(&(_, _, k)) = shine.iter().find(|s| s.0 == x && s.1 == y) {
                    let (r, g, b) = lerp((c.0, c.1, c.2), pal.glint, k);
                    c = (r, g, b, c.3.max(0.95));
                }
            }
            // A popping bubble brightens the cells around it for a moment.
            for &(bx, by, age) in &pops {
                if (x - bx).abs() <= 1 && (y - by).abs() <= 1 && (x, y) != (bx, by) {
                    let k = (1.0 - age / 0.2).max(0.0) * 0.6;
                    let (r, g, b) = lerp((c.0, c.1, c.2), pal.glint, k);
                    c = (r, g, b, c.3);
                }
            }
            if kind == Kind::Slime && eyes.iter().any(|e| e.dark(x, y)) {
                c = (10.0, 22.0, 12.0, 1.0);
            }
            fill(buf, stride, px, py, cs, w, h, pack(c));
        }
    }
    (w, h)
}

/// Renders goo cells in `x0..=x1, y0..=y1`, fading with age.
pub fn draw_goo(buf: &mut [u32], stride: usize, world: &World, (x0, y0, x1, y1): (i32, i32, i32, i32)) -> (usize, usize) {
    let cs = world.cell as usize;
    let pal = &world.palette;
    let w = ((x1 - x0 + 1) as usize * cs).min(stride);
    let h = ((y1 - y0 + 1) as usize * cs).min(buf.len() / stride);
    for row in 0..h {
        buf[row * stride..row * stride + w].fill(0);
    }
    for e in world.ents.iter().filter(|e| e.kind == Kind::Goo) {
        if e.x < x0 || e.x > x1 || e.y < y0 || e.y > y1 {
            continue;
        }
        let fade = World::goo_fade(e);
        let fresh = (e.age / 0.05).min(1.0);
        let top = world.at(e.x, e.y - 1) != Some(Kind::Goo);
        let n = (e.tint as f32 - 128.0) / 128.0 * 12.0;
        let (r, g, b) = if top { pal.rim } else { vary(pal.goo, n) };
        let c = pack((r, g, b, 0.85 * (pal.alpha / 0.94).min(1.0) * fade * fresh));
        fill(buf, stride, (e.x - x0) as usize * cs, (e.y - y0) as usize * cs, cs, w, h, c);
    }
    (w, h)
}

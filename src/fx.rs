//! Moods and little extras: cross brows after a hard knock, spiral eyes after a shaking,
//! sleeping (with Z's floating up), and a cigarette with real drifting smoke. The eyes
//! are drawn with the body; Z's, the cigarette and smoke go in their own window, since
//! they stick out past it.

use super::draw::{EyeBlock, Look};
use super::sim::{Kind, World};
use crate::physics::{v2, V2};

/// A wall hit this hard (cells per substep beyond a gentle touch) makes him cross: only
/// a hard fling, hitting at about 2800 px/s or more (the fastest throw is 3500).
pub const ANGRY_HIT: f32 = 7.0;
/// Shaken this hard (the world's shake meter), he gets dizzy: only a vigorous shaking,
/// like four shakes a second 150 px each way. (Moderate shaking reaches about 70.)
pub const DIZZY_SHAKE: f32 = 110.0;
const ANGRY_FOR: f32 = 2.5;
const DIZZY_FOR: f32 = 4.0;
/// A Z floats up every so often while he sleeps (s), and lives this long.
const Z_EVERY: f32 = 1.4;
const Z_LIFE: f32 = 3.2;
/// Smoke: a thin trickle from the tip, a drag every few seconds (the tip glows), then a
/// puff breathed out.
const TRICKLE_EVERY: f32 = 0.09;
const SMOKE_LIFE: (f32, f32) = (2.2, 3.6);
const DRAG_EVERY: (f32, f32) = (3.5, 6.5);
const DRAG_FOR: f32 = 0.7;
const EXHALE_FOR: f32 = 0.6;

struct Puff {
    /// Screen px, px/s.
    p: V2,
    v: V2,
    age: f32,
    life: f32,
    /// Where in its curl it is, and how big it starts (cells).
    phase: f32,
    size: f32,
}

struct Zee {
    /// Screen px.
    p: V2,
    age: f32,
    phase: f32,
}

/// A pin pulled out: it springs off and tumbles away (screen px, px/s, radians).
struct LoosePin {
    p: V2,
    v: V2,
    angle: f32,
    spin: f32,
    age: f32,
}

const PIN_FALL_FOR: f32 = 1.4;

/// A drawing pin, pixelated from a photo (pin.png): domed cap at the top right, a narrow
/// neck, a flared base, and the needle at the bottom left. D outline, S shade, R red,
/// L lit, H glint, G/g needle.
#[rustfmt::skip]
const PIN: [&str; 16] = [
    "...........DLLDD..",
    "..........DLLLLLD.",
    ".........DSLHLLLD.",
    ".........DSLHLLLLD",
    ".........DSLLLLLLD",
    ".........DSSLLLLLD",
    "........DLRSSSLLD.",
    "...D..DDLRSDSSSD..",
    "..DLDDLLRSD.DDD...",
    ".DLLLLRRSD........",
    "DLLLRRRSD.........",
    "DLRRRRSSD.........",
    ".DRRRSSD..........",
    ".DRRSSSD..........",
    ".GDDSSD...........",
    "G...DD............",
];
/// Where the needle goes into him: the base's centre, just above the needle.
const PIN_AT: (f32, f32) = (2.5, 12.5);

fn pin_colour(c: u8) -> Option<[f32; 4]> {
    Some(match c {
        b'D' => [110.0, 12.0, 22.0, 1.0],
        b'S' => [168.0, 22.0, 34.0, 1.0],
        b'R' => [214.0, 38.0, 48.0, 1.0],
        b'L' => [240.0, 84.0, 92.0, 1.0],
        b'H' => [255.0, 186.0, 188.0, 1.0],
        b'G' => [210.0, 214.0, 222.0, 1.0],
        b'g' => [120.0, 126.0, 138.0, 1.0],
        _ => return None,
    })
}

struct Cigarette {
    /// Seconds left, which side it sticks out of (-1 left, 1 right).
    left: f32,
    side: f32,
    /// Counting down to the next drag; how far into the drag and the breath out.
    drag_in: f32,
    drag: f32,
    exhale: f32,
    trickle: f32,
}

pub struct Fx {
    /// Seconds left of each mood.
    pub angry: f32,
    pub dizzy: f32,
    asleep: Option<f32>,
    cig: Option<Cigarette>,
    zs: Vec<Zee>,
    smoke: Vec<Puff>,
    z_in: f32,
    clock: f32,
    rng: u32,
    /// Where he's pinned to the screen (screen px), and pins on their way down.
    pub pin: Option<V2>,
    loose: Vec<LoosePin>,
}

/// The cigarette's cells (cell coords, colour) and where its glowing tip is, for a
/// slime sitting in `world`, sticking out of his `side`.
struct CigCells {
    cells: Vec<(i32, i32, [f32; 4])>,
    tip: V2,
    lips: V2,
}

impl Fx {
    pub fn new(seed: u32) -> Fx {
        Fx { angry: 0.0, dizzy: 0.0, asleep: None, cig: None, zs: Vec::new(), smoke: Vec::new(), z_in: 0.5, clock: 0.0, rng: seed | 1, pin: None, loose: Vec::new() }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Thrown hard into a wall (`hit` from `World::take_wall_hit`).
    pub fn hit_wall(&mut self, hit: f32) {
        if hit > ANGRY_HIT {
            self.angry = ANGRY_FOR;
            self.wake();
        }
    }

    /// Being shaken (`shake` from `World::shake`).
    pub fn shaken(&mut self, shake: f32) {
        if shake > DIZZY_SHAKE {
            self.dizzy = DIZZY_FOR;
            self.wake();
        }
    }

    /// The pin comes out (grabbed again): it springs off and falls away.
    pub fn pull_pin(&mut self, at: V2, scale: f32) {
        let v = v2((self.rand() - 0.5) * 300.0, -(250.0 + self.rand() * 150.0)) * scale.max(0.5);
        let spin = (self.rand() - 0.5) * 24.0;
        self.loose.push(LoosePin { p: at, v, angle: 0.0, spin, age: 0.0 });
        self.pin = None;
    }

    pub fn fall_asleep(&mut self, seconds: f32) {
        self.asleep = Some(seconds);
        self.z_in = 0.8;
    }

    pub fn asleep(&self) -> bool {
        self.asleep.is_some()
    }

    /// Wakes him if he's asleep; true if he was.
    pub fn wake(&mut self) -> bool {
        self.asleep.take().is_some()
    }

    pub fn light_up(&mut self, seconds: f32, side: f32) {
        let drag_in = 1.0 + self.rand();
        self.cig = Some(Cigarette { left: seconds, side, drag_in, drag: 0.0, exhale: 0.0, trickle: 0.0 });
    }

    /// Drops the cigarette (picked up, thrown about): a last wisp of smoke where it was.
    pub fn stub_out(&mut self, world: &World) {
        if let Some(c) = self.cig.take() {
            if let Some(cc) = cig_cells(world, c.side) {
                let at = world.to_screen(cc.tip);
                for _ in 0..6 {
                    let v = v2((self.rand() - 0.5) * 40.0, -20.0 - self.rand() * 30.0);
                    self.puff(at, v, 1.0);
                }
            }
        }
    }

    /// Asleep or smoking: settled into something, so no other antics.
    pub fn busy(&self) -> bool {
        self.asleep.is_some() || self.cig.is_some()
    }

    /// How much lower he lies (added to the jelly's rest height).
    pub fn rest_bias(&self) -> f32 {
        if self.asleep.is_some() {
            -0.18
        } else {
            0.0
        }
    }

    /// The push to the jelly's lean this frame: he reels while dizzy.
    pub fn sway(&self, dt: f32) -> f32 {
        if self.dizzy <= 0.0 {
            return 0.0;
        }
        let fade = (self.dizzy / 1.5).min(1.0);
        (self.clock * std::f32::consts::TAU * 1.1).sin() * 6.0 * fade * dt
    }

    /// Whether he should look straight ahead rather than follow the mouse.
    pub fn eyes_still(&self) -> bool {
        self.asleep.is_some() || self.dizzy > 0.0
    }

    /// Restyles the eyes for his mood.
    pub fn style_eyes(&self, eyes: &mut [EyeBlock]) {
        for (i, e) in eyes.iter_mut().enumerate() {
            if self.dizzy > 0.0 {
                // Spinning spirals (the two turn opposite ways).
                let turn = (self.clock * 10.0) as i32;
                e.look = Look::Spiral(if i == 0 { turn } else { -turn }.rem_euclid(4) as u8);
            } else if self.asleep.is_some() {
                e.look = Look::Shut;
            } else if self.cig.is_some() && e.h > 1 {
                e.look = Look::Lidded;
            }
            if self.angry > 0.0 {
                // Brows slant down toward the middle (the first eye is the left one).
                e.brow = if i == 0 { 1 } else { -1 };
            }
        }
    }

    fn puff(&mut self, p: V2, v: V2, size: f32) {
        let life = SMOKE_LIFE.0 + self.rand() * (SMOKE_LIFE.1 - SMOKE_LIFE.0);
        let phase = self.rand() * std::f32::consts::TAU;
        self.smoke.push(Puff { p, v, age: 0.0, life, phase, size });
    }

    pub fn update(&mut self, dt: f32, world: &World) {
        self.clock += dt;
        self.angry = (self.angry - dt).max(0.0);
        self.dizzy = (self.dizzy - dt).max(0.0);
        let scale = world.scale.max(0.3);

        // Asleep: Z's from the top of his head.
        if let Some(left) = &mut self.asleep {
            *left -= dt;
            if *left <= 0.0 {
                self.asleep = None;
            }
        }
        if self.asleep.is_some() {
            self.z_in -= dt;
            if self.z_in <= 0.0 {
                self.z_in = Z_EVERY * (0.8 + self.rand() * 0.4);
                if let Some((x0, y0, x1, _)) = world.body_bounds() {
                    let head = world.to_screen(v2(x0 as f32 + (x1 - x0) as f32 * 0.68, y0 as f32));
                    let phase = self.rand() * std::f32::consts::TAU;
                    self.zs.push(Zee { p: head, age: 0.0, phase });
                }
            }
        }
        for z in &mut self.zs {
            z.age += dt;
            // Up and off to the side, swaying.
            z.p += v2(14.0 * scale + (z.age * 2.5 + z.phase).cos() * 12.0 * scale, -26.0 * scale) * dt;
        }
        self.zs.retain(|z| z.age < Z_LIFE);

        // Smoking.
        if let Some(mut c) = self.cig.take() {
            c.left -= dt;
            let cc = cig_cells(world, c.side);
            if c.left > 0.0 {
                if let Some(cc) = &cc {
                    let tip = world.to_screen(cc.tip + v2(0.5, 0.0));
                    c.trickle -= dt;
                    while c.trickle <= 0.0 {
                        c.trickle += TRICKLE_EVERY;
                        let v = v2((self.rand() - 0.5) * 8.0 * scale, -(22.0 + self.rand() * 14.0) * scale);
                        self.puff(tip, v, 1.0);
                    }
                    if c.drag > 0.0 {
                        c.drag += dt;
                        if c.drag > DRAG_FOR {
                            c.drag = 0.0;
                            c.exhale = 1e-4;
                        }
                    } else if c.exhale > 0.0 {
                        // Breathing out: a soft cloud from his lips, drifting up and away.
                        c.exhale += dt;
                        let lips = world.to_screen(cc.lips);
                        for _ in 0..2 {
                            let v = v2(c.side * (25.0 + self.rand() * 35.0) * scale, -(15.0 + self.rand() * 25.0) * scale);
                            self.puff(lips, v, 2.0);
                        }
                        if c.exhale > EXHALE_FOR {
                            c.exhale = 0.0;
                        }
                    } else {
                        c.drag_in -= dt;
                        if c.drag_in <= 0.0 {
                            c.drag_in = DRAG_EVERY.0 + self.rand() * (DRAG_EVERY.1 - DRAG_EVERY.0);
                            c.drag = 1e-4;
                        }
                    }
                }
                self.cig = Some(c);
            }
        }
        for s in &mut self.smoke {
            s.age += dt;
            // Rises, slows, and curls side to side.
            let curl = (s.age * 1.7 + s.phase).sin() * 10.0 * scale;
            s.p += (s.v + v2(curl, 0.0)) * dt;
            s.v = s.v * (1.0 - 0.6 * dt);
            s.v.y -= 6.0 * scale * dt;
        }
        self.smoke.retain(|s| s.age < s.life);
        for l in &mut self.loose {
            l.age += dt;
            l.v.y += 2400.0 * scale * dt;
            l.p += l.v * dt;
            l.angle += l.spin * dt;
        }
        self.loose.retain(|l| l.age < PIN_FALL_FOR);
    }

    /// Anything moving (a pin just sitting there isn't).
    pub fn animating(&self) -> bool {
        self.cig.is_some() || !self.zs.is_empty() || !self.smoke.is_empty() || !self.loose.is_empty()
    }

    /// Anything to draw in the effects window.
    pub fn visible(&self) -> bool {
        self.cig.is_some() || !self.zs.is_empty() || !self.smoke.is_empty() || self.pin.is_some() || !self.loose.is_empty()
    }

    /// Draws Z's, the cigarette and smoke into `buf` (`stride` px wide, `rows` tall).
    /// Returns where it goes on screen (px) and its size, or None if there's nothing.
    pub fn draw(&self, buf: &mut [u32], stride: usize, rows: usize, world: &World) -> Option<(i32, i32, usize, usize)> {
        if !self.visible() {
            return None;
        }
        let cell = world.cell;
        let cig = self.cig.as_ref().and_then(|c| cig_cells(world, c.side).map(|cc| (c, cc)));
        // Everything in cell coords.
        let mut lo = v2(f32::MAX, f32::MAX);
        let mut hi = v2(f32::MIN, f32::MIN);
        let mut grow = |p: V2, r: f32| {
            lo = v2(lo.x.min(p.x - r), lo.y.min(p.y - r));
            hi = v2(hi.x.max(p.x + r), hi.y.max(p.y + r));
        };
        if let Some((_, cc)) = &cig {
            for &(x, y, _) in &cc.cells {
                grow(v2(x as f32, y as f32), 2.0);
            }
        }
        for s in &self.smoke {
            grow(world.to_cell(s.p), 4.0);
        }
        for z in &self.zs {
            grow(world.to_cell(z.p), 8.0);
        }
        let pin_reach = (PIN.len().max(PIN[0].len()) + 2) as f32 * if world.scale > 1.4 { 2.0 } else { 1.0 };
        if let Some(p) = self.pin {
            grow(world.to_cell(p), pin_reach);
        }
        for l in &self.loose {
            grow(world.to_cell(l.p), pin_reach);
        }
        if lo.x > hi.x {
            return None;
        }
        let (x0, y0) = (lo.x.floor() as i32, lo.y.floor() as i32);
        let cs = cell as usize;
        let w = (((hi.x.ceil() as i32 - x0 + 1) as usize) * cs).min(stride);
        let h = (((hi.y.ceil() as i32 - y0 + 1) as usize) * cs).min(rows);
        for row in 0..h {
            buf[row * stride..row * stride + w].fill(0);
        }
        let mut put = |x: i32, y: i32, c: [f32; 4]| {
            if x < x0 || y < y0 || c[3] <= 0.0 {
                return;
            }
            let (px, py) = ((x - x0) as usize * cs, (y - y0) as usize * cs);
            if px + cs > w || py + cs > h {
                return;
            }
            let a = c[3].clamp(0.0, 1.0);
            for yy in py..py + cs {
                for v in &mut buf[yy * stride + px..yy * stride + px + cs] {
                    *v = over(*v, c, a);
                }
            }
        };

        // Smoke: soft grey blobs, growing and thinning as they rise.
        for s in &self.smoke {
            let t = s.age / s.life;
            let size = s.size + t * 2.5;
            let alpha = 0.42 * (1.0 - t) * (t * 6.0).min(1.0);
            let c = world.to_cell(s.p);
            let r = (size * 0.5).max(0.5);
            for y in (c.y - r).floor() as i32..=(c.y + r).floor() as i32 {
                for x in (c.x - r).floor() as i32..=(c.x + r).floor() as i32 {
                    let d = v2(x as f32 + 0.5 - c.x, y as f32 + 0.5 - c.y).len() / r;
                    if d <= 1.0 {
                        put(x, y, [214.0, 216.0, 222.0, alpha * (1.0 - d * d * 0.6)]);
                    }
                }
            }
        }

        // The cigarette, with a tip that glows on a drag.
        if let Some((c, cc)) = &cig {
            let glow = if c.drag > 0.0 { (c.drag / DRAG_FOR * 3.0).min(1.0) } else { 0.0 };
            for &(x, y, col) in &cc.cells {
                put(x, y, col);
            }
            let (tx, ty) = (cc.tip.x as i32, cc.tip.y as i32);
            let flicker = 0.85 + 0.15 * (self.clock * 23.0).sin();
            let ember = [255.0, 90.0 + 140.0 * glow, 30.0 + 60.0 * glow, 1.0];
            put(tx, ty, [ember[0] * flicker, ember[1] * flicker, ember[2], 1.0]);
            if glow > 0.0 {
                for (dx, dy) in [(1, 0), (0, -1), (0, 1)] {
                    put(tx + dx, ty + dy, [255.0, 150.0, 40.0, 0.45 * glow]);
                }
            }
        }

        // Z's: small ones growing as they float up, fading out at the end.
        for z in &self.zs {
            let t = z.age / Z_LIFE;
            let alpha = (t * 8.0).min(1.0) * (1.0 - ((t - 0.7) / 0.3).max(0.0));
            let big = t > 0.4;
            let glyph: &[&str] = if big {
                &["######", "....##", "...##.", "..##..", ".##...", "######"]
            } else {
                &["#####", "...##", "..##.", ".##..", "#####"]
            };
            let k = if world.scale > 1.4 { 2 } else { 1 };
            let c = world.to_cell(z.p);
            let (gx, gy) = (c.x as i32, c.y as i32);
            for (row, line) in glyph.iter().enumerate() {
                for (col, ch) in line.bytes().enumerate() {
                    if ch == b'#' {
                        for yy in 0..k {
                            for xx in 0..k {
                                let (x, y) = (gx + (col as i32) * k + xx, gy + (row as i32) * k + yy);
                                // A dark rim so it reads on any background.
                                for (dx, dy) in [(1, 0), (0, 1), (1, 1)] {
                                    put(x + dx, y + dy, [24.0, 32.0, 48.0, alpha * 0.55]);
                                }
                                put(x, y, [235.0, 240.0, 255.0, alpha]);
                            }
                        }
                    }
                }
            }
        }
        // The drawing pin, pushed into him at the pin point (the needle's out of sight
        // inside him); pins pulled out tumble down, needle and all. `k` cells per sprite
        // pixel on a big slime.
        let k = if world.scale > 1.4 { 2 } else { 1 };
        let mut sprite = |at: V2, angle: f32, needle: bool, fade: f32| {
            let (s, c) = angle.sin_cos();
            let reach = (PIN.len().max(PIN[0].len()) as i32 + 2) * k;
            let (ax, ay) = (at.x.floor() as i32, at.y.floor() as i32);
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    // Back into sprite pixels (turned by -angle about the pin point).
                    let (fx, fy) = ((dx as f32 + 0.5) / k as f32, (dy as f32 + 0.5) / k as f32);
                    let (u, v) = (fx * c + fy * s + PIN_AT.0, -fx * s + fy * c + PIN_AT.1);
                    if u < 0.0 || v < 0.0 {
                        continue;
                    }
                    let Some(row) = PIN.get(v as usize) else { continue };
                    let Some(&ch) = row.as_bytes().get(u as usize) else { continue };
                    if !needle && (ch == b'G' || ch == b'g') {
                        continue;
                    }
                    if let Some(mut col) = pin_colour(ch) {
                        col[3] = fade;
                        put(ax + dx, ay + dy, col);
                    }
                }
            }
        };
        if let Some(p) = self.pin {
            sprite(world.to_cell(p), 0.0, false, 1.0);
        }
        for l in &self.loose {
            let fade = 1.0 - (l.age / PIN_FALL_FOR - 0.7).max(0.0) / 0.3;
            sprite(world.to_cell(l.p), l.angle, true, fade);
        }
        let o = world.to_screen(v2(x0 as f32, y0 as f32));
        Some((o.x as i32, o.y as i32, w, h))
    }
}

/// Premultiplied "over": colour `c` (0-255 rgb) at alpha `a` onto `dst`.
fn over(dst: u32, c: [f32; 4], a: f32) -> u32 {
    let da = (dst >> 24) as f32 / 255.0;
    let ch = |shift: u32, v: f32| {
        let d = ((dst >> shift) & 255) as f32;
        (v * a + d * (1.0 - a)).round().clamp(0.0, 255.0) as u32
    };
    let oa = ((a + da * (1.0 - a)) * 255.0).round() as u32;
    (oa << 24) | (ch(16, c[0]) << 16) | (ch(8, c[1]) << 8) | ch(0, c[2])
}

/// Where the cigarette goes: out of the side of his face, a little below the eyes, from
/// the edge of the body.
fn cig_cells(world: &World, side: f32) -> Option<CigCells> {
    let (x0, y0, x1, y1) = world.body_bounds()?;
    let c = world.to_cell(world.centroid());
    let r = world.radius();
    // Mouth level, but inside the body even when he's squashed low.
    let y = ((c.y + r * 0.12) as i32).clamp(y0 + 2, y1 - 1);
    let dir = if side < 0.0 { -1 } else { 1 };
    // The body's edge on that row.
    let mut edge = c.x as i32;
    let mut x = c.x as i32;
    while x >= x0 - 1 && x <= x1 + 1 {
        if world.at(x, y) == Some(Kind::Slime) || world.at(x, y) == Some(Kind::Bubble) {
            edge = x;
        }
        x += dir;
    }
    let len = ((r * 0.55).round() as i32).max(5);
    let thick = if world.scale > 1.4 { 2 } else { 1 };
    let mut cells = Vec::new();
    // Filter (tucked in his lips), paper, a little ash, then the tip.
    for i in 0..len {
        let cx = edge - dir * 2 + dir * i;
        let cy = y - (i * 2 / len); // tilted up a touch
        let col = if i < 3 {
            [214.0, 150.0, 80.0, 1.0]
        } else if i == len - 2 {
            [150.0, 146.0, 140.0, 1.0]
        } else {
            [245.0, 244.0, 238.0, 1.0]
        };
        if i < len - 1 {
            for t in 0..thick {
                cells.push((cx, cy + t, col));
            }
        }
    }
    let tip = v2((edge - dir * 2 + dir * (len - 1)) as f32, (y - ((len - 1) * 2 / len)) as f32);
    let lips = v2(edge as f32 + 0.5 + dir as f32, y as f32 + 0.5);
    Some(CigCells { cells, tip, lips })
}

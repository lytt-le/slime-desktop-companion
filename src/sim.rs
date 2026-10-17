//! Noita-style cellular simulation. The desktop is a grid of cells (a few screen pixels
//! each); the slime is made of individual cells that each carry a velocity and move one
//! cell at a time, colliding with each other, window title bars and screen edges.
//!
//! What keeps the slime a creature rather than a puddle is local rules only:
//! - cohesion: every slime cell is pulled toward the body's centre of mass (harder in the core)
//! - viscosity: a cell's velocity blends with its neighbours'
//! - surface tension: loose surface cells hop to nearby spots with more slime around them
//! - liquid flow: a falling cell that is blocked tries to slide diagonally
//!
//! Goo cells break off the surface, fall, stick and fade; bubble cells rise through the
//! slime and pop at the surface.

use crate::physics::{v2, Bounds, Platform, V2};

pub const FRAME: f32 = 1.0 / 60.0;
const SUBSTEPS: usize = 2;
const DT: f32 = FRAME / SUBSTEPS as f32;

/// Cohesion pull at the body radius, in units of gravity.
const PULL: f32 = 1.6;
/// Inner fraction of the radius that moves as a firm core.
const CORE: f32 = 0.45;
/// How strongly core cells hold still relative to the body, and general damping of
/// cells' own motion (per substep).
const CORE_HOLD: f32 = 0.05;
const LOCAL_DAMP: f32 = 0.03;
/// Fastest a cell may move relative to the body (cells per substep).
const MAX_LOCAL: f32 = 1.0;
/// Fraction of the body that must be blocked for it to count as landed / hitting a wall.
const LANDED: f32 = 0.12;
/// Whole-body sliding friction while resting (deceleration in units of gravity), and
/// how hard an overhanging body slides off the edge.
const GROUND_FRICTION: f32 = 6.0;
const TIP: f32 = 2.0;
const VISCOSITY: f32 = 0.3;
/// Chance per substep that a loose surface cell tries to round itself off.
const SURFACE_TENSION: f32 = 0.35;
/// Chance that a blocked falling cell flows diagonally.
const RUNNY: f32 = 0.6;
/// Chance a flowing cell goes against its inward lean (how much the mound spreads).
const SPILL: f32 = 0.25;
/// Chance a resting cell steps sideways (levels the top into a dome).
const LEVEL: f32 = 0.3;
/// Resting dome: height as a fraction of half-width, and how eagerly stray surface cells
/// hop back inside it (chance per substep).
const DOME_ASPECT: f32 = 0.85;
/// Surface tension (Kawasaki): energy per unit height while resting (higher = flatter,
/// runnier), and temperature (how lively the surface is).
const ST_GRAVITY: f32 = 0.35;
const ST_TEMPERATURE: f32 = 0.25;
/// Fraction of the cohesion pull that still applies while resting.
const REST_PULL: f32 = 0.3;
/// Jelly: wobble frequency (Hz) and damping ratio (lower = wobblier), and how hard
/// landings, wall hits, braking, carrying, jumps and grabs kick the shape.
const JELLY_HZ: f32 = 2.2;
const JELLY_DAMPING: f32 = 0.16;
const JELLY_LAND: f32 = 3.2;
const JELLY_WALL: f32 = 0.6;
const JELLY_BRAKE: f32 = 1.0;
const JELLY_CARRY: f32 = 4.0;
const JELLY_JUMP: f32 = 6.0;
const JELLY_GRAB: f32 = 3.0;
/// Impact speed (cells per substep) below which a touch doesn't jiggle the jelly.
const IMPACT_MIN: f32 = 0.4;
/// Fraction of the cells sticking out of the shape that flow back in per substep.
const RESHAPE_RATE: f32 = 0.15;
/// How much a deep hole is preferred over a near one (cells of distance per unit depth).
const HOLE_DEPTH_PULL: f32 = 40.0;
/// Chance per substep that a bubble swaps up through the slime above it.
const BUBBLE_RISE: f32 = 0.1;
/// While carried: how much the goo sags (fraction of gravity) and lags behind the hand.
const CARRY_SAG: f32 = 0.25;
/// Sliding friction for cells resting on something.
const FRICTION: f32 = 0.25;
/// Most cells a particle may cross per substep.
const MAX_STEP: f32 = 12.0;
/// Cells further than this many radii from the body drop off as goo.
const DETACH: f32 = 2.4;
const GOO_LIFE: (f32, f32) = (5.0, 8.0);
/// Idle antics: walking pace (px/s at the peak of each step) and steps per second.
const WALK_SPEED: f32 = 70.0;
const WALK_HZ: f32 = 1.6;
/// A little hop: crouch time (s), height and length (fractions of the radius).
const HOP_CROUCH: f32 = 0.22;
const HOP_APEX: f32 = 0.8;
const HOP_LEN: f32 = 0.8;
/// A flying body loses a little sideways speed each substep, and its middle covers only
/// part of each step (the jelly flows back inside it). Both measured.
const LEAP_DRAG: f32 = 0.999;
const LEAP_EFF: f32 = 0.85;
/// How much of the shake meter is left after each substep (it settles in about a second).
const SHAKE_SETTLE: f32 = 0.99;
/// Side-to-side wobble drive (lean per second²).
const WOBBLE: f32 = 15.0;

/// How the slime feels (set from the Customisation tab). `Default` is the standard slime,
/// with the values of the constants above.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    pub viscosity: f32,
    pub runny: f32,
    pub spill: f32,
    pub dome_aspect: f32,
    pub st_gravity: f32,
    pub jelly_damping: f32,
    /// Multiplies every jelly kick (landings, grabs, jumps...).
    pub jelly_kick: f32,
    /// Multiplies how much goo is shed.
    pub shed: f32,
    /// Multiplies how long goo lasts.
    pub goo_life: f32,
    pub bubble_max: usize,
    /// Multiplies how often bubbles form.
    pub bubble_rate: f32,
    pub ground_friction: f32,
    pub friction: f32,
    /// Multiplies gravity.
    pub weight: f32,
}

impl Default for Tuning {
    fn default() -> Tuning {
        Tuning {
            viscosity: VISCOSITY,
            runny: RUNNY,
            spill: SPILL,
            dome_aspect: DOME_ASPECT,
            st_gravity: ST_GRAVITY,
            jelly_damping: JELLY_DAMPING,
            jelly_kick: 1.0,
            shed: 1.0,
            goo_life: 1.0,
            bubble_max: 6,
            bubble_rate: 1.0,
            ground_friction: GROUND_FRICTION,
            friction: FRICTION,
            weight: 1.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Slime,
    Bubble,
    Goo,
}

#[derive(Clone, Copy, Debug)]
pub struct Ent {
    pub x: i32,
    pub y: i32,
    f: V2,
    /// Velocity in cells per substep.
    v: V2,
    pub kind: Kind,
    /// Stable per-cell colour variation, so you can see the goo flow.
    pub tint: u8,
    pub age: f32,
    pub life: f32,
    /// Platform a goo cell is stuck to.
    pub stuck: Option<isize>,
}

/// The shape the body's cells settle into: a dome standing on a base row, or an oval
/// around a centre. `a`/`b` are half-width and height (dome) or radii (oval), in cells;
/// `lean` shears it sideways with height.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    cx: f32,
    cy: f32,
    a: f32,
    b: f32,
    lean: f32,
    dome: bool,
}

pub struct Grab {
    offsets: Vec<V2>,
    weights: Vec<f32>,
    /// Centre of mass relative to the grab point.
    com_offset: V2,
    pub target: V2,
    prev_target: V2,
}

/// Something he does by himself when he's sitting about.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Antic {
    /// Inches along the ground (-1 left, 1 right).
    Walk(f32),
    /// A few little hops that way (direction, hops left).
    Hop(f32, u32),
    /// Squashes himself flat, then springs back.
    Flatten,
    /// Stretches up tall.
    Stretch,
    /// Wobbles from side to side.
    Wobble,
    /// A burst of bubbles.
    Bubbles,
}

/// A bubble that just popped (cell coords, seconds since).
pub struct Pop {
    pub at: V2,
    pub age: f32,
}

pub struct World {
    pub w: i32,
    pub h: i32,
    /// Screen pixels per cell, and the screen position of cell (0, 0).
    pub cell: f32,
    pub origin: V2,
    /// Entity index + 1 at each cell, 0 if empty.
    occ: Vec<u32>,
    /// Platform index + 1 at each cell (a window's top edge), 0 if none.
    plat: Vec<u32>,
    plat_ids: Vec<isize>,
    plat_cells: Vec<usize>,
    /// Cells no monitor shows (gaps in the desktop between differently sized screens).
    void: Vec<bool>,
    monitors: Vec<(i32, i32, i32, i32)>,
    pub ents: Vec<Ent>,
    rng: u32,
    pub grab: Option<Grab>,
    /// Body radius in cells at scale 1, and the current scale.
    base_r: f32,
    pub scale: f32,
    base_count: usize,
    pub tune: Tuning,
    /// Colours (only used for drawing).
    pub palette: super::draw::Palette,
    /// Gravity in cells per substep².
    g: f32,
    launch_grace: f32,
    regrow: f32,
    /// The hardest impact this frame (cells per substep beyond a gentle touch, pointing
    /// into the surface); it throws off splatter.
    splat: V2,
    /// Last column the sliding trail was laid in, and its thickness there.
    trail_x: Option<i32>,
    trail_h: f32,
    bubble_in: f32,
    pub pops: Vec<Pop>,
    ride: HashMapF,
    order: Vec<u32>,
    vel_copy: Vec<V2>,
    /// Whole-body velocity (cells per substep) and its sub-cell remainder.
    vb: V2,
    fb: V2,
    /// Lowest row the body occupies.
    body_bottom: i32,
    /// Dome the resting body settles into: centre x, base row, half-width, height (cells).
    template: Option<Shape>,
    /// Jelly modes: squash/stretch (height factor, 1 = rest) and lean (shear), with velocities.
    squash: f32,
    squash_v: f32,
    lean: f32,
    lean_v: f32,
    /// Sitting on something (not carried).
    resting: bool,
    /// Seconds simulated (drives breathing).
    clock: f32,
    /// Smoothed acceleration of the body while carried.
    carry_accel: V2,
    /// Set by a release or jump, so that speed change isn't felt as a jolt.
    launched: bool,
    /// The idle antic in progress, seconds into it, and how long it lasts.
    antic: Option<Antic>,
    antic_t: f32,
    antic_len: f32,
    /// A hop of the antic is in the air.
    hopping: bool,
    /// The hardest sideways hit against a wall since it was last taken (cells per
    /// substep beyond a gentle touch).
    wall_hit: f32,
    /// How hard he's being shaken in the hand: jolts add up, and it settles over a second or so.
    shake: f32,
    /// Added to the jelly's rest height (negative: lies lower, as when asleep).
    pub rest_bias: f32,
}

/// Fractional window-ride offsets per platform id.
type HashMapF = std::collections::HashMap<isize, V2>;

impl World {
    /// `bounds` in screen px; `cell` px per cell; `r0` body radius in px; gravity in px/s².
    pub fn new(bounds: &Bounds, cell: f32, r0: f32, gravity: f32) -> World {
        let w = ((bounds.right - bounds.left) / cell).ceil() as i32;
        let h = ((bounds.bottom - bounds.top) / cell).ceil() as i32;
        let n = (w * h) as usize;
        World {
            w,
            h,
            cell,
            origin: v2(bounds.left, bounds.top),
            occ: vec![0; n],
            plat: vec![0; n],
            plat_ids: Vec::new(),
            plat_cells: Vec::new(),
            void: vec![false; n],
            monitors: Vec::new(),
            ents: Vec::new(),
            rng: 0x2545_f491,
            grab: None,
            base_r: r0 / cell,
            scale: 1.0,
            base_count: 0,
            tune: Tuning::default(),
            palette: Default::default(),
            g: gravity / cell * DT * DT,
            launch_grace: 0.0,
            regrow: 0.0,
            splat: V2::ZERO,
            trail_x: None,
            trail_h: 0.0,
            bubble_in: 1.0,
            pops: Vec::new(),
            ride: HashMapF::new(),
            order: Vec::new(),
            vel_copy: Vec::new(),
            vb: V2::ZERO,
            fb: V2::ZERO,
            body_bottom: 0,
            template: None,
            squash: 1.0,
            squash_v: 0.0,
            lean: 0.0,
            lean_v: 0.0,
            resting: false,
            clock: 0.0,
            carry_accel: V2::ZERO,
            launched: false,
            antic: None,
            antic_t: 0.0,
            antic_len: 0.0,
            hopping: false,
            wall_hit: 0.0,
            shake: 0.0,
            rest_bias: 0.0,
        }
    }

    pub fn seed(&mut self, s: u32) {
        self.rng = s | 1;
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn radius(&self) -> f32 {
        self.base_r * self.scale
    }

    pub fn to_cell(&self, p: V2) -> V2 {
        (p - self.origin) * (1.0 / self.cell)
    }

    pub fn to_screen(&self, c: V2) -> V2 {
        c * self.cell + self.origin
    }

    fn idx(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && x < self.w && y < self.h).then(|| (y * self.w + x) as usize)
    }

    pub fn tint_at(&self, x: i32, y: i32) -> u8 {
        self.idx(x, y).map_or(0, |i| match self.occ[i] {
            0 => 0,
            o => self.ents[o as usize - 1].tint,
        })
    }

    pub fn at(&self, x: i32, y: i32) -> Option<Kind> {
        let i = self.idx(x, y)?;
        let o = self.occ[i];
        (o != 0).then(|| self.ents[o as usize - 1].kind)
    }

    fn add(&mut self, x: i32, y: i32, kind: Kind, v: V2) -> bool {
        let Some(i) = self.idx(x, y) else { return false };
        if self.occ[i] != 0 {
            return false;
        }
        let tint = (self.rand() * 255.0) as u8;
        let life = (GOO_LIFE.0 + self.rand() * (GOO_LIFE.1 - GOO_LIFE.0)) * self.tune.goo_life;
        self.ents.push(Ent { x, y, f: V2::ZERO, v, kind, tint, age: 0.0, life, stuck: None });
        self.occ[i] = self.ents.len() as u32;
        true
    }

    fn remove(&mut self, e: usize) {
        let (x, y) = (self.ents[e].x, self.ents[e].y);
        if let Some(i) = self.idx(x, y) {
            if self.occ[i] == e as u32 + 1 {
                self.occ[i] = 0;
            }
        }
        self.ents.swap_remove(e);
        if e < self.ents.len() {
            let m = self.ents[e];
            if let Some(i) = self.idx(m.x, m.y) {
                self.occ[i] = e as u32 + 1;
            }
        }
        if let Some(g) = &mut self.grab {
            if e < g.offsets.len() {
                g.offsets.swap_remove(e);
                g.weights.swap_remove(e);
            }
        }
    }

    /// Fills a gumdrop of slime centred on `center` (screen px).
    pub fn spawn(&mut self, center: V2) {
        self.ents.retain(|e| e.kind == Kind::Goo);
        self.occ.iter_mut().for_each(|o| *o = 0);
        for (k, e) in self.ents.iter().enumerate() {
            let i = (e.y * self.w + e.x) as usize;
            self.occ[i] = k as u32 + 1;
        }
        let c = self.to_cell(center);
        let r = self.radius();
        for y in (c.y - r * 1.2) as i32..=(c.y + r * 1.2) as i32 {
            for x in (c.x - r * 1.3) as i32..=(c.x + r * 1.3) as i32 {
                let dx = (x as f32 + 0.5 - c.x) / (r * 1.1);
                let dy = (y as f32 + 0.5 - c.y) / (r * if (y as f32) < c.y { 0.95 } else { 0.6 });
                if dx * dx + dy * dy < 1.0 {
                    self.add(x, y, Kind::Slime, V2::ZERO);
                }
            }
        }
        self.base_count = (self.slime_count() as f32 / (self.scale * self.scale)) as usize;
        self.grab = None;
    }

    pub fn clear_goo(&mut self) {
        let mut i = 0;
        while i < self.ents.len() {
            if self.ents[i].kind == Kind::Goo {
                self.remove(i);
            } else {
                i += 1;
            }
        }
    }

    pub fn slime_count(&self) -> usize {
        self.ents.iter().filter(|e| e.kind != Kind::Goo).count()
    }

    /// How much of the body is left compared with a fresh one at this size (1 = all of it).
    pub fn intact(&self) -> f32 {
        self.slime_count() as f32 / (self.base_count as f32 * self.scale * self.scale).max(1.0)
    }

    /// Centre of mass and mean velocity (cells, cells/substep) of the body.
    fn body(&self) -> (V2, V2) {
        let (mut c, mut v, mut n) = (V2::ZERO, V2::ZERO, 0.0);
        for e in &self.ents {
            if e.kind != Kind::Goo {
                c += v2(e.x as f32 + 0.5, e.y as f32 + 0.5);
                v += e.v;
                n += 1.0;
            }
        }
        if n == 0.0 {
            return (V2::ZERO, V2::ZERO);
        }
        (c * (1.0 / n), v * (1.0 / n))
    }

    pub fn centroid(&self) -> V2 {
        self.to_screen(self.body().0)
    }

    /// Body velocity in px/s.
    pub fn velocity(&self) -> V2 {
        self.vb * (self.cell / DT)
    }

    /// Rasterises the walkable tops of windows and monitor floors.
    pub fn set_platforms(&mut self, plats: &[Platform]) {
        for &i in &self.plat_cells {
            self.plat[i] = 0;
        }
        self.plat_cells.clear();
        self.plat_ids.clear();
        for p in plats {
            let y = ((p.y - self.origin.y) / self.cell).floor() as i32;
            if y < 0 || y >= self.h {
                continue;
            }
            self.plat_ids.push(p.id);
            let k = self.plat_ids.len() as u32;
            let x0 = (((p.x0 - self.origin.x) / self.cell).floor() as i32).max(0);
            let x1 = (((p.x1 - self.origin.x) / self.cell).ceil() as i32).min(self.w);
            for x in x0..x1 {
                let i = (y * self.w + x) as usize;
                if self.plat[i] == 0 {
                    self.plat[i] = k;
                    self.plat_cells.push(i);
                }
            }
        }
    }

    /// Marks every cell outside all monitors as solid. `rects` are screen px
    /// (left, top, right, bottom); only recomputed when they change.
    pub fn set_monitors(&mut self, rects: &[(i32, i32, i32, i32)]) {
        if self.monitors == rects {
            return;
        }
        self.monitors = rects.to_vec();
        for y in 0..self.h {
            for x in 0..self.w {
                let p = self.to_screen(v2(x as f32 + 0.5, y as f32 + 0.5));
                let shown = rects.iter().any(|&(l, t, r, b)| p.x >= l as f32 && p.x < r as f32 && p.y >= t as f32 && p.y < b as f32);
                self.void[(y * self.w + x) as usize] = !shown && !rects.is_empty();
            }
        }
    }

    fn plat_id_at(&self, x: i32, y: i32) -> Option<isize> {
        let i = self.idx(x, y)?;
        let k = self.plat[i];
        (k != 0).then(|| self.plat_ids[k as usize - 1])
    }

    /// Whether entity `e` may step by (dx, dy). Platforms are one-way: solid only to
    /// things arriving from above.
    fn free(&self, e: &Ent, dx: i32, dy: i32) -> bool {
        let (x, y) = (e.x + dx, e.y + dy);
        let Some(i) = self.idx(x, y) else { return false };
        if self.void[i] {
            return false;
        }
        if self.occ[i] != 0 {
            let other = self.ents[self.occ[i] as usize - 1].kind;
            // Slime pushes bubbles aside; anything else blocks.
            if !(e.kind == Kind::Slime && other == Kind::Bubble) {
                return false;
            }
        }
        self.platform_allows(e, x, y, dy)
    }

    fn step_to(&mut self, k: usize, dx: i32, dy: i32) {
        let e = self.ents[k];
        let from = (e.y * self.w + e.x) as usize;
        let to = ((e.y + dy) * self.w + e.x + dx) as usize;
        let other = self.occ[to];
        self.occ[from] = 0;
        if other != 0 {
            // Swap the bubble into our old spot.
            let o = other as usize - 1;
            self.ents[o].x = e.x;
            self.ents[o].y = e.y;
            self.occ[from] = other;
        }
        self.ents[k].x += dx;
        self.ents[k].y += dy;
        self.occ[to] = k as u32 + 1;
    }

    /// Resting on something solid directly below.
    fn supported(&self, e: &Ent) -> Option<isize> {
        let (x, y) = (e.x, e.y + 1);
        if y >= self.h || self.void[(y * self.w + x) as usize] {
            return Some(0);
        }
        if let Some(id) = self.plat_id_at(x, y) {
            if self.plat_id_at(e.x, e.y).is_none() {
                return Some(id);
            }
        }
        None
    }

    /// The platform most of the body rests on.
    pub fn support(&self) -> Option<isize> {
        let mut counts: Vec<(isize, usize)> = Vec::new();
        for e in &self.ents {
            if e.kind != Kind::Slime {
                continue;
            }
            if let Some(id) = self.supported(e) {
                match counts.iter_mut().find(|c| c.0 == id) {
                    Some(c) => c.1 += 1,
                    None => counts.push((id, 1)),
                }
            }
        }
        counts.into_iter().filter(|c| c.1 >= 3).max_by_key(|c| c.1).map(|c| c.0)
    }

    fn neighbours(&self, x: i32, y: i32) -> usize {
        let mut n = 0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if (dx, dy) != (0, 0) && matches!(self.at(x + dx, y + dy), Some(Kind::Slime | Kind::Bubble)) {
                    n += 1;
                }
            }
        }
        n
    }

    pub fn start_grab(&mut self, at: V2) {
        let p = self.to_cell(at);
        let sigma = self.radius() * 0.5;
        let mut offsets = Vec::with_capacity(self.ents.len());
        let mut weights = Vec::with_capacity(self.ents.len());
        for e in &self.ents {
            let q = v2(e.x as f32 + 0.5, e.y as f32 + 0.5);
            offsets.push(q - p);
            let w = if e.kind == Kind::Goo { 0.0 } else { 0.3 + 0.7 * (-(q - p).len2() / (2.0 * sigma * sigma)).exp() };
            weights.push(w);
        }
        self.antic = None;
        self.squash_v -= JELLY_GRAB * self.tune.jelly_kick; // a little squish when grabbed
        let com_offset = self.body().0 - p;
        self.grab = Some(Grab { offsets, weights, com_offset, target: p, prev_target: p });
    }

    /// Updates the grab target (screen px).
    pub fn drag_to(&mut self, at: V2) {
        let p = self.to_cell(at);
        if let Some(g) = &mut self.grab {
            g.target = p;
        }
    }

    pub fn release(&mut self, throw: V2) {
        if self.grab.take().is_none() {
            return;
        }
        self.vb = throw * (DT / self.cell);
        self.launched = true;
        for e in &mut self.ents {
            if e.kind != Kind::Goo {
                e.v = e.v * 0.3;
            }
        }
        self.launch_grace = 0.25;
    }

    pub fn jump_toward(&mut self, target: V2) -> bool {
        let rise = self.body().0.y - self.to_cell(target).y;
        let r = self.radius();
        let apex = (rise.max(0.0) + r * 0.6).clamp(r, 900.0 / self.cell);
        self.launch(target, apex, 1600.0, 0.0)
    }

    /// A big leap to somewhere far off (another monitor). `gate` is where the arc must
    /// pass over (x) and how high it must be there at least (y): the edge between two
    /// screens, high enough to clear the floor and any gap below. `ceiling` is the top
    /// it mustn't rise past. All in screen px.
    pub fn leap_to(&mut self, target: V2, gate: Option<V2>, ceiling: f32) -> bool {
        let (c, t) = (self.body().0, self.to_cell(target));
        let r = self.radius();
        let g = self.g * self.tune.weight;
        let highest = (c.y - self.to_cell(v2(0.0, ceiling)).y - r * 1.5).max(r);
        let mut apex = ((c.y - t.y).max(0.0) + (t.x - c.x).abs() * 0.15).clamp(r, highest);
        let mut min_vx = 0.0;
        if let Some(gate) = gate.map(|p| self.to_cell(p)) {
            // He must be past the gate before he sinks below it: high enough to get there
            // (but under the ceiling), and fast enough sideways. (Allowing for how the
            // body really flies: its middle covers only part of each step, and it loses
            // a little sideways speed.)
            let need = c.y - gate.y;
            let mut fall = None;
            while fall.is_none() {
                let u = (2.0 * g * apex).sqrt();
                let disc = u * u - 2.0 * g * need / LEAP_EFF;
                if disc >= 0.0 {
                    fall = Some((u + disc.sqrt()) / g);
                } else if apex >= highest {
                    break;
                } else {
                    apex = (apex * 1.1).min(highest);
                }
            }
            if let Some(steps) = fall {
                let reach = LEAP_EFF * (1.0 - LEAP_DRAG.powf(steps)) / (1.0 - LEAP_DRAG);
                min_vx = (gate.x - c.x).abs() / reach * 1.1;
            }
        }
        self.launch(target, apex, 4000.0, min_vx)
    }

    /// Jumps toward `target` (screen px), rising `apex` cells, at most `max_vx` px/s
    /// sideways but at least `min_vx` cells per substep.
    fn launch(&mut self, target: V2, apex: f32, max_vx: f32, min_vx: f32) -> bool {
        if self.grab.is_some() || self.support().is_none() {
            return false;
        }
        self.antic = None;
        let (c, _) = self.body();
        let t = self.to_cell(target);
        let r = self.radius();
        let g = self.g * self.tune.weight;
        let rise = c.y - t.y;
        let vy = -(2.0 * g * apex).sqrt();
        let disc = (vy * vy - 2.0 * g * rise).max(0.0);
        let steps = (-vy + disc.sqrt()) / g;
        let max_vx = max_vx * DT / self.cell;
        let vx = (t.x - c.x) / steps.max(1.0);
        let vx = (vx.signum() * vx.abs().max(min_vx)).clamp(-max_vx, max_vx);
        self.vb = v2(vx, vy);
        self.launched = true;
        self.squash_v += JELLY_JUMP * self.tune.jelly_kick; // stretches on take-off
        for e in &mut self.ents {
            if e.kind != Kind::Goo {
                // The top leaves first: a little stretch on take-off.
                e.v = v2(0.0, vy * 0.2 * ((c.y - e.y as f32) / r).clamp(-1.0, 1.0));
            }
        }
        self.launch_grace = 0.25;
        true
    }

    pub fn set_scale(&mut self, s: f32) {
        self.scale = s;
    }

    /// Moves the whole body by `d` screen px (riding a window), keeping the remainder.
    pub fn ride(&mut self, id: isize, d: V2) {
        // Vertically, snap the base exactly onto the window's (already redrawn) edge row:
        // rounding drift would otherwise leave it a row inside the edge, which counts as
        // straddling it and drops the slime through.
        // (Judged by the cells resting over the window itself, so goo draping over its
        // edge doesn't count.)
        let snap = self.plat_ids.iter().position(|&p| p == id).and_then(|k| {
            let mark = k as u32 + 1;
            let cells: Vec<usize> = self.plat_cells.iter().copied().filter(|&i| self.plat[i] == mark).collect();
            let row = *cells.first()? as i32 / self.w;
            let (lo, hi) = cells.iter().fold((i32::MAX, i32::MIN), |(a, b), &i| {
                let x = i as i32 % self.w;
                (a.min(x), b.max(x))
            });
            let base = self
                .ents
                .iter()
                .filter(|e| e.kind != Kind::Goo && e.x >= lo && e.x <= hi && (e.y - (row - 1)).abs() <= 4)
                .map(|e| e.y)
                .max()?;
            Some(row - 1 - base)
        });
        let acc = self.ride.entry(id).or_insert(V2::ZERO);
        *acc += d * (1.0 / self.cell);
        let (dx, mut dy) = (acc.x.trunc() as i32, acc.y.trunc() as i32);
        acc.x -= dx as f32;
        acc.y -= dy as f32;
        if let Some(s) = snap {
            dy = s;
            acc.y = 0.0;
        }
        if (dx, dy) == (0, 0) {
            return;
        }
        // Body plus goo stuck to that window.
        let moving: Vec<usize> = (0..self.ents.len())
            .filter(|&k| self.ents[k].kind != Kind::Goo || self.ents[k].stuck == Some(id))
            .collect();
        for &k in &moving {
            let e = self.ents[k];
            if let Some(i) = self.idx(e.x, e.y) {
                self.occ[i] = 0;
            }
        }
        for &k in &moving {
            let (x, y) = (self.ents[k].x + dx, self.ents[k].y + dy);
            self.ents[k].x = x.clamp(0, self.w - 1);
            self.ents[k].y = y.clamp(0, self.h - 1);
        }
        for &k in &moving {
            let e = self.ents[k];
            let i = (e.y * self.w + e.x) as usize;
            if self.occ[i] == 0 {
                self.occ[i] = k as u32 + 1;
            }
        }
        // Anything that landed on an occupied cell is dropped.
        let mut k = 0;
        while k < self.ents.len() {
            let e = self.ents[k];
            if self.occ[(e.y * self.w + e.x) as usize] != k as u32 + 1 {
                self.ents.swap_remove(k);
                self.reindex();
            } else {
                k += 1;
            }
        }
    }

    /// Moves goo stuck to a window that moved (when the slime isn't riding it).
    pub fn ride_goo(&mut self, id: isize, d: V2) {
        let dx = (d.x / self.cell).round() as i32;
        let dy = (d.y / self.cell).round() as i32;
        if (dx, dy) == (0, 0) {
            return;
        }
        for k in 0..self.ents.len() {
            let e = self.ents[k];
            if e.kind == Kind::Goo && e.stuck == Some(id) {
                if let Some(i) = self.idx(e.x, e.y) {
                    if self.occ[i] == k as u32 + 1 {
                        self.occ[i] = 0;
                    }
                }
                let (x, y) = (e.x + dx, e.y + dy);
                match self.idx(x, y) {
                    Some(i) if self.occ[i] == 0 => {
                        self.ents[k].x = x;
                        self.ents[k].y = y;
                        self.occ[i] = k as u32 + 1;
                    }
                    _ => self.ents[k].age = self.ents[k].life, // nowhere to go: let it expire
                }
            }
        }
    }

    fn reindex(&mut self) {
        self.occ.iter_mut().for_each(|o| *o = 0);
        for (k, e) in self.ents.iter().enumerate() {
            if let Some(i) = (e.x >= 0 && e.y >= 0 && e.x < self.w && e.y < self.h).then(|| (e.y * self.w + e.x) as usize) {
                self.occ[i] = k as u32 + 1;
            }
        }
    }

    pub fn step(&mut self, dt: f32) {
        self.launch_grace = (self.launch_grace - dt).max(0.0);
        for p in &mut self.pops {
            p.age += dt;
        }
        self.pops.retain(|p| p.age < 0.2);
        for s in 0..SUBSTEPS {
            self.substep((s + 1) as f32 / SUBSTEPS as f32);
        }
        if let Some(g) = &mut self.grab {
            g.prev_target = g.target;
        }
        self.life_cycle(dt);
    }

    fn substep(&mut self, grab_t: f32) {
        let (c, _) = self.body();
        let r = self.radius().max(1.0);
        let g = self.g * self.tune.weight;
        let grab_target = self.grab.as_ref().map(|gr| gr.prev_target + (gr.target - gr.prev_target) * grab_t);

        let rest_mod = self.drive_antic(c, r, g);

        // --- Whole-body motion: every body cell steps together, one cell at a time. ---
        let vb0 = self.vb;
        let launched = std::mem::take(&mut self.launched);
        if let (Some(t), Some(gr)) = (grab_target, &self.grab) {
            let want = (t + gr.com_offset - c) * 0.25;
            self.vb = self.vb + (want - self.vb) * 0.5;
        } else {
            self.vb.y += g;
        }
        let l = self.vb.len();
        if l > MAX_STEP {
            self.vb = self.vb * (MAX_STEP / l);
        }
        self.fb += self.vb;
        let mut grounded = false;
        let mut impact = V2::ZERO;
        while self.fb.y.abs() >= 1.0 {
            let sy = self.fb.y.signum() as i32;
            if self.move_group(0, sy) > LANDED {
                impact.y = self.vb.y;
                self.vb.y = 0.0;
                self.fb.y = 0.0;
                grounded |= sy > 0;
                break;
            }
            self.fb.y -= sy as f32;
        }
        while self.fb.x.abs() >= 1.0 {
            let sx = self.fb.x.signum() as i32;
            if self.move_group(sx, 0) > LANDED {
                impact.x = self.vb.x;
                self.vb.x = 0.0;
                self.fb.x = 0.0;
                break;
            }
            self.fb.x -= sx as f32;
        }

        // Resting on something: friction, and tipping off edges.
        let mut span: Option<(i32, i32)> = None;
        for e in &self.ents {
            if e.kind == Kind::Slime && self.supported(e).is_some() {
                span = Some(span.map_or((e.x, e.x), |(a, b)| (a.min(e.x), b.max(e.x))));
            }
        }
        let vx_before = self.vb.x;
        if let Some((left, right)) = span {
            grounded = true;
            // (Clipping a ledge on the way past isn't resting on it: only grip when not
            // falling or rising.)
            if self.grab.is_none() && self.vb.y.abs() < 0.5 {
                let slow = self.tune.ground_friction * g;
                self.vb.x = self.vb.x.signum() * (self.vb.x.abs() - slow).max(0.0);
                // Centre of mass hanging past the edge of the support: slide off that way.
                let over = if c.x > right as f32 + 1.0 {
                    c.x - right as f32 - 1.0
                } else if c.x < left as f32 {
                    c.x - left as f32
                } else {
                    0.0
                };
                self.vb.x += TIP * g * over / r;
            }
        }
        let braking = self.vb.x - vx_before;
        // Walking into a wall: turn round.
        if let Some(Antic::Walk(d)) = self.antic {
            if impact.x != 0.0 {
                self.antic = Some(Antic::Walk(-d));
            }
        }

        // --- Jelly: the body's shape is a pair of springy modes, squash/stretch and lean,
        // kicked by whatever just happened to it. They overshoot and settle, so it
        // wobbles like jelly instead of slumping like sand.
        // Jolts in the hand (sharp changes of direction) build up the shake.
        self.shake *= SHAKE_SETTLE;
        if self.grab.is_some() {
            self.shake += (self.vb - vb0).len();
            self.carry_accel = self.carry_accel + ((self.vb - vb0) - self.carry_accel) * 0.1;
            // Lags behind the hand and dangles.
            self.lean_v -= JELLY_CARRY * self.tune.jelly_kick * self.carry_accel.x;
            self.squash_v -= JELLY_CARRY * self.tune.jelly_kick * self.carry_accel.y;
        } else {
            self.carry_accel = V2::ZERO;
        }
        // (Only real impacts: resting contact re-touches the ground every few substeps.)
        let hard = |v: f32| v.signum() * (v.abs() - IMPACT_MIN).max(0.0);
        if impact.y > 0.0 {
            self.squash_v -= JELLY_LAND * self.tune.jelly_kick * hard(impact.y); // landing: splat
        }
        // Hard impacts throw off splatter (in life_cycle).
        let hit = v2(hard(impact.x), hard(impact.y));
        if hit.len() > self.splat.len() {
            self.splat = hit;
        }
        self.lean_v += JELLY_WALL * self.tune.jelly_kick * hard(impact.x); // hitting a wall: the top keeps going
        if self.grab.is_none() {
            self.wall_hit = self.wall_hit.max(hard(impact.x).abs());
        }
        self.lean_v -= JELLY_BRAKE * self.tune.jelly_kick * braking; // stopping a slide: rocks forward
        if launched {
            self.squash_v = self.squash_v.max(0.0);
        }
        // Resting, it breathes: a slow, slight swell and settle.
        self.clock += DT;
        let breath = 0.035 * (self.clock * std::f32::consts::TAU / 3.2).sin();
        let rest_s = if self.grab.is_some() { 1.15 } else { 1.0 + breath + rest_mod + self.rest_bias };
        let k = (std::f32::consts::TAU * JELLY_HZ).powi(2);
        let damp = 2.0 * self.tune.jelly_damping * std::f32::consts::TAU * JELLY_HZ;
        for (x, xv, rest, lo, hi) in [
            (&mut self.squash, &mut self.squash_v, rest_s, 0.5, 1.8),
            (&mut self.lean, &mut self.lean_v, 0.0, -0.7, 0.7),
        ] {
            *xv = (*xv + (-k * (*x - rest) - damp * *xv) * DT).clamp(-12.0, 12.0);
            *x += *xv * DT;
            if *x < lo || *x > hi {
                *x = x.clamp(lo, hi);
                *xv = 0.0;
            }
        }

        // Target shape the cells walk toward: a dome sitting on its base when resting,
        // an oval in the air or in the hand; squashed and leaned by the jelly modes.
        let n = self.ents.iter().filter(|e| e.kind != Kind::Goo).count() as f32;
        let (s, lean) = (self.squash, self.lean);
        self.resting = span.is_some() && self.grab.is_none();
        self.template = Some(if self.resting {
            let aspect = self.tune.dome_aspect;
            let a = (2.0 * n / (std::f32::consts::PI * aspect)).sqrt();
            Shape { cx: c.x, cy: (self.body_bottom + 1) as f32, a: a / s, b: a * aspect * s, lean, dome: true }
        } else {
            let rr = (n / std::f32::consts::PI).sqrt();
            Shape { cx: c.x, cy: c.y, a: rr / s, b: rr * s, lean, dome: false }
        });

        // --- Local motion: each cell's own velocity on top of the body's. ---
        // Resting cells feel their weight; in the air the body is weightless inside;
        // carried, it sags a little.
        let mut inertia = if self.grab.is_some() {
            v2(0.0, g * CARRY_SAG)
        } else if grounded {
            v2(0.0, g)
        } else {
            V2::ZERO
        };
        let il = inertia.len();
        if il > MAX_LOCAL {
            inertia = inertia * (MAX_LOCAL / il);
        }
        self.vel_copy.clear();
        self.vel_copy.extend(self.ents.iter().map(|e| e.v));
        for k in 0..self.ents.len() {
            let e = self.ents[k];
            let p = v2(e.x as f32 + 0.5, e.y as f32 + 0.5);
            match e.kind {
                Kind::Goo => {
                    if e.stuck.is_none() {
                        self.ents[k].v = v2(e.v.x * 0.995, e.v.y + g);
                    }
                }
                Kind::Bubble => {
                    let (avg, _) = self.local_velocity(e.x, e.y);
                    self.ents[k].v = avg;
                }
                Kind::Slime => {
                    let d = c - p;
                    let dist = d.len();
                    let core = 1.0 - (dist / (r * CORE)).clamp(0.0, 1.0);
                    let mut v = e.v + inertia * (1.0 - 0.6 * core);
                    if dist > 1e-3 {
                        // Resting, surface tension shapes the body; the pull just keeps it together.
                        let pull = if self.template.is_some() { PULL * REST_PULL } else { PULL };
                        v += d * (pull * g * (dist / r).min(1.6) / dist);
                    }
                    let (avg, n) = self.local_velocity(e.x, e.y);
                    if n > 0 {
                        v = v + (avg - v) * self.tune.viscosity;
                    }
                    // The core holds still relative to the body; the shell flows.
                    v = v * (1.0 - LOCAL_DAMP - CORE_HOLD * core);
                    if self.supported(&e).is_some() {
                        v.x *= 1.0 - self.tune.friction;
                    }
                    let l = v.len();
                    if l > MAX_LOCAL {
                        v = v * (MAX_LOCAL / l);
                    }
                    self.ents[k].v = v;
                }
            }
        }

        // Off the ground, motion every cell shares is really motion of the whole body:
        // hand it to the body so cells only keep their own wobble. (Stepping a shared
        // motion cell by cell would block itself and tear holes.)
        if !grounded {
            let (mut m, mut n) = (V2::ZERO, 0.0);
            for e in &self.ents {
                if e.kind == Kind::Slime {
                    m += e.v;
                    n += 1.0;
                }
            }
            if n > 0.0 {
                m = m * (1.0 / n);
                self.vb += m;
                for e in &mut self.ents {
                    if e.kind == Kind::Slime {
                        e.v -= m;
                    }
                }
            }
        }

        self.update_bottom();
        // Bottom-first, so falling cells don't block each other.
        self.order.clear();
        self.order.extend(0..self.ents.len() as u32);
        let ents = &self.ents;
        // Each cell moves in the order of its own travel: fallers bottom-first, risers
        // top-first, so they don't block each other and tear gaps.
        self.order.sort_by_key(|&k| {
            let e = &ents[k as usize];
            // Flying goo goes front-first along its own path, so blobs travel whole.
            let l = e.v.len();
            if e.kind == Kind::Goo && l > 1e-3 {
                return std::cmp::Reverse(((e.x as f32 * e.v.x + e.y as f32 * e.v.y) / l).round() as i32);
            }
            std::cmp::Reverse(if e.v.y < 0.0 { -e.y } else { e.y })
        });
        let order = std::mem::take(&mut self.order);
        for &k in &order {
            let k = k as usize;
            if k >= self.ents.len() {
                continue;
            }
            let e = self.ents[k];
            match e.kind {
                Kind::Goo if e.stuck.is_some() => {}
                Kind::Bubble => self.move_bubble(k),
                _ => self.move_ent(k),
            }
        }
        self.order = order;

        self.close_holes();
        self.surface_tension();
        self.fill_template();
        self.detach_and_absorb(c, r);
    }

    /// Steps every body cell by (dx, dy), front cells first. Cells that are blocked stay
    /// put (the body squashes around them). Returns the fraction that was blocked.
    fn move_group(&mut self, dx: i32, dy: i32) -> f32 {
        self.update_bottom();
        self.order.clear();
        self.order.extend((0..self.ents.len() as u32).filter(|&k| self.ents[k as usize].kind != Kind::Goo));
        let ents = &self.ents;
        self.order.sort_by_key(|&k| std::cmp::Reverse(ents[k as usize].x * dx + ents[k as usize].y * dy));
        let order = std::mem::take(&mut self.order);
        let mut blocked = 0;
        for &k in &order {
            let k = k as usize;
            let e = self.ents[k];
            let (x, y) = (e.x + dx, e.y + dy);
            let open = match self.idx(x, y) {
                None => false,
                // The body pushes through loose goo (they swap places).
                Some(i) => {
                    !self.void[i]
                        && (self.occ[i] == 0 || self.ents[self.occ[i] as usize - 1].kind == Kind::Goo)
                        && self.platform_allows(&e, x, y, dy)
                }
            };
            if open {
                // Goo the body runs into is soaked back up rather than left inside it.
                let o = self.occ[self.idx(x, y).unwrap()];
                if o != 0 {
                    let g = &mut self.ents[o as usize - 1];
                    g.kind = Kind::Slime;
                    g.stuck = None;
                    g.v = V2::ZERO;
                }
                self.step_to(k, dx, dy);
            } else {
                blocked += 1;
            }
        }
        let n = order.len().max(1);
        self.order = order;
        blocked as f32 / n as f32
    }

    fn update_bottom(&mut self) {
        self.body_bottom = self.ents.iter().filter(|e| e.kind != Kind::Goo).map(|e| e.y).max().unwrap_or(0);
    }

    /// One-way platforms: solid only to things arriving from above or the side.
    fn platform_allows(&self, e: &Ent, x: i32, y: i32, dy: i32) -> bool {
        let i = (y * self.w + x) as usize;
        if self.plat[i] == 0 || dy < 0 || self.plat[(e.y * self.w + e.x) as usize] == self.plat[i] {
            return true;
        }
        // Like a platformer's feet: a window top only holds the body if the body is
        // entirely above it. Straddling it (e.g. after being thrown up through the
        // window), the body drops through instead of getting skewered.
        if e.kind != Kind::Goo && self.body_bottom >= y {
            return true;
        }
        let id = self.plat_ids[self.plat[i] as usize - 1];
        // While carried, only monitor floors are solid.
        self.grab.is_some() && e.kind != Kind::Goo && id != 0
    }

    fn local_velocity(&self, x: i32, y: i32) -> (V2, usize) {
        let (mut v, mut n) = (V2::ZERO, 0);
        for dy in -1..=1 {
            for dx in -1..=1 {
                if (dx, dy) == (0, 0) {
                    continue;
                }
                if let Some(i) = self.idx(x + dx, y + dy) {
                    let o = self.occ[i];
                    if o != 0 && self.ents[o as usize - 1].kind == Kind::Slime {
                        v += self.vel_copy[o as usize - 1];
                        n += 1;
                    }
                }
            }
        }
        (if n > 0 { v * (1.0 / n as f32) } else { V2::ZERO }, n)
    }

    fn move_ent(&mut self, k: usize) {
        let mut f = self.ents[k].f + self.ents[k].v;
        let mut v = self.ents[k].v;
        let mut guard = 0;
        loop {
            let sx = if f.x >= 1.0 { 1 } else if f.x <= -1.0 { -1 } else { 0 };
            let sy = if f.y >= 1.0 { 1 } else if f.y <= -1.0 { -1 } else { 0 };
            if (sx, sy) == (0, 0) || guard > 40 {
                break;
            }
            guard += 1;
            let e = self.ents[k];
            if self.free(&e, sx, sy) {
                self.step_to(k, sx, sy);
                f -= v2(sx as f32, sy as f32);
                continue;
            }
            if sx != 0 && sy != 0 {
                // Diagonal blocked: slide along whichever axis is open.
                if self.free(&e, sx, 0) {
                    self.step_to(k, sx, 0);
                    f.x -= sx as f32;
                    f.y = 0.0;
                    v.y = 0.0;
                    continue;
                }
                if self.free(&e, 0, sy) {
                    self.step_to(k, 0, sy);
                    f.y -= sy as f32;
                    f.x = 0.0;
                    v.x = 0.0;
                    continue;
                }
                f = V2::ZERO;
                v = V2::ZERO;
                break;
            }
            if sy > 0 {
                // Liquid: a blocked fall may flow off to one side.
                // (Only while moving: at rest, surface tension shapes the body; sand-style
                // flow would pile it into a cone.)
                if (e.kind == Kind::Goo || self.template.is_none()) && self.rand() < self.tune.runny {
                    // Flow the way the cell is already heading (cohesion pulls it inward),
                    // so the sides bulge instead of spilling into a cone.
                    let d = if self.rand() < 0.5 { -1 } else { 1 };
                    let lean = if v.x > 0.05 { 1 } else if v.x < -0.05 { -1 } else { d };
                    // Mostly follow the lean; sometimes spill the other way so it can spread.
                    let first = if self.rand() < self.tune.spill { -lean } else { lean };
                    let dirs: &[i32] = &[first, -first];
                    for &dx in dirs {
                        if self.free(&e, dx, 1) && self.keeps_shape(&e, dx, 1) {
                            self.step_to(k, dx, 1);
                            f.y -= 1.0;
                            v.x += dx as f32 * 0.15;
                            break;
                        }
                    }
                    if self.ents[k].y != e.y {
                        continue;
                    }
                }
                // Liquid also levels out: a resting cell may step sideways the way it leans.
                if e.kind == Kind::Slime && self.template.is_none() && self.rand() < LEVEL {
                    let lean = if v.x > 0.05 { 1 } else if v.x < -0.05 { -1 } else if self.rand() < 0.5 { -1 } else { 1 };
                    if self.free(&e, lean, 0) && self.at(e.x + lean, e.y).is_none() && self.keeps_shape(&e, lean, 0) {
                        self.step_to(k, lean, 0);
                    }
                }
                f.y = 0.0;
                v.y = 0.0;
                break;
            }
            if sy < 0 {
                f.y = 0.0;
                v.y = 0.0;
                break;
            }
            f.x = 0.0;
            v.x = 0.0;
            break;
        }
        // Goo sticks to whatever stops its fall.
        if self.ents[k].kind == Kind::Goo && v.y == 0.0 && self.ents[k].v.y > 0.0 {
            let e = self.ents[k];
            let below = self.supported(&e).or_else(|| (self.at(e.x, e.y + 1) == Some(Kind::Goo)).then_some(-1));
            if let Some(id) = below {
                self.ents[k].stuck = Some(id);
                v = V2::ZERO;
                f = V2::ZERO;
            }
        }
        self.ents[k].f = f;
        self.ents[k].v = v;
    }

    /// Bubbles drift with the goo and float up by swapping with the slime above them.
    /// Starts an idle antic, if he's sitting still on something by himself. Bubbles
    /// happen at once; the rest play out over the next second or few.
    pub fn start_antic(&mut self, a: Antic) -> bool {
        if self.grab.is_some() || !self.resting || self.antic.is_some() || self.vb.len() > 0.3 {
            return false;
        }
        self.antic_t = 0.0;
        self.hopping = false;
        self.antic_len = match a {
            Antic::Walk(_) => 1.5 + self.rand() * 2.5,
            Antic::Hop(..) => 6.0,
            Antic::Flatten => 1.0 + self.rand() * 0.8,
            Antic::Stretch => 1.3,
            Antic::Wobble => 1.6,
            Antic::Bubbles => {
                self.bubble_burst();
                self.squash_v += 1.5;
                return true;
            }
        };
        self.antic = Some(a);
        true
    }

    /// The hardest wall hit since last asked (cells per substep beyond a gentle touch).
    pub fn take_wall_hit(&mut self) -> f32 {
        std::mem::take(&mut self.wall_hit)
    }

    /// How hard he's being shaken (0 when still).
    pub fn shake(&self) -> f32 {
        self.shake
    }

    /// Pushes the jelly's side-to-side lean (e.g. reeling when dizzy).
    pub fn sway(&mut self, push: f32) {
        self.lean_v += push;
    }

    pub fn antic(&self) -> Option<Antic> {
        self.antic
    }

    pub fn resting(&self) -> bool {
        self.resting
    }

    /// The jelly's squash (height factor) and lean, for tests.
    #[cfg(test)]
    pub fn jelly(&self) -> (f32, f32) {
        (self.squash, self.lean)
    }

    /// Plays one substep of the antic in progress: sets the walking or hopping motion
    /// and returns how far to raise (or lower) the jelly's rest height.
    fn drive_antic(&mut self, c: V2, r: f32, g: f32) -> f32 {
        let Some(a) = self.antic else { return 0.0 };
        if self.grab.is_some() {
            self.antic = None;
            return 0.0;
        }
        self.antic_t += DT;
        let t = self.antic_t;
        let len = self.antic_len;
        if t >= len {
            self.antic = None;
            return 0.0;
        }
        // Fades in over `rise` seconds and out over the last `fall`.
        let env = |rise: f32, fall: f32| (t / rise).min(1.0).min((len - t) / fall);
        // Would going `d` cells that way take him past the edge of what he's on? (His
        // whole base must stay on it: cells hanging over an edge droop below it, and then
        // he's straddling it and drops through.)
        let off_edge = |w: &World, d: f32| !w.ground_at(w.base_edge(d.signum()) + d + d.signum() * 3.0);
        let tau = std::f32::consts::TAU;
        match a {
            Antic::Walk(mut dir) => {
                // A step is a squash and a surge forward: he inches along.
                let step = (t * WALK_HZ * tau).sin().max(0.0);
                if off_edge(self, dir) {
                    if off_edge(self, -dir) {
                        self.antic = None;
                        return 0.0;
                    }
                    dir = -dir;
                    self.antic = Some(Antic::Walk(dir));
                }
                if self.resting {
                    self.vb.x = dir * WALK_SPEED * self.scale.sqrt() * DT / self.cell * step;
                }
                -0.1 * step * env(0.3, 0.3)
            }
            Antic::Hop(dir, left) => {
                if self.hopping {
                    // Landed: another hop after a beat, or done.
                    if self.resting && self.vb.y.abs() < 0.5 && self.launch_grace == 0.0 {
                        self.hopping = false;
                        if left <= 1 {
                            self.antic = None;
                        } else {
                            self.antic = Some(Antic::Hop(dir, left - 1));
                            self.antic_t = -0.15;
                        }
                    }
                    return 0.0;
                }
                if t < 0.0 {
                    return 0.0;
                }
                // Crouch, then spring.
                if t < HOP_CROUCH {
                    return -0.3 * (t / HOP_CROUCH);
                }
                if !self.resting {
                    self.antic = None;
                    return 0.0;
                }
                let apex = r * HOP_APEX;
                let vy = -(2.0 * g * apex).sqrt();
                let steps = 2.0 * -vy / g;
                let mut dir = dir;
                if off_edge(self, dir * r * HOP_LEN) {
                    dir = if off_edge(self, -dir * r * HOP_LEN) { 0.0 } else { -dir };
                    self.antic = Some(Antic::Hop(dir, left));
                }
                self.vb = v2(dir * r * HOP_LEN / steps, vy);
                self.launched = true;
                self.launch_grace = 0.25;
                self.hopping = true;
                self.squash_v += JELLY_JUMP * 0.6 * self.tune.jelly_kick;
                for e in &mut self.ents {
                    if e.kind != Kind::Goo {
                        e.v = v2(0.0, vy * 0.15 * ((c.y - e.y as f32) / r).clamp(-1.0, 1.0));
                    }
                }
                0.0
            }
            // Let go at the end, so he springs back up with a boing.
            Antic::Flatten => -0.45 * (t / 0.3).min(1.0),
            Antic::Stretch => 0.38 * env(0.5, 0.4),
            Antic::Wobble => {
                self.lean_v += WOBBLE * (t * JELLY_HZ * tau).sin() * env(0.2, 0.4) * DT;
                0.0
            }
            Antic::Bubbles => {
                self.antic = None;
                0.0
            }
        }
    }

    /// The column of the body's base furthest that way (-1 left, 1 right).
    fn base_edge(&self, dir: f32) -> f32 {
        let xs = self.ents.iter().filter(|e| e.kind != Kind::Goo && e.y == self.body_bottom).map(|e| e.x as f32 + 0.5);
        let edge = if dir < 0.0 { xs.fold(f32::MAX, f32::min) } else { xs.fold(f32::MIN, f32::max) };
        if edge.is_finite() && edge.abs() < 1e6 {
            edge
        } else {
            self.body().0.x
        }
    }

    /// Whether there's something to stand on at column `x`, just under the body.
    fn ground_at(&self, x: f32) -> bool {
        let (x, y) = (x.floor() as i32, self.body_bottom + 1);
        if x < 0 || x >= self.w {
            return false;
        }
        y >= self.h || self.void[(y * self.w + x) as usize] || self.plat_id_at(x, y).is_some()
    }

    /// Turns a handful of cells deep inside into bubbles at once.
    fn bubble_burst(&mut self) {
        let (c, _) = self.body();
        let r = self.radius();
        let want = ((5.0 + self.rand() * 7.0) * self.scale.sqrt()) as usize;
        let mut made = 0;
        for _ in 0..want * 30 {
            if made >= want || self.ents.is_empty() {
                break;
            }
            let k = (self.rand() * self.ents.len() as f32) as usize % self.ents.len();
            let e = self.ents[k];
            if e.kind == Kind::Slime && (e.y as f32) > c.y - r * 0.3 && self.neighbours(e.x, e.y) == 8 {
                self.ents[k].kind = Kind::Bubble;
                self.ents[k].age = 0.0;
                made += 1;
            }
        }
    }

    fn move_bubble(&mut self, k: usize) {
        self.move_ent(k);
        let e = self.ents[k];
        if self.rand() < BUBBLE_RISE {
            let d = if self.rand() < 0.5 { -1 } else { 1 };
            for (dx, dy) in [(0, -1), (d, -1)] {
                match self.at(e.x + dx, e.y + dy) {
                    Some(Kind::Slime) => {
                        let other = self.occ[self.idx(e.x + dx, e.y + dy).unwrap()] as usize - 1;
                        self.ents[other].x = e.x;
                        self.ents[other].y = e.y;
                        self.ents[k].x = e.x + dx;
                        self.ents[k].y = e.y + dy;
                        let (a, b) = ((e.y * self.w + e.x) as usize, ((e.y + dy) * self.w + e.x + dx) as usize);
                        self.occ.swap(a, b);
                        return;
                    }
                    None if dx == 0 => {
                        // Reached the surface.
                        self.ents[k].age = -1.0;
                        return;
                    }
                    _ => {}
                }
            }
        }
    }

    /// How far outside the resting dome a cell is (< 1 inside), or 0 when not resting.
    fn dome(&self, x: i32, y: i32) -> f32 {
        match self.template {
            Some(s) => {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                // Lean: higher parts shift sideways.
                let dx = (px - s.cx - s.lean * (s.cy - py)) / s.a;
                let dy = if s.dome { (s.cy - py).max(0.0) / s.b } else { (py - s.cy) / s.b };
                dx * dx + dy * dy
            }
            None => 0.0,
        }
    }

    /// Flow may not carry a slime cell out of the resting dome.
    fn keeps_shape(&self, e: &Ent, dx: i32, dy: i32) -> bool {
        e.kind != Kind::Slime || self.template.is_none() || self.dome(e.x + dx, e.y + dy) < 1.0 || self.dome(e.x, e.y) >= 1.0
    }

    /// While resting, cells sticking out of the dome hop one cell at a time toward free
    /// spots inside it, so the body gathers into a mound instead of a sand pile.
    fn fill_template(&mut self) {
        let Some(s) = self.template else { return };
        // Cells sticking out of the shape, worst first.
        let mut outside: Vec<(usize, f32)> = (0..self.ents.len())
            .filter(|&k| self.ents[k].kind == Kind::Slime)
            .map(|k| (k, self.dome(self.ents[k].x, self.ents[k].y)))
            .filter(|&(_, t)| t >= 1.0)
            .collect();
        if outside.is_empty() {
            return;
        }
        outside.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        // Empty spots inside the shape that touch the body, deepest first.
        let (x0, x1) = ((s.cx - s.a - s.lean.abs() * s.b - 2.0) as i32, (s.cx + s.a + s.lean.abs() * s.b + 2.0) as i32);
        let (y0, y1) = if s.dome { ((s.cy - s.b - 2.0) as i32, s.cy as i32) } else { ((s.cy - s.b - 2.0) as i32, (s.cy + s.b + 2.0) as i32) };
        let mut holes: Vec<(i32, i32, f32)> = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                let Some(i) = self.idx(x, y) else { continue };
                if self.occ[i] != 0 || self.void[i] || self.plat[i] != 0 {
                    continue;
                }
                let t = self.dome(x, y);
                if t < 1.0 && self.neighbours(x, y) >= 2 {
                    holes.push((x, y, t));
                }
            }
        }
        if holes.is_empty() {
            return;
        }
        holes.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
        let budget = ((outside.len() as f32 * RESHAPE_RATE).ceil() as usize).max(1).min(holes.len());
        for &(k, _) in outside.iter().take(budget) {
            let e = self.ents[k];
            if e.kind != Kind::Slime {
                continue;
            }
            // A near hole, preferring deep ones (so the middle fills, not just the sides).
            let Some((hi, _)) = holes
                .iter()
                .enumerate()
                .map(|(i, h)| (i, (((h.0 - e.x).pow(2) + (h.1 - e.y).pow(2)) as f32).sqrt() + HOLE_DEPTH_PULL * h.2))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            else {
                break;
            };
            let (hx, hy, _) = holes.swap_remove(hi);
            self.transport((e.x, e.y), (hx, hy));
        }
    }

    /// Incompressible flow: moves the cell at `from` toward the empty spot `to` by
    /// shifting every body cell on the straight line between them one step along, so
    /// the gap travels back to `from`. Stops early at the first gap on the way.
    fn transport(&mut self, from: (i32, i32), to: (i32, i32)) {
        let mut path = vec![from];
        let (mut x, mut y) = from;
        let (dx, dy) = ((to.0 - x).abs(), -(to.1 - y).abs());
        let (sx, sy) = ((to.0 - x).signum(), (to.1 - y).signum());
        let mut err = dx + dy;
        while (x, y) != to {
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
            let Some(i) = self.idx(x, y) else { return };
            if self.void[i] {
                return;
            }
            path.push((x, y));
            match self.occ[i] {
                0 => break,
                o if self.ents[o as usize - 1].kind == Kind::Goo => return,
                _ => {}
            }
        }
        // path.last() is empty: shift everything one step toward it, front first.
        for j in (0..path.len() - 1).rev() {
            let (a, b) = (path[j], path[j + 1]);
            let ia = (a.1 * self.w + a.0) as usize;
            let ib = (b.1 * self.w + b.0) as usize;
            let o = self.occ[ia];
            if o == 0 || self.occ[ib] != 0 {
                return;
            }
            let k = o as usize - 1;
            self.ents[k].x = b.0;
            self.ents[k].y = b.1;
            self.occ[ib] = o;
            self.occ[ia] = 0;
        }
    }

    /// Enclosed gaps collapse: a cell next to an empty spot that slime surrounds steps
    /// into it if it is no better sheltered where it is. The gap migrates outward and
    /// pops out at the surface, like a vacancy in real goo.
    fn close_holes(&mut self) {
        for k in 0..self.ents.len() {
            let e = self.ents[k];
            if e.kind != Kind::Slime || self.rand() > 0.5 {
                continue;
            }
            let here = self.neighbours(e.x, e.y);
            if here == 8 {
                continue;
            }
            let mut best: Option<((i32, i32), usize)> = None;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if (dx, dy) == (0, 0) || self.at(e.x + dx, e.y + dy).is_some() || !self.free(&e, dx, dy) {
                        continue;
                    }
                    let n = self.neighbours(e.x + dx, e.y + dy) - 1;
                    if n >= 5 && n >= here && best.map_or(true, |b| n > b.1) {
                        best = Some(((dx, dy), n));
                    }
                }
            }
            if let Some(((dx, dy), _)) = best {
                self.step_to(k, dx, dy);
            }
        }
    }

    /// Loose surface cells hop to an adjacent empty spot with more slime around it,
    /// rounding the body off.
    fn surface_tension(&mut self) {
        // Kawasaki dynamics: a surface cell proposes a hop to an adjacent empty spot; the
        // hop is taken if it lowers the energy (more slime neighbours = surface tension,
        // lower = gravity), sometimes if it's level, rarely if it costs energy. Cells can
        // wander along the surface, so the body relaxes into a gravity-flattened drop.
        let height_cost = if self.resting { self.tune.st_gravity } else { 0.0 };
        for k in 0..self.ents.len() {
            let e = self.ents[k];
            if e.kind != Kind::Slime || self.rand() > SURFACE_TENSION {
                continue;
            }
            let here = self.neighbours(e.x, e.y);
            if here >= 8 {
                continue;
            }
            let pick = (self.rand() * 8.0) as usize % 8;
            let (dx, dy) = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)][pick];
            if self.at(e.x + dx, e.y + dy).is_some() || !self.free(&e, dx, dy) {
                continue;
            }
            // Neighbours at the new spot, not counting ourselves.
            let there = self.neighbours(e.x + dx, e.y + dy) - 1;
            if there == 0 {
                continue; // never step off into thin air
            }
            let de = -(there as f32 - here as f32) + height_cost * -(dy as f32);
            let accept = if de < 0.0 {
                true
            } else if de == 0.0 {
                self.rand() < 0.15
            } else {
                self.rand() < (-de / ST_TEMPERATURE).exp()
            };
            if accept {
                self.step_to(k, dx, dy);
            }
        }
    }

    fn detach_and_absorb(&mut self, c: V2, r: f32) {
        let mut k = 0;
        while k < self.ents.len() {
            let e = self.ents[k];
            match e.kind {
                Kind::Slime => {
                    let p = v2(e.x as f32 + 0.5, e.y as f32 + 0.5);
                    if (p - c).len() > r * DETACH {
                        self.ents[k].kind = Kind::Goo;
                    }
                }
                Kind::Bubble if e.age < 0.0 => {
                    self.pops.push(Pop { at: v2(e.x as f32 + 0.5, e.y as f32 + 0.5), age: 0.0 });
                    self.remove(k);
                    continue;
                }
                Kind::Goo if e.age > 0.4 => {
                    // Goo touching the body is soaked back up.
                    let touching = (-1..=1).any(|dy| (-1..=1).any(|dx| self.at(e.x + dx, e.y + dy) == Some(Kind::Slime)));
                    if touching && self.slime_count() < self.target_count() + 20 {
                        self.ents[k].kind = Kind::Slime;
                        self.ents[k].stuck = None;
                        self.ents[k].v = V2::ZERO;
                    }
                }
                _ => {}
            }
            k += 1;
        }
    }

    fn target_count(&self) -> usize {
        (self.base_count as f32 * self.scale * self.scale) as usize
    }

    /// Body surface cells (few slime neighbours) whose offset from the centre `c`
    /// satisfies `side`.
    fn surface_cells(&self, c: V2, side: impl Fn(V2) -> bool) -> Vec<usize> {
        (0..self.ents.len())
            .filter(|&k| {
                let e = self.ents[k];
                e.kind == Kind::Slime && self.neighbours(e.x, e.y) <= 5 && side(v2(e.x as f32, e.y as f32) - c)
            })
            .collect()
    }

    /// Throws a round blob of goo (radius `size` cells, at least one cell) off the body
    /// at surface cell `k`, moving at `v`. The blob is made just outside the surface,
    /// already clear of the body, from the same number of cells taken off around `k`.
    fn fling_blob(&mut self, k: usize, c: V2, size: f32, v: V2) {
        let e = self.ents[k];
        if e.kind != Kind::Slime {
            return;
        }
        let p = v2(e.x as f32, e.y as f32);
        let away = if v.len() > 1e-3 { v } else { p - c };
        let dir = away * (1.0 / away.len().max(1e-3));
        let centre = p + dir * (size + 1.0);
        let size = size.max(0.5);
        // Free spots for the blob.
        let n = size.ceil() as i32 + 1;
        let (cx, cy) = (centre.x.round() as i32, centre.y.round() as i32);
        let mut spots = Vec::new();
        for dy in -n..=n {
            for dx in -n..=n {
                let (x, y) = (cx + dx, cy + dy);
                // (Never through a window edge or floor below the seed.)
                let through = (e.y + 1..=y).any(|yy| self.idx(x, yy).is_some_and(|i| self.plat[i] != 0));
                if (v2(x as f32, y as f32) - centre).len() <= size && !through {
                    if let Some(i) = self.idx(x, y) {
                        if !self.void[i] && self.occ[i] == 0 {
                            spots.push((x, y));
                        }
                    }
                }
            }
        }
        // As many body cells, nearest the seed first.
        let mut near: Vec<(i32, i32)> = Vec::new();
        for dy in -n..=n {
            for dx in -n..=n {
                if self.at(e.x + dx, e.y + dy) == Some(Kind::Slime) {
                    near.push((e.x + dx, e.y + dy));
                }
            }
        }
        near.sort_by_key(|&(x, y)| (x - e.x).pow(2) + (y - e.y).pow(2));
        let count = spots.len().min(near.len());
        for &(x, y) in &near[..count] {
            if let Some(i) = self.idx(x, y) {
                let o = self.occ[i];
                if o != 0 {
                    self.remove(o as usize - 1);
                }
            }
        }
        for &(x, y) in &spots[..count] {
            if self.add(x, y, Kind::Goo, v) {
                let last = self.ents.len() - 1;
                self.ents[last].f = V2::ZERO;
            }
        }
    }

    /// Lays a column of `rows` goo cells, stuck to platform `id`, upwards from (x, y),
    /// if that spot sits on something solid.
    fn lay_goo(&mut self, x: i32, y: i32, rows: i32, id: isize) {
        if rows <= 0 || self.idx(x, y).is_none() {
            return;
        }
        let probe = Ent { x, y, f: V2::ZERO, v: V2::ZERO, kind: Kind::Goo, tint: 0, age: 0.0, life: 0.0, stuck: None };
        if self.supported(&probe).is_none() && self.at(x, y + 1) != Some(Kind::Goo) {
            return;
        }
        for i in 0..rows {
            // Only into free space (the body or older goo may be there already).
            let open = self.idx(x, y - i).is_some_and(|c| !self.void[c]) && self.at(x, y - i).is_none();
            if open && self.add(x, y - i, Kind::Goo, V2::ZERO) {
                let last = self.ents.len() - 1;
                self.ents[last].stuck = Some(id);
            }
        }
    }

    /// Ageing, shedding goo while moving, regrowing lost mass, spawning bubbles.
    fn life_cycle(&mut self, dt: f32) {
        let (c, _) = self.body();
        let r = self.radius();
        let vm = self.vb;
        let speed = vm.len() * self.cell / DT;
        let support = self.support();
        // Drips while carried or flying: small clumps, bigger on a bigger slime and when
        // it's moving faster.
        let shed_rate = if self.grab.is_some() {
            3.0 + speed / 600.0
        } else if speed > 500.0 && support.is_none() {
            speed / 500.0
        } else {
            0.0
        };
        let mut shed = shed_rate * self.tune.shed * dt;
        if shed > 0.0 {
            // Candidates: surface cells on the trailing side (or the underside, when
            // carried, so it drips).
            let carried = self.grab.is_some();
            let dir = if vm.len() > 0.01 { vm * (1.0 / vm.len()) } else { v2(0.0, -1.0) };
            let cands = self.surface_cells(c, |rel| if carried { rel.y > 0.0 } else { rel.dot(dir) < 0.0 });
            while !cands.is_empty() && self.rand() < shed {
                shed -= 1.0;
                let k = cands[(self.rand() * cands.len() as f32) as usize % cands.len()];
                let size = r * 0.02 * (1.0 + speed / 1500.0) * (0.5 + self.rand());
                self.fling_blob(k, c, size, self.vb * 0.4);
            }
        }

        // Hard impacts throw splatter: more and bigger blobs the harder the hit and the
        // bigger the slime, bouncing off the surface and spraying along it.
        let hit = std::mem::take(&mut self.splat);
        let violence = ((hit.len() - 1.0) / 3.0).clamp(0.0, 1.5);
        if violence > 0.0 && self.tune.shed > 0.0 {
            let into = hit * (1.0 / hit.len());
            let along = v2(-into.y, into.x);
            let blobs = ((violence * 6.0 * self.tune.shed).round() as usize).min(30);
            let budget = (self.slime_count() as f32 * 0.15 * self.tune.shed.sqrt()) as usize;
            let before = self.slime_count();
            // From the rim where the body meets the surface, so the blobs spray outward
            // rather than straight back into the body.
            let cands = self.surface_cells(c, |rel| rel.dot(into) > -r * 0.1 && rel.dot(along).abs() > r * 0.45);
            for _ in 0..blobs {
                if cands.is_empty() || before - self.slime_count() > budget {
                    break;
                }
                let k = cands[(self.rand() * cands.len() as f32) as usize % cands.len()];
                let e = self.ents[k];
                if e.kind != Kind::Slime {
                    continue;
                }
                let side = if (v2(e.x as f32, e.y as f32) - c).dot(along) < 0.0 { -1.0 } else { 1.0 };
                let pace = 0.8 + 1.2 * violence;
                let (a, b) = (self.rand(), self.rand());
                let v = into * (-pace * (0.3 + 0.7 * a)) + along * (side * pace * (0.8 + 1.4 * b));
                let size = r * (0.06 + 0.08 * violence) * self.tune.shed.powf(0.3) * (0.6 + 0.6 * self.rand());
                self.fling_blob(k, c, size, v);
            }
        }

        // Sliding lays a smear behind it: dotted when slow, thick when fast, and thicker
        // on a bigger slime.
        let vx = vm.x * self.cell / DT;
        match support {
            Some(id) if self.grab.is_none() && vx.abs() > 40.0 && self.tune.shed > 0.0 => {
                let dir = if vx > 0.0 { 1 } else { -1 };
                let bottom = self.body_bottom;
                let base = self.ents.iter().filter(|e| e.kind == Kind::Slime && e.y == bottom).map(|e| e.x);
                let tail = if dir > 0 { base.min() } else { base.max() };
                if let Some(tail) = tail {
                    let last = tail - dir;
                    let from = match self.trail_x {
                        Some(x) if (last - x) * dir > 0 && (last - x).abs() < 40 => x + dir,
                        _ => last,
                    };
                    let pace = ((vx.abs() - 40.0) / 700.0).clamp(0.0, 1.5);
                    let target = (r * (0.02 + 0.13 * pace) * self.tune.shed.sqrt()).min(r * 0.35);
                    let mut x = from;
                    loop {
                        // Thickness wanders smoothly, like a smear rather than grass.
                        let want = target * (0.6 + 0.8 * self.rand());
                        self.trail_h += (want - self.trail_h) * 0.25;
                        let h = self.trail_h;
                        let rows = if h < 1.0 { (self.rand() < h) as i32 } else { h.round() as i32 };
                        self.lay_goo(x, bottom, rows, id);
                        if x == last {
                            break;
                        }
                        x += dir;
                    }
                    self.trail_x = Some(last);
                }
            }
            _ => {
                self.trail_x = None;
                self.trail_h = 0.0;
            }
        }

        // Regrow toward the target size (also how growing/shrinking works).
        let target = self.target_count();
        let count = self.slime_count();
        if count + 2 < target {
            self.regrow += dt * (40.0 + (target - count) as f32 * 2.0);
            while self.regrow >= 1.0 {
                self.regrow -= 1.0;
                self.grow_one();
            }
        } else if count > target + 2 {
            for _ in 0..((count - target) / 10).max(1) {
                self.shrink_one(c);
            }
        }

        // Bubbles form deep inside.
        self.bubble_in -= dt;
        let bubbles = self.ents.iter().filter(|e| e.kind == Kind::Bubble).count();
        if self.bubble_in <= 0.0 && bubbles < self.tune.bubble_max {
            self.bubble_in = (0.3 + self.rand() * 0.9) / self.tune.bubble_rate.max(0.05);
            for _ in 0..10 {
                let k = (self.rand() * self.ents.len() as f32) as usize % self.ents.len().max(1);
                let e = self.ents[k];
                if e.kind == Kind::Slime && (e.y as f32) > c.y && self.neighbours(e.x, e.y) == 8 {
                    self.ents[k].kind = Kind::Bubble;
                    self.ents[k].age = 0.0;
                    break;
                }
            }
        }

        // Goo ages and fades.
        let mut k = 0;
        while k < self.ents.len() {
            self.ents[k].age += dt;
            let e = self.ents[k];
            if e.kind == Kind::Goo {
                // Lost its footing (window moved away)? Fall again.
                if let Some(id) = e.stuck {
                    let below_goo = self.at(e.x, e.y + 1) == Some(Kind::Goo);
                    if self.supported(&e) != Some(id) && !below_goo {
                        self.ents[k].stuck = None;
                    }
                }
                if e.age >= e.life {
                    self.remove(k);
                    continue;
                }
            }
            k += 1;
        }
    }

    /// Adds one cell of slime in the most sheltered gap on the surface (most slime
    /// around it, low down, inside the dome), so regrowth smooths rather than roughens.
    fn grow_one(&mut self) {
        let mut best: Option<((i32, i32), V2, f32)> = None;
        for _ in 0..24 {
            let k = (self.rand() * self.ents.len() as f32) as usize % self.ents.len().max(1);
            let e = self.ents[k];
            if e.kind != Kind::Slime {
                continue;
            }
            let (dx, dy) = [(0, -1), (-1, 0), (1, 0), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)][(self.rand() * 8.0) as usize % 8];
            let p = (e.x + dx, e.y + dy);
            if !self.free(&e, dx, dy) || self.at(p.0, p.1).is_some() {
                continue;
            }
            let inside = if self.template.is_some() && self.dome(p.0, p.1) < 1.0 { 3.0 } else { 0.0 };
            let score = self.neighbours(p.0, p.1) as f32 + inside + p.1 as f32 * 0.05;
            if best.map_or(true, |b| score > b.2) {
                best = Some((p, e.v, score));
            }
        }
        if let Some((p, v, _)) = best {
            self.add(p.0, p.1, Kind::Slime, v);
        }
    }

    fn shrink_one(&mut self, c: V2) {
        // Remove the outermost of a few samples.
        let mut best: Option<(usize, f32)> = None;
        for _ in 0..8 {
            let k = (self.rand() * self.ents.len() as f32) as usize % self.ents.len().max(1);
            let e = self.ents[k];
            if e.kind != Kind::Slime {
                continue;
            }
            let d = (v2(e.x as f32, e.y as f32) - c).len2();
            if best.map_or(true, |b| d > b.1) {
                best = Some((k, d));
            }
        }
        if let Some((k, _)) = best {
            self.remove(k);
        }
    }

    /// Cell-space bounding box of the body (slime + bubbles), inclusive.
    pub fn body_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        bounds_of(self.ents.iter().filter(|e| e.kind != Kind::Goo))
    }

    pub fn goo_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        bounds_of(self.ents.iter().filter(|e| e.kind == Kind::Goo))
    }

    /// Fraction of the goo's life left (1 fresh .. 0 gone).
    pub fn goo_fade(e: &Ent) -> f32 {
        ((e.life - e.age) / (e.life * 0.4)).clamp(0.0, 1.0)
    }
}

fn bounds_of<'a>(it: impl Iterator<Item = &'a Ent>) -> Option<(i32, i32, i32, i32)> {
    let mut b: Option<(i32, i32, i32, i32)> = None;
    for e in it {
        b = Some(match b {
            None => (e.x, e.y, e.x, e.y),
            Some((x0, y0, x1, y1)) => (x0.min(e.x), y0.min(e.y), x1.max(e.x), y1.max(e.y)),
        });
    }
    b
}

#[cfg(test)]
impl World {
    /// Dome template and how many body cells sit outside it.
    pub fn debug_dome(&self) -> String {
        let outside = self.ents.iter().filter(|e| e.kind == Kind::Slime && self.dome(e.x, e.y) >= 1.0).count();
        let holes = match self.template {
            Some(s) => {
                let mut n = 0;
                let (top, bottom) = if s.dome { (s.cy - s.b, s.cy) } else { (s.cy - s.b, s.cy + s.b) };
                for y in top as i32..bottom as i32 {
                    for x in (s.cx - s.a - 2.0) as i32..=(s.cx + s.a + 2.0) as i32 {
                        if self.dome(x, y) < 1.0 && self.at(x, y).is_none() {
                            n += 1;
                        }
                    }
                }
                n
            }
            None => 0,
        };
        let mut movable = 0;
        let mut lower = 0;
        for e in self.ents.iter().filter(|e| e.kind == Kind::Slime && self.dome(e.x, e.y) >= 1.0) {
            let here = self.dome(e.x, e.y);
            let mut any = false;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if (dx, dy) == (0, 0) || self.at(e.x + dx, e.y + dy).is_some() {
                        continue;
                    }
                    if self.dome(e.x + dx, e.y + dy) < here {
                        lower += 1;
                        if self.free(e, dx, dy) && self.neighbours(e.x + dx, e.y + dy) >= 3 {
                            any = true;
                        }
                    }
                }
            }
            movable += any as usize;
        }
        format!("template {:?} outside {outside} (movable {movable}, lower-empty {lower}) empty-inside {holes} bottom {}", self.template, self.body_bottom)
    }
}

#[cfg(test)]
impl World {
    pub fn debug_jelly(&self) -> String {
        let outside = self.ents.iter().filter(|e| e.kind == Kind::Slime && self.dome(e.x, e.y) >= 1.0).count();
        format!("s {:.2} sv {:.1} lean {:.2} resting {} vb {:.2},{:.2} outside {outside}", self.squash, self.squash_v, self.lean, self.resting, self.vb.x, self.vb.y)
    }
}

impl World {
    /// A little wobble (e.g. when it starts talking).
    pub fn poke(&mut self, amount: f32) {
        self.squash_v += amount;
    }
}

//! Small 2D math helpers and the collision world shared by the simulation and desktop scan.

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};


#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

pub const fn v2(x: f32, y: f32) -> V2 {
    V2 { x, y }
}

impl V2 {
    pub const ZERO: V2 = v2(0.0, 0.0);
    pub fn dot(self, o: V2) -> f32 {
        self.x * o.x + self.y * o.y
    }
    pub fn len2(self) -> f32 {
        self.dot(self)
    }
    pub fn len(self) -> f32 {
        self.len2().sqrt()
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        v2(self.x + o.x, self.y + o.y)
    }
}
impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        v2(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f32> for V2 {
    type Output = V2;
    fn mul(self, s: f32) -> V2 {
        v2(self.x * s, self.y * s)
    }
}
impl Neg for V2 {
    type Output = V2;
    fn neg(self) -> V2 {
        v2(-self.x, -self.y)
    }
}
impl AddAssign for V2 {
    fn add_assign(&mut self, o: V2) {
        self.x += o.x;
        self.y += o.y;
    }
}
impl SubAssign for V2 {
    fn sub_assign(&mut self, o: V2) {
        self.x -= o.x;
        self.y -= o.y;
    }
}

/// One-way platform: solid only when approached from above.
#[derive(Clone, Copy, Debug)]
pub struct Platform {
    pub x0: f32,
    pub x1: f32,
    pub y: f32,
    /// HWND of the owning window, or 0 for a monitor floor (top of taskbar / work area bottom).
    pub id: isize,
}

#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}


//! Finds the surfaces the slime can sit on: the visible parts of top-level window
//! title bars (respecting z-order occlusion) and the bottom of each monitor's work area.
//! The platform layer lists the monitors and windows; this works out what's standable.

use std::collections::HashMap;

use crate::physics::{Bounds, Platform};

/// A screen rectangle in pixels (right and bottom exclusive).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left as f32 && x < self.right as f32 && y >= self.top as f32 && y < self.bottom as f32
    }
}

pub struct Scan {
    pub platforms: Vec<Platform>,
    /// Top-left of every candidate window, keyed by window id, for carrying the slime along.
    pub window_pos: HashMap<isize, (i32, i32)>,
    pub bounds: Bounds,
    pub primary_work: Rect,
    /// Every monitor's full rectangle: the slime must stay inside their union.
    pub monitors: Vec<Rect>,
    /// Each monitor's work area (without the taskbar), in `monitors` order.
    pub work: Vec<Rect>,
    /// Fullscreen windows (F11 in a browser, a video, a borderless game): the window
    /// and the index of the monitor it fills, frontmost first.
    pub fullscreen: Vec<(isize, usize)>,
}

/// A top-level window as the platform sees it.
pub struct Window {
    pub id: isize,
    /// Its visible frame (title bar included).
    pub rect: Rect,
    /// The monitor it fills, if it's fullscreen.
    pub fullscreen: Option<usize>,
}

/// Removes `[a, b]` from a list of disjoint intervals.
fn subtract(spans: &mut Vec<(i32, i32)>, a: i32, b: i32) {
    let mut out = Vec::with_capacity(spans.len() + 1);
    for &(l, r) in spans.iter() {
        if b <= l || a >= r {
            out.push((l, r));
            continue;
        }
        if a > l {
            out.push((l, a));
        }
        if b < r {
            out.push((b, r));
        }
    }
    *spans = out;
}

/// The desktop from its monitors (full and work areas, `primary` being the main one) and
/// its candidate windows, front to back.
pub fn build(monitors: Vec<Rect>, work: Vec<Rect>, primary: usize, windows: impl IntoIterator<Item = Window>) -> Scan {
    let bounds = match monitors.first() {
        Some(_) => Bounds {
            left: monitors.iter().map(|m| m.left).min().unwrap_or(0) as f32,
            top: monitors.iter().map(|m| m.top).min().unwrap_or(0) as f32,
            right: monitors.iter().map(|m| m.right).max().unwrap_or(1920) as f32,
            bottom: monitors.iter().map(|m| m.bottom).max().unwrap_or(1080) as f32,
        },
        None => Bounds { left: 0.0, top: 0.0, right: 1920.0, bottom: 1080.0 },
    };
    let mut platforms = Vec::new();
    for w in &work {
        platforms.push(Platform { x0: w.left as f32, x1: w.right as f32, y: w.bottom as f32, id: 0 });
    }
    let primary_work = work.get(primary).copied().unwrap_or(Rect { left: 0, top: 0, right: 1920, bottom: 1080 });

    // Front to back, so anything already seen is above the current window and may hide
    // parts of its title bar.
    let mut fullscreen = Vec::new();
    let mut above: Vec<Rect> = Vec::new();
    let mut window_pos = HashMap::new();
    for w in windows {
        let r = w.rect;
        window_pos.insert(w.id, (r.left, r.top));
        if let Some(m) = w.fullscreen {
            // Nothing to stand on (its top is the top of the screen), but it hides the
            // title bars behind it.
            fullscreen.push((w.id, m));
            above.push(r);
            continue;
        }
        let mut spans = vec![(r.left, r.right)];
        for o in &above {
            if o.top <= r.top && o.bottom > r.top {
                subtract(&mut spans, o.left, o.right);
            }
        }
        if r.top as f32 > bounds.top + 4.0 {
            for (l, rr) in spans {
                if rr - l >= 8 {
                    platforms.push(Platform { x0: l as f32, x1: rr as f32, y: r.top as f32, id: w.id });
                }
            }
        }
        above.push(r);
    }
    Scan { platforms, window_pos, bounds, primary_work, monitors, work, fullscreen }
}

impl Scan {
    /// The monitor at `(x, y)`, or failing that the nearest one.
    pub fn monitor_near(&self, x: f32, y: f32) -> Option<usize> {
        self.monitors.iter().position(|m| m.contains(x, y)).or_else(|| {
            (0..self.monitors.len()).min_by_key(|&i| {
                let m = self.monitors[i];
                let dx = (m.left as f32 - x).max(0.0).max(x - m.right as f32);
                let dy = (m.top as f32 - y).max(0.0).max(y - m.bottom as f32);
                (dx * dx + dy * dy) as i64
            })
        })
    }
}

/// The monitor a window fills in fullscreen, if it does (Windows): its visible frame `r`
/// and its whole window `outer` both cover exactly that monitor, taskbar included, and it
/// has no title bar. A maximized window keeps its title bar and its outer frame hangs a
/// few pixels past the screen's edges, even with the taskbar hidden. (Its maximized flag
/// is no guide: Firefox keeps it set in fullscreen.)
#[cfg_attr(not(windows), allow(dead_code))]
pub fn fullscreen_on(r: &Rect, outer: &Rect, monitors: &[Rect], captioned: bool) -> Option<usize> {
    if captioned {
        return None;
    }
    let near = |a: i32, b: i32| (a - b).abs() <= 1;
    let fills = |r: &Rect, m: &Rect| near(r.left, m.left) && near(r.top, m.top) && near(r.right, m.right) && near(r.bottom, m.bottom);
    monitors.iter().position(|m| fills(r, m) && fills(outer, m))
}

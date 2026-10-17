//! Finds the surfaces the slime can sit on: the visible parts of top-level window
//! title bars (respecting z-order occlusion) and the bottom of each monitor's work area.

use std::collections::HashMap;
use std::ffi::c_void;

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetSystemMetrics, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, IsIconic,
    IsWindowVisible, GWL_EXSTYLE, GWL_STYLE, MONITORINFOF_PRIMARY, WS_CAPTION, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

use crate::physics::{Bounds, Platform};

pub struct Scan {
    pub platforms: Vec<Platform>,
    /// Top-left of every candidate window, keyed by HWND, for carrying the slime along.
    pub window_pos: HashMap<isize, (i32, i32)>,
    pub bounds: Bounds,
    pub primary_work: RECT,
    /// Every monitor's full rectangle: the slime must stay inside their union.
    pub monitors: Vec<RECT>,
    /// Each monitor's work area (without the taskbar), in `monitors` order.
    pub work: Vec<RECT>,
    /// Fullscreen windows (F11 in a browser, a video, a borderless game): the window
    /// and the index of the monitor it fills, frontmost first.
    pub fullscreen: Vec<(isize, usize)>,
}

/// The monitor a window fills in fullscreen, if it does: its visible frame `r` and its
/// whole window `outer` both cover exactly that monitor, taskbar included, and it has no
/// title bar. A maximized window keeps its title bar and its outer frame hangs a few
/// pixels past the screen's edges, even with the taskbar hidden. (Its maximized flag is
/// no guide: Firefox keeps it set in fullscreen.)
pub fn fullscreen_on(r: &RECT, outer: &RECT, monitors: &[RECT], captioned: bool) -> Option<usize> {
    if captioned {
        return None;
    }
    let near = |a: i32, b: i32| (a - b).abs() <= 1;
    let fills = |r: &RECT, m: &RECT| near(r.left, m.left) && near(r.top, m.top) && near(r.right, m.right) && near(r.bottom, m.bottom);
    monitors.iter().position(|m| fills(r, m) && fills(outer, m))
}

unsafe extern "system" fn collect_windows(hwnd: HWND, lp: LPARAM) -> BOOL {
    (*(lp as *mut Vec<HWND>)).push(hwnd);
    1
}

unsafe extern "system" fn collect_monitors(mon: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(mon, &mut mi) != 0 {
        (*(lp as *mut Vec<MONITORINFO>)).push(mi);
    }
    1
}

const IGNORED_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

unsafe fn candidate_rect(hwnd: HWND) -> Option<RECT> {
    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
        return None;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & (WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT) != 0 {
        return None;
    }
    let mut cloaked: u32 = 0;
    DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as _, &mut cloaked as *mut u32 as *mut c_void, 4);
    if cloaked != 0 || GetWindowTextLengthW(hwnd) == 0 {
        return None;
    }
    let mut cls = [0u16; 64];
    let n = GetClassNameW(hwnd, cls.as_mut_ptr(), cls.len() as i32).max(0) as usize;
    let cls = String::from_utf16_lossy(&cls[..n]);
    if IGNORED_CLASSES.contains(&cls.as_str()) {
        return None;
    }
    let mut r: RECT = std::mem::zeroed();
    let ok = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS as _,
        &mut r as *mut RECT as *mut c_void,
        std::mem::size_of::<RECT>() as u32,
    ) == 0;
    if !ok && GetWindowRect(hwnd, &mut r) == 0 {
        return None;
    }
    if r.right - r.left < 60 || r.bottom - r.top < 30 {
        return None;
    }
    Some(r)
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

pub fn scan(own: HWND) -> Scan {
    unsafe {
        let mut monitors: Vec<MONITORINFO> = Vec::new();
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitors),
            &mut monitors as *mut _ as LPARAM,
        );
        let mut hwnds: Vec<HWND> = Vec::with_capacity(256);
        EnumWindows(Some(collect_windows), &mut hwnds as *mut _ as LPARAM);

        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let bounds = Bounds {
            left: vx as f32,
            top: vy as f32,
            right: (vx + GetSystemMetrics(SM_CXVIRTUALSCREEN)) as f32,
            bottom: (vy + GetSystemMetrics(SM_CYVIRTUALSCREEN)) as f32,
        };

        let mut platforms = Vec::new();
        let mut primary_work = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        for m in &monitors {
            let w = m.rcWork;
            platforms.push(Platform { x0: w.left as f32, x1: w.right as f32, y: w.bottom as f32, id: 0 });
            if m.dwFlags & MONITORINFOF_PRIMARY != 0 {
                primary_work = w;
            }
        }

        // EnumWindows yields top-level windows front-to-back, so anything already
        // seen is above the current window and may hide parts of its title bar.
        let rects: Vec<RECT> = monitors.iter().map(|m| m.rcMonitor).collect();
        let mut fullscreen = Vec::new();
        let mut above: Vec<RECT> = Vec::new();
        let mut window_pos = HashMap::new();
        for hwnd in hwnds {
            if hwnd == own {
                continue;
            }
            let Some(r) = candidate_rect(hwnd) else { continue };
            window_pos.insert(hwnd as isize, (r.left, r.top));
            let captioned = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION == WS_CAPTION;
            let mut outer: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut outer);
            if let Some(m) = fullscreen_on(&r, &outer, &rects, captioned) {
                // Nothing to stand on (its top is the top of the screen), but it hides
                // the title bars behind it.
                fullscreen.push((hwnd as isize, m));
                above.push(r);
                continue;
            }
            let mut spans = vec![(r.left, r.right)];
            for o in &above {
                if o.top <= r.top && o.bottom > r.top {
                    subtract(&mut spans, o.left, o.right);
                }
            }
            if r.top > vy + 4 {
                for (l, rr) in spans {
                    if rr - l >= 8 {
                        platforms.push(Platform { x0: l as f32, x1: rr as f32, y: r.top as f32, id: hwnd as isize });
                    }
                }
            }
            above.push(r);
        }

        let work = monitors.iter().map(|m| m.rcWork).collect();
        Scan { platforms, window_pos, bounds, primary_work, monitors: rects, work, fullscreen }
    }
}

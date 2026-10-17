#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Slime: a desktop companion, simulated Noita-style. The desktop
//! is a grid of cells and the slime is ~1000 individual cells, each moving and colliding
//! on its own; goo and bubbles are cells too.
//!
//! Left-click and drag to pick it up and throw it; left-click anywhere else and it hops
//! toward the click. Right-click it (or the tray icon) for the settings panel: chat, size,
//! which brain answers, reset and quit.

mod brain;
mod desktop;
mod draw;
mod font;
mod fx;
mod guardian;
mod physics;
mod settings;
mod shot;
mod sim;
mod ui;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
};
use windows_sys::Win32::Media::timeBeginPeriod;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture, SetFocus};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use draw::{draw_body, draw_goo, EyeBlock, Look};
use physics::{v2, Bounds, V2};
use sim::{Antic, World, FRAME};

use brain::Provider;
use settings::{Settings, MAX_SIZE, MIN_SIZE};
use ui::{Hit, Section};

const WM_TRAY: u32 = WM_APP + 1;
/// Longest question the chat box accepts (characters).
const MAX_QUESTION: usize = 400;
/// Easter eggs: the most seconds between clicks that still count as a triple click, and
/// how long a rainbow slime takes to go once round the colour wheel.
const TRIPLE_GAP: f32 = 0.5;
const RAINBOW_LAP: f32 = 6.0;
/// Keepy uppies: how long the count stays up after he lands.
const KEEPY_LINGER: f32 = 1.5;

enum Event {
    GrabStart,
    GrabEnd,
    /// Right-clicked while held: pin him there.
    Pin,
    Menu,
    Reset,
    Click(V2),
    /// A character typed on the panel (UTF-16 unit, including control codes).
    Char(u16),
    /// Mouse on the settings panel, in pixels from its corner (drags only need x).
    PanelDown(i32, i32),
    PanelDrag(i32),
    PanelUp,
    /// The panel lost focus or Esc was pressed.
    PanelClose,
}

/// Slider position (0..1) for a slime size, and back.
fn size_to_slider(size: f32) -> f32 {
    (size - MIN_SIZE) / (MAX_SIZE - MIN_SIZE)
}

fn slider_to_size(t: f32) -> f32 {
    let s = MIN_SIZE + t * (MAX_SIZE - MIN_SIZE);
    // Snap to normal size near the notch.
    if (s - 1.0).abs() < 0.05 {
        1.0
    } else {
        (s * 20.0).round() / 20.0
    }
}

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    /// The right button went down to pin him: its release mustn't open the panel.
    static PINNING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn push(e: Event) {
    EVENTS.with(|q| q.borrow_mut().push(e));
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            push(Event::GrabStart);
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            push(Event::GrabEnd);
            0
        }
        WM_CAPTURECHANGED => {
            push(Event::GrabEnd);
            0
        }
        // Right-click while dragging him (he has the mouse captured): a pin.
        WM_RBUTTONDOWN if GetCapture() == hwnd => {
            PINNING.with(|p| p.set(true));
            push(Event::Pin);
            0
        }
        WM_RBUTTONUP => {
            if !PINNING.with(|p| p.replace(false)) {
                push(Event::Menu);
            }
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_TRAY => {
            match (lp as u32) & 0xffff {
                WM_RBUTTONUP | WM_CONTEXTMENU => push(Event::Menu),
                WM_LBUTTONDBLCLK => push(Event::Reset),
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// The settings panel: a focusable window (typing goes to its Chat tab); clicking
/// anywhere else closes it.
unsafe extern "system" fn panel_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            push(Event::PanelDown(x, y));
            0
        }
        WM_MOUSEMOVE if wp & 1 != 0 => {
            push(Event::PanelDrag(x));
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            push(Event::PanelUp);
            0
        }
        WM_ACTIVATE if wp & 0xffff == WA_INACTIVE as usize => {
            push(Event::PanelClose);
            0
        }
        WM_KEYDOWN if wp == 0x1b => {
            push(Event::PanelClose);
            0
        }
        WM_CHAR => {
            push(Event::Char(wp as u16));
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Where the panel goes: at the cursor, kept inside that monitor's work area.
fn panel_origin(at: V2, w: i32, h: i32) -> (i32, i32) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let mon = MonitorFromPoint(POINT { x: at.x as i32, y: at.y as i32 }, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let r = mi.rcWork;
        let x = (at.x as i32 + 8).min(r.right - w).max(r.left);
        // Below the cursor if it fits, otherwise above it (e.g. from the tray).
        let y = if at.y as i32 + h <= r.bottom { at.y as i32 } else { at.y as i32 - h };
        (x, y.min(r.bottom - h).max(r.top))
    }
}

/// Text on the clipboard, if any.
unsafe fn clipboard_text(hwnd: HWND) -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    if OpenClipboard(hwnd) == 0 {
        return None;
    }
    let mut out = None;
    let h = GetClipboardData(CF_UNICODETEXT);
    if !h.is_null() {
        let p = GlobalLock(h) as *const u16;
        if !p.is_null() {
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
            GlobalUnlock(h);
        }
    }
    CloseClipboard();
    out
}

unsafe extern "system" fn mouse_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && wp as u32 == WM_LBUTTONDOWN {
        let info = &*(lp as *const MSLLHOOKSTRUCT);
        push(Event::Click(v2(info.pt.x as f32, info.pt.y as f32)));
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
}

fn cursor() -> V2 {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    v2(p.x as f32, p.y as f32)
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Two black pixel-block eyes riding in the body, spring-smoothed so they lag a little.
pub struct Eyes {
    pos: [V2; 2],
    vel: [V2; 2],
    blink: f32,
    next_blink: f32,
}

impl Eyes {
    pub fn new(world: &World) -> Eyes {
        let a = Self::anchors(world, V2::ZERO);
        Eyes { pos: a, vel: [V2::ZERO; 2], blink: 0.0, next_blink: 2.0 }
    }

    fn anchors(world: &World, look: V2) -> [V2; 2] {
        let c = world.to_cell(world.centroid());
        let r = world.radius();
        [c + v2(-0.32 * r, -0.12 * r) + look, c + v2(0.32 * r, -0.12 * r) + look]
    }

    pub fn update(&mut self, world: &World, look: V2, dt: f32, rand01: f32) {
        let a = Self::anchors(world, look);
        for i in 0..2 {
            for _ in 0..4 {
                let h = dt / 4.0;
                let acc = (a[i] - self.pos[i]) * 700.0 - self.vel[i] * 35.0;
                self.vel[i] += acc * h;
                self.pos[i] += self.vel[i] * h;
            }
            let off = self.pos[i] - a[i];
            let max = world.radius() * 0.15;
            if off.len() > max {
                self.pos[i] = a[i] + off * (max / off.len());
            }
        }
        self.next_blink -= dt;
        if self.blink > 0.0 {
            self.blink += dt;
            if self.blink > 0.15 {
                self.blink = 0.0;
            }
        } else if self.next_blink <= 0.0 {
            self.blink = 1e-4;
            self.next_blink = 2.5 + rand01 * 4.0;
        }
    }

    pub fn blocks(&self, world: &World) -> [EyeBlock; 2] {
        let r = world.radius();
        let w = ((r * 0.12).round() as i32).max(2);
        let h = if self.blink > 0.0 { 1 } else { ((r * 0.22).round() as i32).max(3) };
        let s = ((r / 18.0).round() as i32).max(1);
        self.pos.map(|center| EyeBlock { center, w, h, look: Look::Open, brow: 0, s })
    }
}

/// A 32-bit drawing buffer selected into a memory DC. `cap` is its width (and row
/// stride), `rows` its height.
struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    bits: *mut u32,
    cap: usize,
    rows: usize,
}

impl Surface {
    unsafe fn pixels(&self) -> &'static mut [u32] {
        std::slice::from_raw_parts_mut(self.bits, self.cap * self.rows)
    }

    unsafe fn free(&self) {
        DeleteDC(self.dc);
        DeleteObject(self.bmp);
    }
}

unsafe fn make_surface(screen_dc: HDC, cap: usize) -> Surface {
    make_surface_rect(screen_dc, cap, cap)
}

unsafe fn make_surface_rect(screen_dc: HDC, cap: usize, rows: usize) -> Surface {
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: cap as i32,
        biHeight: -(rows as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..std::mem::zeroed()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    let dc = CreateCompatibleDC(screen_dc);
    SelectObject(dc, bmp);
    Surface { dc, bmp, bits: bits as *mut u32, cap, rows }
}

unsafe fn make_icon(screen_dc: HDC) -> HICON {
    const S: usize = 32;
    let mut w = World::new(&Bounds { left: 0.0, top: 0.0, right: S as f32, bottom: S as f32 }, 2.0, 12.0, 0.0);
    w.spawn(v2(16.0, 19.0));
    let mut px = vec![0u32; S * S];
    if let Some(b) = w.body_bounds() {
        let eyes = Eyes::new(&w).blocks(&w);
        let mut tmp = vec![0u32; S * S];
        let (bw, bh) = draw_body(&mut tmp, S, &w, b, &eyes);
        let (ox, oy) = (b.0 as usize * 2, b.1 as usize * 2);
        for y in 0..bh.min(S - oy) {
            for x in 0..bw.min(S - ox) {
                px[(y + oy) * S + x + ox] = tmp[y * S + x];
            }
        }
    }
    let surf = make_surface(screen_dc, S);
    for (i, &p) in px.iter().enumerate() {
        let a = p >> 24;
        let un = |c: u32| if a == 0 { 0 } else { (c * 255 / a).min(255) };
        *surf.bits.add(i) = (a << 24) | (un((p >> 16) & 255) << 16) | (un((p >> 8) & 255) << 8) | un(p & 255);
    }
    let color = SelectObject(surf.dc, CreateBitmap(1, 1, 1, 1, std::ptr::null()));
    let mask = CreateBitmap(S as i32, S as i32, 1, 1, std::ptr::null());
    let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&info);
    DeleteObject(mask);
    DeleteObject(color);
    icon
}

fn monitor_rects(scan: &desktop::Scan) -> Vec<(i32, i32, i32, i32)> {
    scan.monitors.iter().map(|m| (m.left, m.top, m.right, m.bottom)).collect()
}

/// The fullscreen window filling the monitor at `p` (screen px), if there is one.
fn fullscreen_at(scan: &desktop::Scan, p: V2) -> Option<isize> {
    let m = scan.monitors.iter().position(|m| {
        p.x >= m.left as f32 && p.x < m.right as f32 && p.y >= m.top as f32 && p.y < m.bottom as f32
    })?;
    scan.fullscreen.iter().find(|f| f.1 == m).map(|f| f.0)
}

/// For a leap from `from` to monitor `to`: where the screens meet side by side, and the
/// highest the slime's middle can be there and still be in the opening between both
/// work areas; and the top of the screens it mustn't rise past (screen px, `r` = his radius).
fn leap_gate(scan: &desktop::Scan, from: V2, to: usize, r: f32) -> (Option<V2>, f32) {
    let inside = |m: &RECT| from.x >= m.left as f32 && from.x < m.right as f32;
    let Some(here) = scan.monitors.iter().position(|m| inside(m) && from.y >= m.top as f32 && from.y < m.bottom as f32) else {
        return (None, scan.bounds.top);
    };
    let (a, b) = (scan.work[here], scan.work[to]);
    let ceiling = scan.monitors[here].top.max(scan.monitors[to].top) as f32;
    // (Past the edge by half his width, so his trailing side clears the floor too.)
    let x = if b.left >= a.right {
        a.right as f32 + r * 1.6
    } else if b.right <= a.left {
        a.left as f32 - r * 1.6
    } else {
        return (None, ceiling); // one above the other
    };
    (Some(v2(x, a.bottom.min(b.bottom) as f32 - r * 1.5)), ceiling)
}

/// Puts our windows just behind `fs` (out of always-on-top), or back on top with `None`.
unsafe fn tuck_behind(hwnds: &[HWND], fs: Option<isize>) {
    let after = fs.map_or(HWND_TOPMOST, |f| f as HWND);
    for &h in hwnds {
        SetWindowPos(h, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

fn spawn_point(work: &RECT, r0: f32) -> V2 {
    v2((work.left + work.right) as f32 * 0.5, work.top as f32 + r0 * 2.0)
}

unsafe fn layered_window(class: &[u16], title: &[u16], hinst: *mut c_void, extra: u32) -> HWND {
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | extra,
        class.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        0,
        0,
        1,
        1,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        hinst,
        std::ptr::null(),
    );
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    hwnd
}

fn main() {
    guardian::run(app);
}

/// The slime itself (`respawned`: restarted after a crash).
fn app(respawned: bool) {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        timeBeginPeriod(1);
        let dpi = GetDpiForSystem() as f32 / 96.0;
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("SlimeCompanion");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let title = wide("Slime");
        let goo_hwnd = layered_window(&class, &title, hinst, WS_EX_TRANSPARENT);
        let hwnd = layered_window(&class, &title, hinst, 0);
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hinst, 0);
        // Speech bubble (click-through).
        let bubble_hwnd = layered_window(&class, &title, hinst, WS_EX_TRANSPARENT);
        // Z's, the cigarette and its smoke (click-through, above the body).
        let fx_hwnd = layered_window(&class, &title, hinst, WS_EX_TRANSPARENT);
        let panel_class = wide("SlimeCompanionPanel");
        let pwc = WNDCLASSW {
            lpfnWndProc: Some(panel_wndproc),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            lpszClassName: panel_class.as_ptr(),
            ..wc
        };
        RegisterClassW(&pwc);
        let panel_hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            panel_class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );

        let screen_dc = GetDC(std::ptr::null_mut());
        let surface = make_surface(screen_dc, (1400.0 * dpi) as usize);
        let pixels = std::slice::from_raw_parts_mut(surface.bits, surface.cap * surface.cap);
        // Goo can be left anywhere, so its window can span the whole desktop.
        let goo_size = |b: &Bounds| ((b.right - b.left).ceil() as usize + 8, (b.bottom - b.top).ceil() as usize + 8);
        let ui_surface = make_surface(screen_dc, (640.0 * dpi) as usize);
        let ui_pixels = std::slice::from_raw_parts_mut(ui_surface.bits, ui_surface.cap * ui_surface.cap);
        let panel_surface = make_surface(screen_dc, (1000.0 * dpi) as usize);
        let fx_surface = make_surface(screen_dc, (1200.0 * dpi) as usize);
        let fx_pixels = std::slice::from_raw_parts_mut(fx_surface.bits, fx_surface.cap * fx_surface.cap);
        let panel_pixels = std::slice::from_raw_parts_mut(panel_surface.bits, panel_surface.cap * panel_surface.cap);
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = make_icon(screen_dc);
        for (d, s) in nid.szTip.iter_mut().zip(wide("Slime — right-click for settings")) {
            *d = s;
        }
        Shell_NotifyIconW(NIM_ADD, &nid);

        let mut scan = desktop::scan(hwnd);
        let base_r0 = 56.0 * dpi;
        let cell = (3.0 * dpi).round().max(2.0);
        let new_world = |scan: &desktop::Scan| {
            let mut w = World::new(&scan.bounds, cell, base_r0, 2400.0 * dpi);
            w.seed(std::process::id());
            w.set_platforms(&scan.platforms);
            w.set_monitors(&monitor_rects(scan));
            w.spawn(spawn_point(&scan.primary_work, base_r0));
            w
        };
        let mut world = new_world(&scan);
        let (gw, gh) = goo_size(&scan.bounds);
        let mut goo_surface = make_surface_rect(screen_dc, gw, gh);
        let mut goo_pixels = goo_surface.pixels();
        let mut eyes = Eyes::new(&world);
        let mut rng = Rng(0x9E37_79B9 ^ (std::process::id() | 1));
        let mut prev_windows: HashMap<isize, (i32, i32)> = HashMap::new();
        let mut trail: VecDeque<(Instant, V2)> = VecDeque::new();
        let mut cfg = Settings::load();
        let mut drawn = (0i32, 0i32, 0usize, 0usize);
        let mut goo_shown = false;
        let mut goo_clock = 0.0f32;
        // Talking: facts now and then, and chat answered by Claude Haiku.
        let font_px = (2.0 * dpi).round().max(2.0) as usize;
        let mut brain = brain::Brain::new();
        brain.web = cfg.web;
        let mut speech = brain::Speech::new();
        // The Chat tab: what's being typed, and the conversation so far (true = the user).
        let mut chat_text = String::new();
        let mut chat_log: Vec<(bool, String)> = Vec::new();
        // "Ask about this page": the window in front when the panel opened, and the
        // screenshot waiting to go with the next question.
        let mut shot_target: Option<shot::Target> = None;
        let mut attachment: Option<shot::Shot> = None;
        let mut chat_blink = 0.0f32;
        // Settings panel: where it is on screen, what the mouse is doing on it, and a note
        // that briefly replaces the status line ("Key saved").
        let mut panel_at: Option<(i32, i32)> = None;
        // Which tab is showing, and the slider being dragged (if any).
        let mut panel_tab = 0usize;
        let mut panel_drag: Option<Hit> = None;
        let mut panel_note: Option<(String, f32)> = None;
        let mut panel_show = false;
        // Easter eggs: quick clicks on a section title (which, how many, when the last
        // was), and a moment's pause before nootropic mode lights another cigarette.
        let mut title_taps: (Option<Hit>, u32, Instant) = (None, 0, Instant::now());
        let mut relight_in = 0.0f32;
        // Keepy uppies: bounces in this run (0 when he's not being kept up), the count on
        // show and how long it stays up once the run's over, and whether it's a new best.
        let mut keepy_run = 0u32;
        let mut keepy_shown = (0u32, 0.0f32);
        let mut keepy_record = false;
        // The current provider's key (re-read when it may have changed).
        let mut key_cache: Option<Option<(String, brain::KeySource)>> = None;
        let mut greeted = false;
        let mut fact_in = 4.0f32;
        let mut fact_order: Vec<usize> = (0..brain::FACTS.len()).collect();
        for i in (1..fact_order.len()).rev() {
            let j = (rng.next() * (i + 1) as f32) as usize % (i + 1);
            fact_order.swap(i, j);
        }
        let mut fact_next = 0usize;
        // Idle antics: something to do every so often while he's sitting about, and a
        // look around (seconds left, which way first).
        let mut antic_in = 6.0f32;
        // Fullscreen on his monitor: the window he's hiding behind, and his leaps away.
        let mut hidden_behind: Option<isize> = None;
        let mut leap_wait = 0.0f32;
        let mut leap_tries = 0u32;
        let mut glance = (0.0f32, 1.0f32);
        // Moods (cross, dizzy, asleep, smoking), and how long since anyone bothered him.
        let mut fx = fx::Fx::new(std::process::id() ^ 0x5bd1_e995);
        let mut fx_shown = false;
        let mut idle_for = 0.0f32;
        if respawned {
            greeted = true;
            speech.say("Whoops! Something went splat, but I'm back!");
        }
        let mut bubble_shown = false;

        let mut last = Instant::now();
        let mut acc = 0.0f32;
        let mut calm_for = 0.0f32;
        let mut lost_for = 0.0f32;
        let mut msg: MSG = std::mem::zeroed();

        'main: loop {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    break 'main;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let events: Vec<Event> = EVENTS.with(|q| q.borrow_mut().drain(..).collect());
            let mut close_panel = false;
            for e in events {
                match e {
                    Event::GrabStart => {
                        let p = cursor();
                        // Grabbed again: the pin comes out.
                        if let Some(at) = world.pin_point() {
                            fx.pull_pin(at, world.scale);
                            world.pinned = false;
                        }
                        fx.wake();
                        fx.stub_out(&world);
                        idle_for = 0.0;
                        world.start_grab(p);
                        trail.clear();
                        trail.push_back((Instant::now(), p));
                    }
                    // Pinned: letting go leaves him hanging on the pin.
                    Event::GrabEnd if world.pinned => {}
                    Event::Pin => {
                        if world.pin() {
                            world.poke(-2.0);
                        }
                    }
                    Event::GrabEnd => {
                        let now = Instant::now();
                        let p = cursor();
                        let throw = trail
                            .iter()
                            .find(|(t, _)| now.duration_since(*t).as_secs_f32() < 0.06)
                            .map(|&(t, q)| (p - q) * (1.0 / now.duration_since(t).as_secs_f32().max(0.008)))
                            .unwrap_or(V2::ZERO);
                        let max = 3500.0 * dpi;
                        let throw = if throw.len() > max { throw * (max / throw.len()) } else { throw };
                        world.release(throw);
                    }
                    Event::Click(p) => {
                        let (ox, oy, w, h) = drawn;
                        let (lx, ly) = (p.x as i32 - ox, p.y as i32 - oy);
                        let on_slime = lx >= 0
                            && ly >= 0
                            && (lx as usize) < w
                            && (ly as usize) < h
                            && pixels[ly as usize * surface.cap + lx as usize] >> 24 > 0;
                        // (Not while chatting or in settings: clicks are for those then.)
                        // (Nor for clicks in a fullscreen app.)
                        if cfg.jump && !on_slime && panel_at.is_none() && fullscreen_at(&scan, p).is_none() {
                            fx.wake();
                            idle_for = 0.0;
                            world.jump_toward(p);
                        }
                    }
                    Event::Char(c) if panel_at.is_some() && panel_tab == ui::TAB_CHAT => match c {
                        0x08 => {
                            chat_text.pop();
                        }
                        0x16 => {
                            if let Some(t) = clipboard_text(panel_hwnd) {
                                chat_text.push_str(&brain::clean_input(&t));
                                chat_text.truncate(MAX_QUESTION);
                            }
                        }
                        0x0d => {
                            let q = chat_text.trim().to_string();
                            chat_text.clear();
                            if (q.is_empty() && attachment.is_none()) || brain.waiting {
                                // Nothing to send, or still answering the last one.
                            } else if let Some(p) = Provider::ALL.into_iter().find(|p| p.looks_like_key(&q)) {
                                // A pasted key: keep it and switch to that provider.
                                match brain::save_key(p, &q) {
                                    Ok(()) => {
                                        if cfg.provider != p {
                                            cfg.provider = p;
                                            cfg.save();
                                        }
                                        key_cache = None;
                                        let msg = format!("Got it! I'll use that {} key. Ask me anything!", p.label());
                                        tell(&mut speech, &mut chat_log, &msg);
                                    }
                                    Err(e) => tell(&mut speech, &mut chat_log, &format!("I couldn't save that key: {e}")),
                                }
                            } else {
                                // A screenshot goes with this question (and is then used up).
                                let image = attachment.take().map(|s| {
                                    let q = if q.is_empty() { "What's this? Help me with it." } else { &q };
                                    let img = brain::Image { media_type: s.media_type, base64: shot::base64(&s.bytes) };
                                    (format!("Here's a screenshot of {}. {q}", s.label), format!("[screenshot] {q}"), img)
                                });
                                let (q, shown, image) = match image {
                                    Some((q, shown, img)) => (q, shown, Some(img)),
                                    None => (q.clone(), q, None),
                                };
                                log_chat(&mut chat_log, true, &shown);
                                match backend(&cfg).and_then(|b| brain.ask(&q, b, image)) {
                                    Ok(()) => {
                                        speech.think();
                                        world.poke(2.0);
                                    }
                                    Err(e) => tell(&mut speech, &mut chat_log, &e),
                                }
                            }
                            chat_blink = 0.0;
                        }
                        c if c >= 0x20 && chat_text.len() < MAX_QUESTION => {
                            if let Some(ch) = char::from_u32(c as u32) {
                                chat_text.push_str(&brain::clean_input(&ch.to_string()));
                            }
                            chat_blink = 0.0;
                        }
                        _ => {}
                    },
                    Event::Char(_) => {}
                    Event::Reset => {
                        world.clear_goo();
                        world.spawn(spawn_point(&scan.primary_work, base_r0));
                    }
                    Event::Menu => {
                        fx.wake();
                        idle_for = 0.0;
                        if !world.pinned {
                            world.release(V2::ZERO);
                        }
                        let (pw, ph) = ui::panel_size(font_px);
                        panel_at = Some(panel_origin(cursor(), pw as i32, ph as i32));
                        panel_note = None;
                        panel_drag = None;
                        key_cache = None;
                        panel_show = true;
                        // It opens on the Chat tab; start Claude Code now so answers are quick.
                        panel_tab = ui::TAB_CHAT;
                        // What "Ask about this page" will capture (before the panel takes focus).
                        shot_target = shot::foreground_window(&[hwnd, goo_hwnd, bubble_hwnd, fx_hwnd, panel_hwnd]);
                        chat_blink = 0.0;
                        if let Ok(b) = backend(&cfg) {
                            brain.warm(&b);
                        }
                    }
                    Event::PanelDown(x, y) if panel_at.is_some() => {
                        let note = |s: &str| Some((s.to_string(), 4.0f32));
                        let hit = ui::panel_hit(x, y, panel_tab, cfg.provider.needs_key(), attachment.is_some(), &cfg.collapsed, font_px);
                        let tripled = match hit {
                            Some(h @ Hit::Title(_)) => {
                                let quick = title_taps.0 == Some(h) && title_taps.2.elapsed().as_secs_f32() < TRIPLE_GAP;
                                title_taps = (Some(h), if quick { title_taps.1 + 1 } else { 1 }, Instant::now());
                                title_taps.1 == 3
                            }
                            _ => {
                                title_taps.0 = None;
                                false
                            }
                        };
                        match hit {
                            // (The first two clicks closed and reopened it.)
                            Some(Hit::Title(Section::Colour)) if tripled => {
                                cfg.rainbow = !cfg.rainbow;
                                if cfg.rainbow {
                                    // Grey would hide it.
                                    cfg.look[settings::VIVID] = cfg.look[settings::VIVID].max(0.5);
                                    speech.say("Ooh! I'm every colour at once!");
                                }
                                cfg.save();
                            }
                            Some(Hit::Title(Section::Brain)) if tripled => {
                                cfg.nootropic = !cfg.nootropic;
                                if cfg.nootropic {
                                    relight_in = 0.0;
                                    speech.say("My brain feels... enormous.");
                                } else {
                                    fx.stub_out(&world);
                                    speech.say("Back to my usual brainless self.");
                                }
                                cfg.save();
                            }
                            Some(Hit::Title(s)) => {
                                cfg.collapsed[s as usize] = !cfg.collapsed[s as usize];
                                cfg.save();
                            }
                            Some(Hit::Shoot) => {
                                let at = cursor();
                                let target = shot_target
                                    .clone()
                                    .unwrap_or_else(|| shot::monitor_at(POINT { x: at.x as i32, y: at.y as i32 }));
                                let ours = [hwnd, goo_hwnd, bubble_hwnd, fx_hwnd, panel_hwnd];
                                let max = (ui::THUMB_MAX.0 * font_px, ui::THUMB_MAX.1 * font_px);
                                match shot::capture(&target, &ours, max) {
                                    Ok(s) => attachment = Some(s),
                                    Err(e) => log_chat(&mut chat_log, false, &format!("I couldn't take a screenshot: {e}")),
                                }
                            }
                            Some(Hit::DropShot) => attachment = None,
                            Some(Hit::ClearChat) => {
                                brain.clear();
                                chat_log.clear();
                                if let Ok(b) = backend(&cfg) {
                                    brain.warm(&b);
                                }
                                speech.say("Fresh start! What shall we chat about?");
                            }
                            Some(h @ (Hit::Slider | Hit::Tune(_))) => {
                                panel_drag = Some(h);
                                set_slider(&mut cfg, h, ui::slider_at(h, x, font_px));
                            }
                            Some(Hit::Tab(i)) => panel_tab = i,
                            Some(Hit::Swatch(i)) => {
                                (cfg.look[settings::HUE], cfg.look[settings::VIVID]) = settings::SWATCHES[i];
                                cfg.rainbow = false;
                                cfg.save();
                            }
                            Some(Hit::ResetLook) => {
                                cfg.look = settings::LOOK_DEFAULT;
                                cfg.rainbow = false;
                                cfg.save();
                            }
                            Some(Hit::Close) => close_panel = true,
                            Some(Hit::Jump) => {
                                cfg.jump = !cfg.jump;
                                cfg.save();
                            }
                            Some(Hit::Facts) => {
                                cfg.facts = !cfg.facts;
                                cfg.save();
                            }
                            Some(Hit::Idle) => {
                                cfg.idle = !cfg.idle;
                                cfg.save();
                            }
                            Some(Hit::Keepy) => {
                                cfg.keepy = !cfg.keepy;
                                cfg.save();
                            }
                            Some(Hit::Web) => {
                                cfg.web = !cfg.web;
                                cfg.save();
                                // Claude Code's tools are set when it starts.
                                brain.web = cfg.web;
                                brain.close_session();
                            }
                            Some(h @ (Hit::ProviderPrev | Hit::ProviderNext | Hit::ModelPrev | Hit::ModelNext)) => {
                                match h {
                                    Hit::ProviderPrev => cfg.cycle_provider(-1),
                                    Hit::ProviderNext => cfg.cycle_provider(1),
                                    Hit::ModelPrev => cfg.cycle_model(-1),
                                    _ => cfg.cycle_model(1),
                                }
                                cfg.save();
                                brain.close_session();
                                key_cache = None;
                                panel_note = None;
                            }
                            Some(Hit::PasteKey) => {
                                let text = clipboard_text(panel_hwnd).map(|t| t.trim().to_string()).unwrap_or_default();
                                if cfg.provider.looks_like_key(&text) {
                                    match brain::save_key(cfg.provider, &text) {
                                        Ok(()) => panel_note = note("Key saved. Ask me anything!"),
                                        Err(e) => panel_note = note(&format!("Couldn't save the key: {e}")),
                                    }
                                } else {
                                    panel_note = note(match cfg.provider {
                                        Provider::DeepSeek => "Copy your DeepSeek key (sk-...) first, then press Paste.",
                                        _ => "Copy your Anthropic key (sk-ant-...) first, then press Paste.",
                                    });
                                }
                                key_cache = None;
                            }
                            Some(Hit::ClearKey) => {
                                brain::forget_key(cfg.provider);
                                key_cache = None;
                                panel_note = note("Key removed.");
                            }
                            Some(Hit::Reset) => {
                                world.clear_goo();
                                world.spawn(spawn_point(&scan.primary_work, base_r0));
                            }
                            Some(Hit::Quit) => break 'main,
                            None => {}
                        }
                    }
                    Event::PanelDrag(x) if panel_drag.is_some() => {
                        if let Some(h) = panel_drag {
                            set_slider(&mut cfg, h, ui::slider_at(h, x, font_px));
                        }
                    }
                    Event::PanelUp if panel_drag.is_some() => {
                        panel_drag = None;
                        cfg.save();
                    }
                    Event::PanelClose => close_panel = true,
                    Event::PanelDown(..) | Event::PanelDrag(_) | Event::PanelUp => {}
                }
            }
            if close_panel {
                if panel_at.take().is_some() {
                    ShowWindow(panel_hwnd, SW_HIDE);
                    if panel_drag.take().is_some() {
                        cfg.save();
                    }
                }
            }

            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f32().min(0.1);
            last = now;
            acc += dt;

            scan = desktop::scan(hwnd);
            // Screen layout changed (monitor plugged in, resolution): rebuild the world.
            let b = scan.bounds;
            if ((b.right - b.left) / cell).ceil() as i32 != world.w || ((b.bottom - b.top) / cell).ceil() as i32 != world.h
            {
                world = new_world(&scan);
                eyes = Eyes::new(&world);
                let (gw, gh) = goo_size(&b);
                if gw > goo_surface.cap || gh > goo_surface.rows {
                    goo_surface.free();
                    goo_surface = make_surface_rect(screen_dc, gw, gh);
                    goo_pixels = goo_surface.pixels();
                }
            }
            // What it's resting on must be judged against where the windows *were*,
            // before their edges are redrawn at the new positions.
            let support = if world.grab.is_none() { world.support() } else { None };
            world.set_platforms(&scan.platforms);
            world.set_monitors(&monitor_rects(&scan));
            for (&id, &(nx, ny)) in &scan.window_pos {
                if let Some(&(ox, oy)) = prev_windows.get(&id) {
                    if (ox, oy) != (nx, ny) {
                        let d = v2((nx - ox) as f32, (ny - oy) as f32);
                        if support == Some(id) {
                            world.ride(id, d);
                            calm_for = 0.0;
                        } else {
                            world.ride_goo(id, d);
                        }
                    }
                }
            }
            prev_windows = std::mem::take(&mut scan.window_pos);

            // Something went fullscreen (F11) on his monitor: he leaps to a monitor that's
            // free, or if there isn't one, sits behind it, out of sight, until it's gone.
            let mut hide = None;
            match fullscreen_at(&scan, world.centroid()) {
                Some(fs) if world.grab.is_none() || world.pinned => {
                    let c = world.centroid();
                    let free = (0..scan.monitors.len())
                        .filter(|&m| !scan.fullscreen.iter().any(|f| f.1 == m))
                        .min_by_key(|&m| {
                            let w = scan.work[m];
                            let d = v2((w.left + w.right) as f32 * 0.5, (w.top + w.bottom) as f32 * 0.5) - c;
                            d.len() as i64
                        });
                    match free {
                        // (Pinned, he can't leap: he hides behind it.)
                        Some(_) if world.pinned => hide = Some(fs),
                        Some(m) => {
                            leap_wait -= dt;
                            if leap_wait <= 0.0 && world.resting() {
                                let w = scan.work[m];
                                if leap_tries < 3 {
                                    let target = v2((w.left + w.right) as f32 * 0.5, w.bottom as f32 - base_r0 * world.scale);
                                    let (gate, ceiling) = leap_gate(&scan, c, m, base_r0 * world.scale);
                                    world.leap_to(target, gate, ceiling);
                                } else {
                                    // Kept falling short (say, an awkward gap between screens).
                                    world.spawn(spawn_point(&w, base_r0));
                                }
                                leap_tries += 1;
                                leap_wait = 2.5;
                            }
                        }
                        None => hide = Some(fs),
                    }
                }
                _ => {
                    leap_tries = 0;
                    leap_wait = 0.0;
                }
            }
            if hide != hidden_behind {
                tuck_behind(&[hwnd, goo_hwnd, bubble_hwnd, fx_hwnd], hide);
                hidden_behind = hide;
            }

            if (world.scale - cfg.size).abs() > 1e-3 {
                world.set_scale(world.scale + (cfg.size - world.scale) * (dt * 5.0).min(1.0));
                calm_for = 0.0;
            }

            let mouse = cursor();
            if world.grab.is_some() && !world.pinned {
                world.drag_to(mouse);
                trail.push_back((now, mouse));
                while trail.len() > 2 && now.duration_since(trail[0].0).as_secs_f32() > 0.15 {
                    trail.pop_front();
                }
            }
            // Customisation: colours and feel follow the settings live (a rainbow slime's
            // hue slides round the wheel, knob and all).
            if cfg.rainbow {
                cfg.look[settings::HUE] = (cfg.look[settings::HUE] + dt / RAINBOW_LAP).rem_euclid(1.0);
            }
            world.tune = cfg.tuning();
            world.palette = cfg.palette();
            // Keepy uppies: falling onto the cursor, he bounces; the run ends when he lands
            // or is picked up.
            if cfg.keepy && world.keepy_up(mouse) {
                keepy_run += 1;
                if keepy_run > cfg.keepy_best {
                    cfg.keepy_best = keepy_run;
                    keepy_record = true;
                }
                keepy_shown = (keepy_run, f32::INFINITY);
                fx.wake();
                idle_for = 0.0;
            }
            while acc >= FRAME {
                world.step(FRAME);
                acc -= FRAME;
            }
            // Moods: cross after a hard knock into a wall, dizzy after a shaking (reeling
            // once he's put down), lying low while asleep, waking with a stretch.
            fx.hit_wall(world.take_wall_hit());
            fx.pin = world.pin_point();
            if world.grab.is_some() && !world.pinned {
                fx.shaken(world.shake());
            } else {
                world.sway(fx.sway(dt));
            }
            world.rest_bias = fx.rest_bias();
            let was_asleep = fx.asleep();
            fx.update(dt, &world);
            if was_asleep && !fx.asleep() {
                world.start_antic(Antic::Stretch);
            }
            if keepy_run > 0 && (world.resting() || world.grab.is_some()) {
                if keepy_record {
                    cfg.save();
                    if keepy_run >= 3 {
                        speech.say(&format!("New record: {keepy_run} keepy uppies!"));
                    }
                }
                keepy_run = 0;
                keepy_shown.1 = KEEPY_LINGER;
            }
            if keepy_run == 0 {
                keepy_shown.1 -= dt;
                if keepy_shown.1 <= 0.0 {
                    keepy_record = false;
                }
            }
            fx.keepy = (keepy_shown.0 > 0 && keepy_shown.1 > 0.0).then(|| (keepy_shown.0, (keepy_shown.1 / 0.4).min(1.0), keepy_record));
            // Nootropic mode: a fresh cigarette shortly after each one is knocked away.
            if cfg.nootropic && !fx.smoking() && !fx.asleep() && (world.grab.is_none() || world.pinned) {
                relight_in -= dt;
                if relight_in <= 0.0 {
                    relight_in = 1.5;
                    fx.light_up(f32::INFINITY, if rng.next() < 0.5 { -1.0 } else { 1.0 });
                }
            }
            idle_for += dt;
            // Safety net: if he's somehow off every screen, or mostly gone, bring him back.
            let c = world.centroid();
            let seen = scan.monitors.iter().any(|m| {
                c.x >= m.left as f32 && c.x < m.right as f32 && c.y >= m.top as f32 && c.y < m.bottom as f32
            });
            if world.grab.is_none() && (!seen || world.intact() < 0.25) {
                lost_for += dt;
            } else {
                lost_for = 0.0;
            }
            if lost_for > 1.0 {
                lost_for = 0.0;
                world.clear_goo();
                world.spawn(spawn_point(&scan.primary_work, base_r0));
                speech.say("Whoops! I got lost for a moment. I'm back!");
            }

            // --- Idle antics ---
            if cfg.idle && !fx.busy() && hidden_behind.is_none() && panel_at.is_none() && world.grab.is_none() && world.resting() && world.antic().is_none() && glance.0 <= 0.0 {
                antic_in -= dt;
                if antic_in <= 0.0 {
                    antic_in = 5.0 + rng.next() * 12.0;
                    let dir = if rng.next() < 0.5 { -1.0 } else { 1.0 };
                    let pick = rng.next();
                    let antic = if pick < 0.24 {
                        Some(Antic::Walk(dir))
                    } else if pick < 0.40 {
                        Some(Antic::Hop(dir, 1 + (rng.next() * 3.0) as u32))
                    } else if pick < 0.50 {
                        Some(Antic::Flatten)
                    } else if pick < 0.59 {
                        Some(Antic::Stretch)
                    } else if pick < 0.69 {
                        Some(Antic::Wobble)
                    } else if pick < 0.78 {
                        Some(Antic::Bubbles)
                    } else {
                        None
                    };
                    match antic {
                        Some(a) => {
                            world.start_antic(a);
                        }
                        // A smoke break.
                        None if (0.86..0.94).contains(&pick) => fx.light_up(25.0 + rng.next() * 20.0, dir),
                        // A nap, if he's been left alone a while.
                        None if pick >= 0.94 && idle_for > 40.0 => fx.fall_asleep(30.0 + rng.next() * 60.0),
                        None => glance = (1.6, dir),
                    }
                }
            }
            glance.0 = (glance.0 - dt).max(0.0);

            let to_mouse = world.to_cell(mouse) - world.to_cell(world.centroid());
            let r = world.radius();
            // Eyes follow the mouse, or look where he's going, or glance one way then the other.
            let look = match world.antic() {
                _ if fx.eyes_still() => V2::ZERO,
                Some(Antic::Walk(d) | Antic::Hop(d, _)) => v2(d * r * 0.06, 0.0),
                _ if glance.0 > 0.0 => v2(if glance.0 > 0.8 { glance.1 } else { -glance.1 } * r * 0.07, 0.0),
                _ => to_mouse * (r * 0.06 / to_mouse.len().max(r * 3.0)),
            };
            eyes.update(&world, look, dt, rng.next());

            // Body window.
            if let Some((x0, y0, x1, y1)) = world.body_bounds() {
                let b = (x0 - 1, y0 - 1, x1 + 1, y1 + 1);
                let mut face = eyes.blocks(&world);
                fx.style_eyes(&mut face);
                let (w, h) = draw_body(pixels, surface.cap, &world, b, &face);
                let o = world.to_screen(v2(b.0 as f32, b.1 as f32));
                drawn = (o.x as i32, o.y as i32, w, h);
                let dst = POINT { x: o.x as i32, y: o.y as i32 };
                let size = SIZE { cx: w as i32, cy: h as i32 };
                let src = POINT { x: 0, y: 0 };
                UpdateLayeredWindow(hwnd, screen_dc, &dst, &size, surface.dc, &src, 0, &blend, ULW_ALPHA);
            }

            // Effects window.
            match fx.draw(fx_pixels, fx_surface.cap, fx_surface.cap, &world) {
                Some((x, y, w, h)) => {
                    let dst = POINT { x, y };
                    let size = SIZE { cx: w as i32, cy: h as i32 };
                    let src = POINT { x: 0, y: 0 };
                    UpdateLayeredWindow(fx_hwnd, screen_dc, &dst, &size, fx_surface.dc, &src, 0, &blend, ULW_ALPHA);
                    fx_shown = true;
                }
                None if fx_shown => {
                    fx_pixels[0] = 0;
                    let dst = POINT { x: 0, y: 0 };
                    let size = SIZE { cx: 1, cy: 1 };
                    let src = POINT { x: 0, y: 0 };
                    UpdateLayeredWindow(fx_hwnd, screen_dc, &dst, &size, fx_surface.dc, &src, 0, &blend, ULW_ALPHA);
                    fx_shown = false;
                }
                None => {}
            }

            // --- Talking ---
            if let Some(reply) = brain.poll() {
                match reply {
                    brain::Reply::Text(t) | brain::Reply::Error(t) => {
                        fx.wake();
                        tell(&mut speech, &mut chat_log, &t);
                        world.poke(3.0);
                    }
                    brain::Reply::Searching(what) => {
                        speech.think_about(&what);
                        world.poke(1.0);
                    }
                }
            }
            speech.update(dt);
            chat_blink += dt;
            if cfg.facts && !fx.asleep() && hidden_behind.is_none() && panel_at.is_none() && !speech.busy() {
                fact_in -= dt;
                if fact_in <= 0.0 {
                    if greeted {
                        speech.say(brain::FACTS[fact_order[fact_next % fact_order.len()]]);
                        fact_next += 1;
                    } else {
                        speech.say("Hi! I'm Slime. Right-click me to chat!");
                        greeted = true;
                    }
                    world.poke(2.0);
                    fact_in = 60.0 + rng.next() * 90.0;
                }
            }
            // The speech bubble, just above his head.
            let (sx, sy, sw, _) = drawn;
            let head_x = sx + sw as i32 / 2;
            let (min_x, max_x, min_y) = (scan.bounds.left as i32, scan.bounds.right as i32, scan.bounds.top as i32);
            let place_x = |w: usize| (head_x - w as i32 / 2).clamp(min_x, (max_x - w as i32).max(min_x));
            if let Some((lines, shown)) = speech.view() {
                let (bw, bh) = ui::bubble_size(&lines, font_px);
                let (bx, by) = (place_x(bw), (sy - bh as i32).max(min_y));
                let tail = (head_x - bx) as f32 / bw as f32;
                ui::draw_bubble(ui_pixels, ui_surface.cap, &lines, shown, tail, font_px);
                let dst = POINT { x: bx, y: by };
                let size = SIZE { cx: bw as i32, cy: bh as i32 };
                let src = POINT { x: 0, y: 0 };
                UpdateLayeredWindow(bubble_hwnd, screen_dc, &dst, &size, ui_surface.dc, &src, 0, &blend, ULW_ALPHA);
                bubble_shown = true;
            } else if bubble_shown {
                ui_pixels[0] = 0;
                let dst = POINT { x: 0, y: 0 };
                let size = SIZE { cx: 1, cy: 1 };
                let src = POINT { x: 0, y: 0 };
                UpdateLayeredWindow(bubble_hwnd, screen_dc, &dst, &size, ui_surface.dc, &src, 0, &blend, ULW_ALPHA);
                bubble_shown = false;
            }

            // Settings panel.
            if let Some((px0, py0)) = panel_at {
                if let Some((_, t)) = &mut panel_note {
                    *t -= dt;
                    if *t <= 0.0 {
                        panel_note = None;
                    }
                }
                let key = key_cache.get_or_insert_with(|| brain::api_key(cfg.provider));
                let (key_text, status) = panel_texts(&cfg, key.as_ref(), &brain.session_state(), brain::claude_exe().is_some());
                let status = panel_note.as_ref().map_or(status, |n| n.0.clone());
                let m = cursor();
                let keyb = cfg.provider.needs_key();
                let hover = panel_drag
                    .or_else(|| ui::panel_hit(m.x as i32 - px0, m.y as i32 - py0, panel_tab, keyb, attachment.is_some(), &cfg.collapsed, font_px));
                let brain_line = match backend(&cfg) {
                    Ok(brain::Backend::Cli { model, .. }) if cfg.provider.needs_key() => {
                        format!("Brain: Claude plan ({model}), no key yet")
                    }
                    Ok(_) => format!("Brain: {}, {}", cfg.provider.label(), cfg.model().1),
                    Err(_) => "Brain: not set up yet (see Settings)".to_string(),
                };
                let view = ui::PanelView {
                    tab: panel_tab,
                    chat: &chat_log,
                    thinking: brain.waiting,
                    input: &chat_text,
                    cursor_on: chat_blink % 1.0 < 0.55,
                    brain_line: &brain_line,
                    shot: attachment.as_ref().map(|s| ui::ShotView { label: &s.label, thumb: &s.thumb, w: s.thumb_w, h: s.thumb_h }),
                    size: size_to_slider(cfg.size),
                    size_normal: size_to_slider(1.0),
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
                    key_buttons: keyb,
                    key_clearable: matches!(key, Some((_, brain::KeySource::File))),
                    status: &status,
                    look_labels: settings::LOOK.map(|l| l.1),
                    look: cfg.look,
                    look_normal: settings::LOOK_DEFAULT,
                    swatches: settings::swatch_colours(),
                    swatch_on: settings::SWATCHES.iter().position(|&(h, v)| {
                        !cfg.rainbow && (cfg.look[settings::HUE] - h).abs() < 0.004 && (cfg.look[settings::VIVID] - v).abs() < 0.004
                    }),
                    rainbow: cfg.rainbow,
                    nootropic: cfg.nootropic,
                    collapsed: cfg.collapsed,
                    hover,
                };
                let (pw, ph) = ui::draw_panel(panel_pixels, panel_surface.cap, &view, font_px);
                let dst = POINT { x: px0, y: py0 };
                let size = SIZE { cx: pw as i32, cy: ph as i32 };
                let src = POINT { x: 0, y: 0 };
                UpdateLayeredWindow(panel_hwnd, screen_dc, &dst, &size, panel_surface.dc, &src, 0, &blend, ULW_ALPHA);
                if panel_show {
                    panel_show = false;
                    ShowWindow(panel_hwnd, SW_SHOW);
                    SetForegroundWindow(panel_hwnd);
                    SetFocus(panel_hwnd);
                }
                calm_for = 0.0;
            }

            // Goo window, up to 30 fps.
            goo_clock += dt;
            if goo_clock >= 1.0 / 30.0 {
                goo_clock = 0.0;
                match world.goo_bounds() {
                    Some((x0, y0, x1, y1)) => {
                        let b = (x0, y0, x1, y1);
                        let (w, h) = draw_goo(goo_pixels, goo_surface.cap, &world, b);
                        let o = world.to_screen(v2(x0 as f32, y0 as f32));
                        let dst = POINT { x: o.x as i32, y: o.y as i32 };
                        let size = SIZE { cx: w as i32, cy: h as i32 };
                        let src = POINT { x: 0, y: 0 };
                        UpdateLayeredWindow(goo_hwnd, screen_dc, &dst, &size, goo_surface.dc, &src, 0, &blend, ULW_ALPHA);
                        goo_shown = true;
                    }
                    None if goo_shown => {
                        goo_pixels[0] = 0;
                        let dst = POINT { x: 0, y: 0 };
                        let size = SIZE { cx: 1, cy: 1 };
                        let src = POINT { x: 0, y: 0 };
                        UpdateLayeredWindow(goo_hwnd, screen_dc, &dst, &size, goo_surface.dc, &src, 0, &blend, ULW_ALPHA);
                        goo_shown = false;
                    }
                    None => {}
                }
            }

            if (world.grab.is_some() && !world.pinned) || world.velocity().len() > 15.0 || world.antic().is_some() || glance.0 > 0.0 || fx.animating() || fx.dizzy > 0.0 {
                calm_for = 0.0;
            } else {
                calm_for += dt;
            }
            let frame = if calm_for > 1.5 { 1.0 / 30.0 } else { FRAME };
            let wait = Duration::from_secs_f32(frame).saturating_sub(now.elapsed());
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait.as_millis() as u32, QS_ALLINPUT);
        }

        UnhookWindowsHookEx(hook);
        Shell_NotifyIconW(NIM_DELETE, &nid);
        DestroyWindow(hwnd);
        DestroyWindow(goo_hwnd);
        DestroyWindow(bubble_hwnd);
        DestroyWindow(fx_hwnd);
        DestroyWindow(panel_hwnd);
    }
}

/// Moves slider `h` to position `t` (0..1). Customisation sliders snap to their standard
/// setting near the notch.
fn set_slider(cfg: &mut Settings, h: Hit, t: f32) {
    match h {
        Hit::Slider => cfg.size = slider_to_size(t),
        Hit::Tune(i) => {
            if i == settings::HUE {
                cfg.rainbow = false;
            }
            let normal = settings::LOOK_DEFAULT[i];
            cfg.look[i] = if (t - normal).abs() < 0.025 { normal } else { t };
        }
        _ => {}
    }
}

/// Adds a message to the chat log, keeping the last few dozen.
fn log_chat(log: &mut Vec<(bool, String)>, user: bool, text: &str) {
    log.push((user, font::to_ascii(text)));
    let excess = log.len().saturating_sub(40);
    log.drain(..excess);
}

/// He says `text` in his bubble, and it goes in the chat log.
fn tell(speech: &mut brain::Speech, log: &mut Vec<(bool, String)>, text: &str) {
    speech.say(text);
    log_chat(log, false, text);
}

/// Who answers chat with the current settings (an API without a key falls back to the
/// Claude plan when Claude Code is installed).
fn backend(cfg: &Settings) -> Result<brain::Backend, String> {
    let key = brain::api_key(cfg.provider).map(|k| k.0);
    brain::resolve(cfg.provider, cfg.model().0, key, brain::claude_exe())
}

/// The panel's key line and status line.
fn panel_texts(
    cfg: &Settings,
    key: Option<&(String, brain::KeySource)>,
    session: &brain::SessionState,
    have_cli: bool,
) -> (String, String) {
    use brain::SessionState as S;
    if !cfg.provider.needs_key() {
        let status = match (have_cli, session) {
            (false, _) => "Install Claude Code and sign in to use your plan.".to_string(),
            (true, S::Closed) => "Uses your Claude plan via Claude Code. Opens when you chat.".into(),
            (true, S::Starting) => "Starting Claude Code...".into(),
            (true, S::Ready) => "Claude Code session open: replies are quick.".into(),
            (true, S::Failed(e)) => e.clone(),
        };
        let key = if have_cli { "your Claude login" } else { "Claude Code not found" };
        return (key.into(), status);
    }
    match key {
        Some((k, src)) => {
            let shown = brain::mask_key(k);
            let shown = if *src == brain::KeySource::Env { format!("env {}", &shown[3..]) } else { shown };
            (shown, format!("Answers come from {} with your key.", cfg.model().1))
        }
        None if have_cli => ("none yet".into(), "No key yet, so your Claude plan answers. Copy a key, then Paste.".into()),
        None => ("none yet".into(), "Copy your API key, then press Paste.".into()),
    }
}

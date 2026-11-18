//! Windows: per-pixel-alpha layered windows (`UpdateLayeredWindow`) for each layer, a
//! low-level mouse hook for click-to-jump, a tray icon, and GDI screen capture.

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Duration;

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetMonitorInfoW,
    SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HMONITOR, MONITORINFO,
};
use windows_sys::Win32::Media::timeBeginPeriod;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture, SetFocus};
use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::{Event, Layer};
use crate::desktop::{self, Rect, Scan};
use crate::physics::{v2, V2};
use crate::shot;

const WM_TRAY: u32 = WM_APP + 1;

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    /// The right button went down to pin him: its release mustn't open the panel.
    static PINNING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn push(e: Event) {
    EVENTS.with(|q| q.borrow_mut().push(e));
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn rect(r: RECT) -> Rect {
    Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
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

unsafe extern "system" fn mouse_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && wp as u32 == WM_LBUTTONDOWN {
        let info = &*(lp as *const MSLLHOOKSTRUCT);
        push(Event::Click(v2(info.pt.x as f32, info.pt.y as f32)));
    }
    CallNextHookEx(std::ptr::null_mut(), code, wp, lp)
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
    unsafe fn new(screen_dc: HDC, cap: usize, rows: usize) -> Surface {
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

    unsafe fn pixels(&self) -> &mut [u32] {
        std::slice::from_raw_parts_mut(self.bits, self.cap * self.rows)
    }

    unsafe fn free(&self) {
        DeleteDC(self.dc);
        DeleteObject(self.bmp);
    }
}

/// A tray icon from premultiplied pixels (`s` x `s`).
unsafe fn make_icon(screen_dc: HDC, px: &[u32], s: usize) -> HICON {
    let surf = Surface::new(screen_dc, s, s);
    for (i, &p) in px.iter().enumerate().take(s * s) {
        let a = p >> 24;
        let un = |c: u32| if a == 0 { 0 } else { (c * 255 / a).min(255) };
        *surf.bits.add(i) = (a << 24) | (un((p >> 16) & 255) << 16) | (un((p >> 8) & 255) << 8) | un(p & 255);
    }
    let color = SelectObject(surf.dc, CreateBitmap(1, 1, 1, 1, std::ptr::null()));
    let mask = CreateBitmap(s as i32, s as i32, 1, 1, std::ptr::null());
    let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let icon = CreateIconIndirect(&info);
    DeleteObject(mask);
    DeleteObject(color);
    icon
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

/// Puts our windows just behind `fs` (out of always-on-top), or back on top with `None`.
pub unsafe fn tuck_behind(hwnds: &[HWND], fs: Option<isize>) {
    let after = fs.map_or(HWND_TOPMOST, |f| f as HWND);
    for &h in hwnds {
        SetWindowPos(h, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub struct Host {
    /// One window per layer, in `Layer` order.
    hwnds: [HWND; 5],
    surfaces: Vec<Surface>,
    shown: [bool; 5],
    screen_dc: HDC,
    nid: NOTIFYICONDATAW,
    hook: HHOOK,
    dpi: f32,
}

impl Host {
    /// Opens the windows and puts `icon` (premultiplied, `icon_size` square) in the tray.
    pub fn new(icon: &[u32], icon_size: usize) -> Host {
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
            let goo = layered_window(&class, &title, hinst, WS_EX_TRANSPARENT);
            let body = layered_window(&class, &title, hinst, 0);
            let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hinst, 0);
            // Speech bubble (click-through).
            let bubble = layered_window(&class, &title, hinst, WS_EX_TRANSPARENT);
            // Z's, the cigarette and its smoke (click-through, above the body).
            let fx = layered_window(&class, &title, hinst, WS_EX_TRANSPARENT);
            let panel_class = wide("SlimeCompanionPanel");
            let pwc = WNDCLASSW {
                lpfnWndProc: Some(panel_wndproc),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                lpszClassName: panel_class.as_ptr(),
                ..wc
            };
            RegisterClassW(&pwc);
            let panel = CreateWindowExW(
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
            let surfaces = (0..5).map(|_| Surface::new(screen_dc, 64, 64)).collect();

            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = body;
            nid.uID = 1;
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY;
            nid.hIcon = make_icon(screen_dc, icon, icon_size);
            for (d, s) in nid.szTip.iter_mut().zip(wide("Slime — right-click for settings")) {
                *d = s;
            }
            Shell_NotifyIconW(NIM_ADD, &nid);
            Host { hwnds: [goo, body, fx, bubble, panel], surfaces, shown: [false; 5], screen_dc, nid, hook, dpi }
        }
    }

    pub fn dpi(&self) -> f32 {
        self.dpi
    }

    /// Whether click-to-jump can work here.
    pub fn sees_clicks(&self) -> bool {
        true
    }

    /// Handles window messages; returns what happened.
    pub fn events(&mut self) -> Vec<Event> {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    push(Event::Quit);
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        EVENTS.with(|q| q.borrow_mut().drain(..).collect())
    }

    pub fn cursor(&self) -> V2 {
        let mut p = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut p) };
        v2(p.x as f32, p.y as f32)
    }

    pub fn scan(&mut self) -> Scan {
        unsafe { scan(self.hwnds[Layer::Body.index()]) }
    }

    /// Shows `px` (premultiplied BGRA rows `stride` apart) as `layer`, `w` x `h` at (`x`, `y`).
    pub fn show(&mut self, layer: Layer, x: i32, y: i32, w: usize, h: usize, px: &[u32], stride: usize) {
        let i = layer.index();
        unsafe {
            if w > self.surfaces[i].cap || h > self.surfaces[i].rows {
                self.surfaces[i].free();
                let s = &self.surfaces[i];
                let (cw, ch) = (w.max(s.cap), h.max(s.rows));
                self.surfaces[i] = Surface::new(self.screen_dc, cw, ch);
            }
            let s = &self.surfaces[i];
            let dst = s.pixels();
            for row in 0..h {
                dst[row * s.cap..row * s.cap + w].copy_from_slice(&px[row * stride..row * stride + w]);
            }
            self.update(i, x, y, w, h);
        }
        self.shown[i] = true;
    }

    unsafe fn update(&self, i: usize, x: i32, y: i32, w: usize, h: usize) {
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let dst = POINT { x, y };
        let size = SIZE { cx: w as i32, cy: h as i32 };
        let src = POINT { x: 0, y: 0 };
        UpdateLayeredWindow(self.hwnds[i], self.screen_dc, &dst, &size, self.surfaces[i].dc, &src, 0, &blend, ULW_ALPHA);
    }

    pub fn hide(&mut self, layer: Layer) {
        let i = layer.index();
        if !self.shown[i] {
            return;
        }
        self.shown[i] = false;
        unsafe {
            if layer == Layer::Panel {
                ShowWindow(self.hwnds[i], SW_HIDE);
            } else {
                self.surfaces[i].pixels()[0] = 0;
                self.update(i, 0, 0, 1, 1);
            }
        }
    }

    /// Brings the (just drawn) panel up and gives it the keyboard.
    pub fn focus_panel(&mut self) {
        let h = self.hwnds[Layer::Panel.index()];
        unsafe {
            ShowWindow(h, SW_SHOW);
            SetForegroundWindow(h);
            SetFocus(h);
        }
    }

    /// Tucks the slime's windows behind fullscreen window `fs`, or back on top.
    pub fn hide_behind(&mut self, fs: Option<isize>) {
        unsafe { tuck_behind(&self.hwnds[..4], fs) }
    }

    /// Text on the clipboard, if any.
    pub fn clipboard(&self) -> Option<String> {
        use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
        use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        const CF_UNICODETEXT: u32 = 13;
        unsafe {
            if OpenClipboard(self.hwnds[Layer::Panel.index()]) == 0 {
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
    }

    /// The window the user is looking at (before the panel takes focus).
    pub fn shot_target(&self) -> Option<shot::Target> {
        shot::foreground_window(&self.hwnds)
    }

    /// The whole monitor at `at`.
    pub fn monitor_target(&self, at: V2) -> shot::Target {
        shot::monitor_at(POINT { x: at.x as i32, y: at.y as i32 })
    }

    pub fn capture(&mut self, target: &shot::Target, thumb_max: (usize, usize)) -> Result<shot::Shot, String> {
        shot::capture(target, &self.hwnds, thumb_max)
    }

    /// Sleeps up to `d`, waking early for input.
    pub fn wait(&mut self, d: Duration) {
        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, d.as_millis() as u32, QS_ALLINPUT) };
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            UnhookWindowsHookEx(self.hook);
            Shell_NotifyIconW(NIM_DELETE, &self.nid);
            for &h in &self.hwnds {
                DestroyWindow(h);
            }
            for s in &self.surfaces {
                s.free();
            }
        }
    }
}

// --- What's on the desktop ---

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

/// The monitors and the windows on them (`own` is left out).
pub unsafe fn scan(own: HWND) -> Scan {
    let mut monitors: Vec<MONITORINFO> = Vec::new();
    EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect_monitors), &mut monitors as *mut _ as LPARAM);
    let mut hwnds: Vec<HWND> = Vec::with_capacity(256);
    // EnumWindows yields top-level windows front-to-back.
    EnumWindows(Some(collect_windows), &mut hwnds as *mut _ as LPARAM);
    let rects: Vec<Rect> = monitors.iter().map(|m| rect(m.rcMonitor)).collect();
    let work: Vec<Rect> = monitors.iter().map(|m| rect(m.rcWork)).collect();
    let primary = monitors.iter().position(|m| m.dwFlags & MONITORINFOF_PRIMARY != 0).unwrap_or(0);
    let windows: Vec<desktop::Window> = hwnds
        .into_iter()
        .filter(|&h| h != own)
        .filter_map(|hwnd| {
            let r = rect(candidate_rect(hwnd)?);
            let captioned = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION == WS_CAPTION;
            let mut outer: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut outer);
            let fullscreen = desktop::fullscreen_on(&r, &rect(outer), &rects, captioned);
            Some(desktop::Window { id: hwnd as isize, rect: r, fullscreen })
        })
        .collect();
    desktop::build(rects, work, primary, windows)
}

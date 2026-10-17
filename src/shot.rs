//! "Ask about this page": captures the window the user was looking at (or their monitor),
//! scaled to what vision models use, encoded as PNG (JPEG if that's too big) with GDI+,
//! plus a little thumbnail for the chat tab. The slime's own windows are kept out of it.

use std::ffi::c_void;

use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmFlush, DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::GdiPlus::{
    EncoderParameter, EncoderParameters, GdipCreateBitmapFromScan0, GdipDisposeImage, GdipSaveImageToFile, GdiplusShutdown,
    GdiplusStartup, GdiplusStartupInput, GpBitmap, GpImage,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Longest side sent to the model (larger images are scaled down by the APIs anyway).
const MAX_EDGE: usize = 1568;
/// Encoded images above this fall back to JPEG (the APIs take about 5 MB as base64).
const MAX_BYTES: usize = 3_500_000;

/// What to capture: a screen rectangle and a name for it.
#[derive(Clone)]
pub struct Target {
    pub rect: RECT,
    pub label: String,
}

/// A captured image, ready to send.
pub struct Shot {
    pub label: String,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
    /// Opaque BGRA thumbnail and its size.
    pub thumb: Vec<u32>,
    pub thumb_w: usize,
    pub thumb_h: usize,
}

fn wide_text(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// The window the user is looking at, if it's a real, visible app window (not ours, the
/// desktop or the taskbar).
pub fn foreground_window(ours: &[HWND]) -> Option<Target> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        let hwnd = if root.is_null() { hwnd } else { root };
        if ours.contains(&hwnd) || IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return None;
        }
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut c_void, 4);
        if cloaked != 0 {
            return None;
        }
        let mut class = [0u16; 64];
        GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
        if matches!(wide_text(&class).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return None;
        }
        // The visible frame (GetWindowRect includes invisible resize borders).
        let mut r: RECT = std::mem::zeroed();
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, &mut r as *mut RECT as *mut c_void, 16) != 0 {
            GetWindowRect(hwnd, &mut r);
        }
        if r.right - r.left < 40 || r.bottom - r.top < 40 {
            return None;
        }
        let mut title = [0u16; 256];
        GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
        let label = wide_text(&title);
        Some(Target { rect: r, label: if label.trim().is_empty() { "this window".into() } else { label } })
    }
}

/// The whole monitor at `at`.
pub fn monitor_at(at: POINT) -> Target {
    unsafe {
        let mon = MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        Target { rect: mi.rcMonitor, label: "your screen".into() }
    }
}

/// Size to send: the rectangle scaled down to fit `MAX_EDGE`.
pub fn fit(w: usize, h: usize, max: usize) -> (usize, usize) {
    let k = (max as f32 / w.max(h) as f32).min(1.0);
    (((w as f32 * k).round() as usize).max(1), ((h as f32 * k).round() as usize).max(1))
}

/// Captures `target`, with a thumbnail fitting `thumb_max` (w, h). `ours` are kept out of
/// the picture (excluded from capture only for this moment, so they still show in the
/// user's own screenshots).
pub fn capture(target: &Target, ours: &[HWND], thumb_max: (usize, usize)) -> Result<Shot, String> {
    let r = clip_to_screen(target.rect);
    let (sw, sh) = ((r.right - r.left) as usize, (r.bottom - r.top) as usize);
    if sw < 8 || sh < 8 {
        return Err("that window is off screen".into());
    }
    let (w, h) = fit(sw, sh, MAX_EDGE);
    let pixels = unsafe {
        for &hwnd in ours {
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        }
        DwmFlush();
        let grabbed = grab(r, w, h);
        for &hwnd in ours {
            SetWindowDisplayAffinity(hwnd, WDA_NONE);
        }
        grabbed?
    };
    let (media_type, bytes) = encode(&pixels, w, h)?;
    let (thumb, thumb_w, thumb_h) = thumbnail(&pixels, w, h, thumb_max.0, thumb_max.1);
    Ok(Shot { label: target.label.clone(), media_type, bytes, thumb, thumb_w, thumb_h })
}

fn clip_to_screen(r: RECT) -> RECT {
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        RECT { left: r.left.max(x), top: r.top.max(y), right: r.right.min(x + w), bottom: r.bottom.min(y + h) }
    }
}

/// Copies screen rectangle `r` into a `w` x `h` opaque BGRA image (smoothly scaled).
unsafe fn grab(r: RECT, w: usize, h: usize) -> Result<Vec<u32>, String> {
    let screen = GetDC(std::ptr::null_mut());
    let dc = CreateCompatibleDC(screen);
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: -(h as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..std::mem::zeroed()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    let ok = !bmp.is_null() && {
        let old = SelectObject(dc, bmp);
        SetStretchBltMode(dc, HALFTONE);
        let ok = StretchBlt(
            dc,
            0,
            0,
            w as i32,
            h as i32,
            screen,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SRCCOPY | CAPTUREBLT,
        ) != 0;
        SelectObject(dc, old);
        ok
    };
    let out = ok.then(|| std::slice::from_raw_parts(bits as *const u32, w * h).iter().map(|p| p | 0xFF00_0000).collect());
    if !bmp.is_null() {
        DeleteObject(bmp);
    }
    DeleteDC(dc);
    ReleaseDC(std::ptr::null_mut(), screen);
    out.ok_or_else(|| "couldn't copy the screen".to_string())
}

const PNG: GUID = GUID::from_u128(0x557cf406_1a04_11d3_9a73_0000f81ef32e);
const JPEG: GUID = GUID::from_u128(0x557cf401_1a04_11d3_9a73_0000f81ef32e);
const QUALITY: GUID = GUID::from_u128(0x1d5be4b5_fa4a_452d_9cdd_5db35105e7eb);

/// Encodes opaque BGRA pixels as PNG, or JPEG if the PNG is too big to send.
pub fn encode(pixels: &[u32], w: usize, h: usize) -> Result<(&'static str, Vec<u8>), String> {
    let png = gdiplus_save(pixels, w, h, &PNG, None)?;
    if png.len() <= MAX_BYTES {
        return Ok(("image/png", png));
    }
    Ok(("image/jpeg", gdiplus_save(pixels, w, h, &JPEG, Some(85))?))
}

/// Saves through GDI+ to a temporary file and reads it back.
fn gdiplus_save(pixels: &[u32], w: usize, h: usize, encoder: &GUID, quality: Option<u32>) -> Result<Vec<u8>, String> {
    const PIXEL_FORMAT_32BPP_RGB: i32 = 0x0002_2009;
    unsafe {
        let mut token = 0usize;
        let input = GdiplusStartupInput { GdiplusVersion: 1, DebugEventCallback: 0, SuppressBackgroundThread: 0, SuppressExternalCodecs: 0 };
        if GdiplusStartup(&mut token, &input, std::ptr::null_mut()) != 0 {
            return Err("GDI+ didn't start".into());
        }
        let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
        let made = GdipCreateBitmapFromScan0(w as i32, h as i32, (w * 4) as i32, PIXEL_FORMAT_32BPP_RGB, pixels.as_ptr() as *const u8, &mut bitmap);
        let path = std::env::temp_dir().join(format!("slime-shot-{}-{}.img", std::process::id(), w * h));
        let path_w: Vec<u16> = path.as_os_str().encode_wide_nul();
        let mut q = quality.unwrap_or(0);
        let params = EncoderParameters {
            Count: 1,
            Parameter: [EncoderParameter { Guid: QUALITY, NumberOfValues: 1, Type: 4, Value: &mut q as *mut u32 as *mut c_void }],
        };
        let saved = made == 0
            && GdipSaveImageToFile(bitmap as *mut GpImage, path_w.as_ptr(), encoder, if quality.is_some() { &params } else { std::ptr::null() }) == 0;
        if !bitmap.is_null() {
            GdipDisposeImage(bitmap as *mut GpImage);
        }
        GdiplusShutdown(token);
        let bytes = if saved { std::fs::read(&path).ok() } else { None };
        let _ = std::fs::remove_file(&path);
        bytes.ok_or_else(|| "couldn't encode the screenshot".to_string())
    }
}

trait WideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl WideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

/// A box-filtered thumbnail of `pixels` (`w` x `h`) that fits `max_w` x `max_h`.
pub fn thumbnail(pixels: &[u32], w: usize, h: usize, max_w: usize, max_h: usize) -> (Vec<u32>, usize, usize) {
    let k = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(1.0);
    let (tw, th) = (((w as f32 * k) as usize).max(1), ((h as f32 * k) as usize).max(1));
    let mut out = vec![0u32; tw * th];
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let p = pixels[y * w + x];
                    r += p >> 16 & 255;
                    g += p >> 8 & 255;
                    b += p & 255;
                    n += 1;
                }
            }
            let n = n.max(1);
            out[ty * tw + tx] = 0xFF00_0000 | (r / n) << 16 | (g / n) << 8 | b / n;
        }
    }
    (out, tw, th)
}

/// Standard base64 (for the APIs' image fields).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_images_and_base64() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(fit(3840, 2160, MAX_EDGE), (1568, 882));
        assert_eq!(fit(800, 600, MAX_EDGE), (800, 600));
        // A small gradient survives the trip through GDI+ as a real PNG.
        let (w, h) = (64, 32);
        let px: Vec<u32> = (0..w * h).map(|i| 0xFF00_0000 | ((i % w) as u32 * 4) << 16 | ((i / w) as u32 * 8) << 8).collect();
        let (kind, bytes) = encode(&px, w, h).unwrap();
        assert_eq!(kind, "image/png");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let (t, tw, th) = thumbnail(&px, w, h, 16, 16);
        assert_eq!((tw, th, t.len()), (16, 8, 128));
    }
}

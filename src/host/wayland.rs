//! Linux (KDE Plasma on Wayland). Each monitor gets a transparent, click-through
//! layer-shell surface over everything; the slime's layers (goo, body, effects, bubble,
//! panel) are subsurfaces of it, moved about freely. Something crossing from one monitor
//! to another is shown on both (KWin clips each to its own monitor). Only the body's
//! pixels and the open panel take the mouse.
//!
//! What Wayland keeps from apps (other windows, the cursor away from our surfaces,
//! screenshots, the clipboard) comes from KWin over D-Bus (`kwin.rs`), and clicks on
//! other windows from `/dev/input` when that's readable (`evdev.rs`).

use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_subcompositor,
    output::{OutputHandler, OutputState},
    reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RepeatInfo},
        pointer::{cursor_shape::CursorShapeManager, PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer as ZLayer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
    subcompositor::SubcompositorState,
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_subsurface::WlSubsurface, wl_surface::WlSurface},
    Connection, EventQueue, QueueHandle,
};

use super::evdev::Clicks;
use super::kwin::{self, Kwin, Shoot};
use super::{tray, Event, Layer};
use crate::desktop::{self, Rect, Scan};
use crate::physics::{v2, V2};
use crate::shot;

/// Set by SIGTERM / SIGINT / SIGHUP: quit tidily (taking the KWin script and tray icon
/// with us).
static QUIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    QUIT.store(true, std::sync::atomic::Ordering::Relaxed);
}

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// One of our layers on one monitor.
struct Sub {
    surface: WlSurface,
    sub: WlSubsurface,
    pos: (i32, i32),
    shown: bool,
}

/// A monitor: its overlay surface and our layers on it.
struct Screen {
    output: wl_output::WlOutput,
    name: String,
    origin: (i32, i32),
    size: (i32, i32),
    root: LayerSurface,
    configured: bool,
    /// The overlay's own (fully transparent) buffer, and the pool it lives in.
    root_buffer: Option<(Buffer, SlotPool)>,
    subs: Vec<Sub>,
    /// Needs a commit for its layers' changes to show.
    dirty: bool,
}

/// What the mouse is over: a monitor's overlay, or one of our layers on it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Over {
    Root(usize),
    Sub(usize, Layer),
}

struct State {
    registry: RegistryState,
    outputs: OutputState,
    seat: SeatState,
    shm: Shm,
    compositor: CompositorState,
    subcompositor: SubcompositorState,
    layer_shell: LayerShell,
    cursor_shapes: Option<CursorShapeManager>,
    pool: SlotPool,
    screens: Vec<Screen>,
    /// Outputs that need a screen (made between dispatches).
    outputs_changed: bool,

    pointer: Option<wl_pointer::WlPointer>,
    shape_device: Option<WpCursorShapeDeviceV1>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    ctrl: bool,
    repeat: Option<RepeatInfo>,
    /// A held key repeating: what it types, and when next.
    held: Option<(u32, Vec<Event>, Instant)>,

    events: Vec<Event>,
    /// Where the mouse is (screen px), while it's over one of our surfaces.
    over: Option<(Over, V2)>,
    grabbing: bool,
    /// The monitor whose copy of the body is being dragged: it has to stay mapped (even
    /// off its screen) or the compositor ends the drag.
    grab_screen: Option<usize>,
    pinning: bool,
    panel_down: bool,
    panel_open: bool,
    /// Each layer's current rectangle on screen.
    rects: [Option<(i32, i32, usize, usize)>; 5],
    /// Hidden behind something fullscreen.
    tucked: bool,
}

impl State {
    fn find(&self, surface: &WlSurface) -> Option<Over> {
        for (i, s) in self.screens.iter().enumerate() {
            if s.root.wl_surface() == surface {
                return Some(Over::Root(i));
            }
            for (k, sub) in s.subs.iter().enumerate() {
                if &sub.surface == surface {
                    return Some(Over::Sub(i, Layer::ALL[k]));
                }
            }
        }
        None
    }

    /// Screen px of a point `p` on `over`.
    fn global(&self, over: Over, p: (f64, f64)) -> V2 {
        let (i, off) = match over {
            Over::Root(i) => (i, (0, 0)),
            Over::Sub(i, l) => (i, self.screens[i].subs[l.index()].pos),
        };
        let o = self.screens[i].origin;
        v2((o.0 + off.0) as f32 + p.0 as f32, (o.1 + off.1) as f32 + p.1 as f32)
    }

    fn make_screen(&mut self, qh: &QueueHandle<State>, output: wl_output::WlOutput) {
        let Some(info) = self.outputs.info(&output) else { return };
        let origin = info.logical_position.unwrap_or((0, 0));
        let size = info.logical_size.unwrap_or((1920, 1080));
        let name = info.name.clone().unwrap_or_default();
        let surface = self.compositor.create_surface(qh);
        let root = self.layer_shell.create_layer_surface(qh, surface, ZLayer::Overlay, Some("slime"), Some(&output));
        root.set_anchor(Anchor::all());
        root.set_exclusive_zone(-1);
        root.set_keyboard_interactivity(KeyboardInteractivity::None);
        if let Ok(r) = Region::new(&self.compositor) {
            root.wl_surface().set_input_region(Some(r.wl_region()));
        }
        // Bottom to top, in `Layer` order.
        let subs = Layer::ALL
            .iter()
            .map(|_| {
                let (sub, surface) = self.subcompositor.create_subsurface(root.wl_surface().clone(), qh);
                if let Ok(r) = Region::new(&self.compositor) {
                    surface.set_input_region(Some(r.wl_region()));
                }
                surface.commit();
                Sub { surface, sub, pos: (0, 0), shown: false }
            })
            .collect();
        root.commit();
        self.screens.push(Screen { output, name, origin, size, root, configured: false, root_buffer: None, subs, dirty: false });
    }

    fn set_cursor(&self, serial: u32, shape: Shape) {
        if let Some(d) = &self.shape_device {
            d.set_shape(serial, shape);
        }
    }

    /// While the panel's open, the overlays catch every click (one outside the panel
    /// closes it) and the panel's monitor takes the keyboard.
    fn panel_mode(&mut self, open: bool) {
        self.panel_open = open;
        let panel_screen = self.rects[Layer::Panel.index()].map(|(x, y, _, _)| {
            self.screens.iter().position(|s| contains(s, x + 1, y + 1)).unwrap_or(0)
        });
        for (i, s) in self.screens.iter_mut().enumerate() {
            if open {
                s.root.wl_surface().set_input_region(None);
            } else if let Ok(r) = Region::new(&self.compositor) {
                s.root.wl_surface().set_input_region(Some(r.wl_region()));
            }
            let keys = if open && panel_screen == Some(i) { KeyboardInteractivity::Exclusive } else { KeyboardInteractivity::None };
            s.root.set_keyboard_interactivity(keys);
            s.root.set_layer(if self.tucked && !open { ZLayer::Top } else { ZLayer::Overlay });
            s.dirty = true;
        }
        self.held = None;
    }

    fn key(&mut self, event: &KeyEvent) -> Vec<Event> {
        let k = event.keysym;
        if k == Keysym::Escape {
            return vec![Event::PanelClose];
        }
        if k == Keysym::Return || k == Keysym::KP_Enter {
            return vec![Event::Char(0x0d)];
        }
        if k == Keysym::BackSpace {
            return vec![Event::Char(0x08)];
        }
        if self.ctrl {
            return if k == Keysym::v || k == Keysym::V { vec![Event::Char(0x16)] } else { vec![] };
        }
        match &event.utf8 {
            Some(t) => t.chars().filter(|c| !c.is_control()).flat_map(|c| c.encode_utf16(&mut [0; 2]).to_vec()).map(Event::Char).collect(),
            None => vec![],
        }
    }
}

fn contains(s: &Screen, x: i32, y: i32) -> bool {
    x >= s.origin.0 && x < s.origin.0 + s.size.0 && y >= s.origin.1 && y < s.origin.1 + s.size.1
}

fn overlaps(s: &Screen, x: i32, y: i32, w: i32, h: i32) -> bool {
    x < s.origin.0 + s.size.0 && x + w > s.origin.0 && y < s.origin.1 + s.size.1 && y + h > s.origin.1
}

pub struct Host {
    conn: Connection,
    queue: EventQueue<State>,
    st: State,
    kwin: Option<Kwin>,
    _tray: Option<Box<dyn std::any::Any>>,
    tray_rx: Receiver<Event>,
    clicks: Option<Clicks>,
    /// The last cursor position we know of.
    last_cursor: V2,
    /// KWin's window ids by our numeric ones.
    uuids: std::collections::HashMap<isize, String>,
}

/// A window's numeric id from KWin's UUID.
fn window_id(uuid: &str) -> isize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in uuid.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    (h >> 1) as isize | 1
}

impl Host {
    pub fn new(icon: &[u32], icon_size: usize) -> Host {
        unsafe {
            for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                libc::signal(sig, on_signal as *const () as libc::sighandler_t);
            }
        }
        let conn = Connection::connect_to_env().unwrap_or_else(|e| fail(&format!("couldn't reach the Wayland compositor ({e}). Is this a Wayland session?")));
        let (globals, mut queue) = registry_queue_init::<State>(&conn).unwrap_or_else(|e| fail(&format!("Wayland: {e}")));
        let qh = queue.handle();
        let compositor = CompositorState::bind(&globals, &qh).unwrap_or_else(|_| fail("no wl_compositor"));
        let subcompositor =
            SubcompositorState::bind(compositor.wl_compositor().clone(), &globals, &qh).unwrap_or_else(|_| fail("no wl_subcompositor"));
        let layer_shell = LayerShell::bind(&globals, &qh)
            .unwrap_or_else(|_| fail("the compositor has no layer shell (needs KDE Plasma, or another wlroots-style compositor)"));
        let shm = Shm::bind(&globals, &qh).unwrap_or_else(|_| fail("no wl_shm"));
        let pool = SlotPool::new(4 << 20, &shm).unwrap_or_else(|e| fail(&format!("shared memory: {e}")));
        let mut st = State {
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &qh),
            seat: SeatState::new(&globals, &qh),
            shm,
            compositor,
            subcompositor,
            layer_shell,
            cursor_shapes: CursorShapeManager::bind(&globals, &qh).ok(),
            pool,
            screens: Vec::new(),
            outputs_changed: false,
            pointer: None,
            shape_device: None,
            keyboard: None,
            ctrl: false,
            repeat: None,
            held: None,
            events: Vec::new(),
            over: None,
            grabbing: false,
            grab_screen: None,
            pinning: false,
            panel_down: false,
            panel_open: false,
            rects: [None; 5],
            tucked: false,
        };
        // Learn the outputs (names and positions), then cover each.
        let _ = queue.roundtrip(&mut st);
        let _ = queue.roundtrip(&mut st);
        for o in st.outputs.outputs().collect::<Vec<_>>() {
            st.make_screen(&qh, o);
        }
        st.outputs_changed = false;
        let _ = queue.roundtrip(&mut st);

        let kwin = match Kwin::start() {
            Ok(k) => Some(k),
            Err(e) => {
                eprintln!("slime: {e}; he won't see your windows");
                None
            }
        };
        kwin::install_desktop_file(shot::encode(&unpremultiply(icon), icon_size, icon_size).ok().map(|(_, b)| b).as_deref());
        let (tx, tray_rx) = channel();
        let tray = tray::spawn(tx, icon, icon_size).map(|h| Box::new(h) as Box<dyn std::any::Any>);
        if tray.is_none() {
            eprintln!("slime: no system tray; right-click him for the panel");
        }
        let clicks = Clicks::open();
        let mut host = Host { conn, queue, st, kwin, _tray: tray, tray_rx, clicks, last_cursor: V2::ZERO, uuids: Default::default() };
        // Give the KWin script a moment to report in, so he starts on the right floor.
        let t = Instant::now();
        while host.desk_updates() == 0 && t.elapsed() < Duration::from_millis(1500) && host.kwin.is_some() {
            host.wait(Duration::from_millis(20));
        }
        host
    }

    fn desk_updates(&self) -> u64 {
        self.kwin.as_ref().and_then(|k| k.desk.lock().ok().map(|d| d.updates)).unwrap_or(0)
    }

    pub fn dpi(&self) -> f32 {
        1.0
    }

    /// Whether click-to-jump can work here (reading the mice needs the `input` group).
    pub fn sees_clicks(&self) -> bool {
        self.clicks.is_some()
    }

    pub fn events(&mut self) -> Vec<Event> {
        let _ = self.queue.dispatch_pending(&mut self.st);
        if self.st.outputs_changed {
            self.st.outputs_changed = false;
            self.rebuild_screens();
        }
        while let Ok(e) = self.tray_rx.try_recv() {
            self.st.events.push(e);
        }
        if QUIT.load(std::sync::atomic::Ordering::Relaxed) {
            self.st.events.push(Event::Quit);
        }
        // Key repeat.
        if let (Some((_, evs, next)), Some(RepeatInfo::Repeat { rate, .. })) = (&mut self.st.held, self.st.repeat) {
            let now = Instant::now();
            while *next <= now {
                self.st.events.extend(evs.iter().cloned());
                *next += Duration::from_secs_f32(1.0 / rate.get().max(1) as f32);
            }
        }
        if let Some(c) = &mut self.clicks {
            if c.presses() > 0 && self.st.over.is_none() {
                let at = self.cursor();
                self.st.events.push(Event::Click(at));
            }
        }
        std::mem::take(&mut self.st.events)
    }

    /// Outputs came or went: start over with a surface on each.
    fn rebuild_screens(&mut self) {
        let qh = self.queue.handle();
        for s in self.st.screens.drain(..) {
            for sub in &s.subs {
                sub.sub.destroy();
                sub.surface.destroy();
            }
            drop(s.root);
        }
        for o in self.st.outputs.outputs().collect::<Vec<_>>() {
            self.st.make_screen(&qh, o);
        }
        self.st.rects = [None; 5];
        self.st.over = None;
        let open = self.st.panel_open;
        self.st.panel_mode(open);
    }

    pub fn cursor(&mut self) -> V2 {
        if let Some((_, p)) = self.st.over {
            self.last_cursor = p;
        } else if let Some((x, y)) = self.kwin.as_ref().and_then(|k| k.desk.lock().ok().and_then(|d| d.cursor)) {
            self.last_cursor = v2(x as f32, y as f32);
        }
        self.last_cursor
    }

    pub fn scan(&mut self) -> Scan {
        let desk = self.kwin.as_ref().and_then(|k| k.desk.lock().ok().map(|d| (d.monitors.clone(), d.windows.clone())));
        let (monitors, windows) = match desk {
            Some((m, w)) if !m.is_empty() => (m, w),
            // KWin hasn't said: the outputs as Wayland describes them, with no windows.
            _ => {
                let m = self
                    .st
                    .screens
                    .iter()
                    .map(|s| {
                        let r = Rect { left: s.origin.0, top: s.origin.1, right: s.origin.0 + s.size.0, bottom: s.origin.1 + s.size.1 };
                        kwin::Monitor { name: s.name.clone(), rect: r, work: r }
                    })
                    .collect();
                (m, Vec::new())
            }
        };
        // The main monitor: a landscape one, leftmost first.
        let primary = (0..monitors.len())
            .min_by_key(|&i| {
                let r = monitors[i].rect;
                ((r.right - r.left) < (r.bottom - r.top), r.left, r.top)
            })
            .unwrap_or(0);
        self.uuids.clear();
        let wins: Vec<desktop::Window> = windows
            .iter()
            .filter(|w| w.rect.right - w.rect.left >= 60 && w.rect.bottom - w.rect.top >= 30)
            .map(|w| {
                let id = window_id(&w.uuid);
                self.uuids.insert(id, w.uuid.clone());
                let fullscreen = if w.fullscreen { monitors.iter().position(|m| m.name == w.output) } else { None };
                desktop::Window { id, rect: w.rect, fullscreen }
            })
            .collect();
        desktop::build(monitors.iter().map(|m| m.rect).collect(), monitors.iter().map(|m| m.work).collect(), primary, wins)
    }

    pub fn show(&mut self, layer: Layer, x: i32, y: i32, w: usize, h: usize, px: &[u32], stride: usize) {
        if w == 0 || h == 0 {
            return self.hide(layer);
        }
        let Ok((buffer, canvas)) = self.st.pool.create_buffer(w as i32, h as i32, w as i32 * 4, wl_shm::Format::Argb8888) else {
            return;
        };
        // (Premultiplied 0xAARRGGBB is exactly wl_shm's ARGB8888.)
        match unsafe { canvas.align_to_mut::<u32>() } {
            (&mut [], dst, &mut []) => {
                for row in 0..h {
                    dst[row * w..row * w + w].copy_from_slice(&px[row * stride..row * stride + w]);
                }
            }
            _ => {
                for row in 0..h {
                    for (d, s) in canvas[row * w * 4..(row + 1) * w * 4].chunks_exact_mut(4).zip(&px[row * stride..row * stride + w]) {
                        d.copy_from_slice(&s.to_le_bytes());
                    }
                }
            }
        }
        // Only the body's own pixels (and the whole panel) catch the mouse.
        let region = match layer {
            Layer::Body | Layer::Panel => Region::new(&self.st.compositor).ok().inspect(|r| {
                if layer == Layer::Panel {
                    r.add(0, 0, w as i32, h as i32);
                    return;
                }
                for row in 0..h {
                    let line = &px[row * stride..row * stride + w];
                    let mut x0 = None;
                    for (x, &p) in line.iter().chain(std::iter::once(&0)).enumerate() {
                        match (p >> 24 > 0, x0) {
                            (true, None) => x0 = Some(x),
                            (false, Some(s)) => {
                                r.add(s as i32, row as i32, (x - s) as i32, 1);
                                x0 = None;
                            }
                            _ => {}
                        }
                    }
                }
            }),
            _ => None,
        };
        let mut attached = false;
        let i = layer.index();
        let held = if layer == Layer::Body && self.st.grabbing { self.st.grab_screen } else { None };
        for (k, s) in self.st.screens.iter_mut().enumerate() {
            let on = overlaps(s, x, y, w as i32, h as i32) || held == Some(k);
            let sub = &mut s.subs[i];
            if on {
                if attached {
                    sub.surface.attach(Some(buffer.wl_buffer()), 0, 0);
                } else if buffer.attach_to(&sub.surface).is_err() {
                    continue;
                }
                attached = true;
                sub.surface.damage_buffer(0, 0, w as i32, h as i32);
                if let Some(r) = &region {
                    sub.surface.set_input_region(Some(r.wl_region()));
                }
                sub.surface.commit();
                let pos = (x - s.origin.0, y - s.origin.1);
                if pos != sub.pos || !sub.shown {
                    sub.sub.set_position(pos.0, pos.1);
                    sub.pos = pos;
                }
                sub.shown = true;
                s.dirty = true;
            } else if sub.shown {
                sub.surface.attach(None, 0, 0);
                sub.surface.commit();
                sub.shown = false;
                s.dirty = true;
            }
        }
        self.st.rects[i] = Some((x, y, w, h));
    }

    pub fn hide(&mut self, layer: Layer) {
        let i = layer.index();
        for s in &mut self.st.screens {
            let sub = &mut s.subs[i];
            if sub.shown {
                sub.surface.attach(None, 0, 0);
                sub.surface.commit();
                sub.shown = false;
                s.dirty = true;
            }
        }
        if layer == Layer::Panel && self.st.panel_open {
            self.st.panel_mode(false);
            self.st.panel_down = false;
            if matches!(self.st.over, Some((Over::Sub(_, Layer::Panel), _)) | Some((Over::Root(_), _))) {
                self.st.over = None;
            }
        }
        self.st.rects[i] = None;
    }

    /// The panel's up: it takes the keyboard, and a click anywhere else closes it.
    pub fn focus_panel(&mut self) {
        self.st.panel_mode(true);
    }

    /// Tucks the slime behind something fullscreen (into the layer under fullscreen
    /// windows), or back on top of everything.
    pub fn hide_behind(&mut self, fs: Option<isize>) {
        self.st.tucked = fs.is_some();
        let open = self.st.panel_open;
        for s in &mut self.st.screens {
            s.root.set_layer(if self.st.tucked && !open { ZLayer::Top } else { ZLayer::Overlay });
            s.dirty = true;
        }
    }

    pub fn clipboard(&self) -> Option<String> {
        self.kwin.as_ref()?.clipboard()
    }

    /// The window the user is looking at (KWin's active window).
    pub fn shot_target(&self) -> Option<shot::Target> {
        let desk = self.kwin.as_ref()?.desk.lock().ok()?;
        let uuid = desk.active.clone()?;
        let w = desk.windows.iter().find(|w| w.uuid == uuid)?;
        if w.rect.right - w.rect.left < 40 || w.rect.bottom - w.rect.top < 40 {
            return None;
        }
        let label = if w.caption.trim().is_empty() { "this window".to_string() } else { w.caption.clone() };
        Some(shot::Target { rect: w.rect, label, window: Some(uuid) })
    }

    /// The whole monitor at `at`.
    pub fn monitor_target(&mut self, at: V2) -> shot::Target {
        let scan = self.scan();
        let m = scan.monitor_near(at.x, at.y).unwrap_or(0);
        let rect = scan.monitors.get(m).copied().unwrap_or_default();
        shot::Target { rect, label: "your screen".into(), window: None }
    }

    pub fn capture(&mut self, target: &shot::Target, thumb_max: (usize, usize)) -> Result<shot::Shot, String> {
        let kwin = self.kwin.as_ref().ok_or("can't reach KWin")?;
        let (px, w, h) = match &target.window {
            // Just that window: nothing of ours in it.
            Some(uuid) => kwin.screenshot(Shoot::Window(uuid.clone()))?,
            None => {
                // The whole screen: take the slime's layers down for a moment.
                for l in Layer::ALL {
                    self.hide_keep(l);
                }
                self.commit();
                let _ = self.queue.roundtrip(&mut self.st);
                std::thread::sleep(Duration::from_millis(80));
                let kwin = self.kwin.as_ref().ok_or("can't reach KWin")?;
                kwin.screenshot(Shoot::Area(target.rect))?
            }
        };
        shot::finish(&px, w, h, &target.label, thumb_max)
    }

    /// Takes a layer off screen without forgetting it's meant to be up (it's back on the
    /// next frame).
    fn hide_keep(&mut self, layer: Layer) {
        let i = layer.index();
        for s in &mut self.st.screens {
            let sub = &mut s.subs[i];
            if sub.shown {
                sub.surface.attach(None, 0, 0);
                sub.surface.commit();
                sub.shown = false;
                s.dirty = true;
            }
        }
    }

    /// Shows this frame's changes.
    fn commit(&mut self) {
        for s in &mut self.st.screens {
            if s.dirty && s.configured {
                s.root.commit();
                s.dirty = false;
            }
        }
        let _ = self.conn.flush();
    }

    /// Shows this frame's changes, then sleeps up to `d`, waking early for input.
    pub fn wait(&mut self, d: Duration) {
        self.commit();
        let _ = self.queue.flush();
        let _ = self.queue.dispatch_pending(&mut self.st);
        if let Some(guard) = self.queue.prepare_read() {
            use std::os::fd::AsRawFd;
            let mut fds = [libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, d.as_millis() as i32) };
            if ready > 0 {
                let _ = guard.read();
            }
        } else {
            std::thread::sleep(d.min(Duration::from_millis(1)));
        }
        let _ = self.queue.dispatch_pending(&mut self.st);
    }
}

fn unpremultiply(px: &[u32]) -> Vec<u32> {
    px.iter()
        .map(|&p| {
            let a = p >> 24;
            let un = |c: u32| if a == 0 { 0 } else { (c * 255 / a).min(255) };
            (a << 24) | (un((p >> 16) & 255) << 16) | (un((p >> 8) & 255) << 8) | un(p & 255)
        })
        .collect()
}

fn fail(msg: &str) -> ! {
    eprintln!("slime: {msg}");
    std::process::exit(2)
}

// --- Wayland plumbing ---

impl CompositorHandler for State {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.outputs_changed = true;
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // Moved or resized: keep its overlay where it is now.
        let info = self.outputs.info(&output);
        if let (Some(s), Some(info)) = (self.screens.iter_mut().find(|s| s.output == output), info) {
            s.origin = info.logical_position.unwrap_or(s.origin);
        }
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.outputs_changed = true;
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.outputs_changed = true;
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, c: LayerSurfaceConfigure, _: u32) {
        let Some(s) = self.screens.iter_mut().find(|s| &s.root == layer) else { return };
        let (w, h) = (c.new_size.0.max(1) as i32, c.new_size.1.max(1) as i32);
        if s.configured && s.size == (w, h) {
            return;
        }
        s.size = (w, h);
        s.configured = true;
        // A fully transparent buffer the size of the monitor (fresh shared memory is all
        // zeros), so the overlay is mapped and our layers can show on it.
        if let Ok(mut pool) = SlotPool::new((w * h * 4) as usize, &self.shm) {
            if let Ok((buffer, _)) = pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888) {
                let _ = buffer.attach_to(s.root.wl_surface());
                s.root.wl_surface().damage_buffer(0, 0, w, h);
                s.root_buffer = Some((buffer, pool));
            }
        }
        s.root.commit();
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Pointer && self.pointer.is_none() {
            if let Ok(p) = self.seat.get_pointer(qh, &seat) {
                self.shape_device = self.cursor_shapes.as_ref().map(|m| m.get_shape_device(&p, qh));
                self.pointer = Some(p);
            }
        }
        if cap == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat.get_keyboard(qh, &seat, None).ok();
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
            self.shape_device = None;
        }
        if cap == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for State {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for e in events {
            let Some(over) = self.find(&e.surface) else { continue };
            let at = self.global(over, e.position);
            match e.kind {
                PointerEventKind::Enter { serial } => {
                    self.over = Some((over, at));
                    self.set_cursor(serial, if matches!(over, Over::Sub(_, Layer::Body)) { Shape::Pointer } else { Shape::Default });
                }
                PointerEventKind::Leave { .. } => {
                    if self.over.is_some_and(|(o, _)| o == over) {
                        self.over = None;
                    }
                    // (A drag the compositor took away.)
                    if self.grabbing {
                        self.grabbing = false;
                        self.events.push(Event::GrabEnd);
                    }
                }
                PointerEventKind::Motion { .. } => {
                    self.over = Some((over, at));
                    if self.panel_down {
                        if let Some((px, _, _, _)) = self.rects[Layer::Panel.index()] {
                            self.events.push(Event::PanelDrag(at.x as i32 - px));
                        }
                    }
                }
                PointerEventKind::Press { button, .. } => {
                    self.over = Some((over, at));
                    match (over, button) {
                        (Over::Sub(i, Layer::Body), BTN_LEFT) => {
                            self.grabbing = true;
                            self.grab_screen = Some(i);
                            self.events.push(Event::GrabStart);
                        }
                        (_, BTN_RIGHT) if self.grabbing => {
                            self.pinning = true;
                            self.events.push(Event::Pin);
                        }
                        (Over::Sub(_, Layer::Panel), BTN_LEFT) => {
                            if let Some((px, py, _, _)) = self.rects[Layer::Panel.index()] {
                                self.panel_down = true;
                                self.events.push(Event::PanelDown(at.x as i32 - px, at.y as i32 - py));
                            }
                        }
                        (Over::Sub(_, Layer::Panel), _) => {}
                        // A click outside the open panel closes it.
                        (Over::Root(_), _) if self.panel_open => self.events.push(Event::PanelClose),
                        _ => {}
                    }
                }
                PointerEventKind::Release { button, .. } => match button {
                    BTN_LEFT if self.grabbing => {
                        self.grabbing = false;
                        self.events.push(Event::GrabEnd);
                    }
                    BTN_LEFT if self.panel_down => {
                        self.panel_down = false;
                        self.events.push(Event::PanelUp);
                    }
                    BTN_RIGHT if self.pinning => self.pinning = false,
                    BTN_RIGHT if matches!(over, Over::Sub(_, Layer::Body)) => self.events.push(Event::Menu),
                    _ => {}
                },
                PointerEventKind::Axis { .. } => {}
            }
        }
    }
}

impl KeyboardHandler for State {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &WlSurface, _: u32) {
        self.held = None;
    }
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        if !self.panel_open {
            return;
        }
        let evs = self.key(&event);
        self.events.extend(evs.iter().cloned());
        let once = [Event::PanelClose, Event::Char(0x0d), Event::Char(0x16)];
        let repeats = !evs.is_empty() && !evs.iter().any(|e| once.contains(e));
        self.held = match self.repeat {
            Some(RepeatInfo::Repeat { delay, .. }) if repeats => {
                Some((event.raw_code, evs, Instant::now() + Duration::from_millis(delay as u64)))
            }
            _ => None,
        };
    }
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        if self.held.as_ref().is_some_and(|h| h.0 == event.raw_code) {
            self.held = None;
        }
    }
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, m: Modifiers, _: u32) {
        self.ctrl = m.ctrl;
    }
    fn update_repeat_info(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, info: RepeatInfo) {
        self.repeat = Some(info);
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(State);
delegate_subcompositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_keyboard!(State);
delegate_pointer!(State);
delegate_layer!(State);
delegate_registry!(State);

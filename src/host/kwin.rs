//! Talking to KDE Plasma over D-Bus. Wayland doesn't let an app see other windows or
//! where the mouse is, so a small KWin script (loaded while the slime runs) sends us the
//! monitors, the windows in stacking order and the cursor position whenever they change.
//! Also: screenshots through KWin's ScreenShot2 interface, the clipboard through Klipper.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use zbus::blocking::{connection, Connection};
use zbus::zvariant::{Fd, OwnedValue};

use crate::desktop::Rect;

const SERVICE: &str = "org.slimecompanion.Slime";
const PATH: &str = "/Slime";
/// The script's plugin name in KWin (fixed, so a crashed run's script is replaced).
const PLUGIN: &str = "slime-companion";

/// A monitor: its name, whole area and work area (without panels).
#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub rect: Rect,
    pub work: Rect,
}

/// A window, as KWin sees it.
#[derive(Clone, Debug)]
pub struct Win {
    /// KWin's internal id (a UUID), for screenshots.
    pub uuid: String,
    pub rect: Rect,
    pub fullscreen: bool,
    /// The monitor it's on.
    pub output: String,
    pub caption: String,
}

/// The latest from the script.
#[derive(Default)]
pub struct Desk {
    pub monitors: Vec<Monitor>,
    /// Front to back.
    pub windows: Vec<Win>,
    pub active: Option<String>,
    pub cursor: Option<(i32, i32)>,
    /// Updates so far (0: the script hasn't reported in yet).
    pub updates: u64,
}

struct Listener {
    desk: Arc<Mutex<Desk>>,
}

fn rect(v: &Value, at: usize) -> Rect {
    let n = |i: usize| v[at + i].as_f64().unwrap_or(0.0).round() as i32;
    Rect { left: n(0), top: n(1), right: n(0) + n(2), bottom: n(1) + n(3) }
}

#[zbus::interface(name = "org.slimecompanion.Slime")]
impl Listener {
    /// The cursor moved: "x,y".
    fn cursor(&self, at: String) {
        let mut it = at.split(',').filter_map(|s| s.trim().parse::<f64>().ok());
        if let (Some(x), Some(y)) = (it.next(), it.next()) {
            if let Ok(mut d) = self.desk.lock() {
                d.cursor = Some((x.round() as i32, y.round() as i32));
            }
        }
    }

    /// The desktop changed: JSON with the monitors (`m`), windows front to back (`w`) and
    /// the active window (`a`).
    fn desktop(&self, json: String) {
        let Ok(v) = serde_json::from_str::<Value>(&json) else { return };
        let monitors = v["m"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|m| Monitor { name: m[0].as_str().unwrap_or("").to_string(), rect: rect(m, 1), work: rect(m, 5) })
                    .collect()
            })
            .unwrap_or_default();
        let windows = v["w"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|w| Win {
                        uuid: w[0].as_str().unwrap_or("").to_string(),
                        rect: rect(w, 1),
                        fullscreen: w[5].as_i64() == Some(1),
                        output: w[6].as_str().unwrap_or("").to_string(),
                        caption: w[7].as_str().unwrap_or("").to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let active = v["a"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        if let Ok(mut d) = self.desk.lock() {
            d.monitors = monitors;
            d.windows = windows;
            d.active = active;
            d.updates += 1;
        }
    }
}

/// Polls 60 times a second and sends anything that changed. Front to back; only windows
/// the slime could stand on (normal windows and dialogs on this desktop and activity).
const SCRIPT: &str = r#"
var SERVICE = "org.slimecompanion.Slime", PATH = "/Slime", IFACE = "org.slimecompanion.Slime";
var OWN_PID = %PID%;
var lastDesk = "", lastCursor = "";
function onHere(w, desk) {
    if (!w.onAllDesktops) {
        var ok = false;
        for (var i = 0; i < w.desktops.length; i++) if (w.desktops[i].id === desk.id) ok = true;
        if (!ok) return false;
    }
    var acts = w.activities;
    if (acts && acts.length > 0 && acts.indexOf(workspace.currentActivity) < 0) return false;
    return true;
}
function snapshot() {
    var desk = workspace.currentDesktop;
    var mons = [];
    var screens = workspace.screens;
    for (var i = 0; i < screens.length; i++) {
        var s = screens[i], g = s.geometry, a = workspace.clientArea(KWin.MaximizeArea, s, desk);
        mons.push([s.name, g.x, g.y, g.width, g.height, a.x, a.y, a.width, a.height]);
    }
    var wins = [];
    var order = workspace.stackingOrder;
    for (var j = order.length - 1; j >= 0; j--) {
        var w = order[j];
        if (!w || w.deleted || !(w.normalWindow || w.dialog)) continue;
        if (w.minimized || w.hidden || w.pid === OWN_PID) continue;
        if (!onHere(w, desk)) continue;
        var f = w.frameGeometry;
        wins.push([String(w.internalId), f.x, f.y, f.width, f.height, w.fullScreen ? 1 : 0, w.output ? w.output.name : "", w.caption]);
    }
    var act = workspace.activeWindow;
    return JSON.stringify({ m: mons, w: wins, a: act && act.pid !== OWN_PID ? String(act.internalId) : "" });
}
var timer = new QTimer();
timer.interval = 16;
timer.timeout.connect(function () {
    var c = workspace.cursorPos, cs = c.x + "," + c.y;
    if (cs !== lastCursor) {
        lastCursor = cs;
        callDBus(SERVICE, PATH, IFACE, "Cursor", cs);
    }
    var d = snapshot();
    if (d !== lastDesk) {
        lastDesk = d;
        callDBus(SERVICE, PATH, IFACE, "Desktop", d);
    }
});
timer.start();
"#;

/// Our D-Bus connection, with the KWin script running.
pub struct Kwin {
    conn: Connection,
    pub desk: Arc<Mutex<Desk>>,
    script: Option<PathBuf>,
}

impl Kwin {
    pub fn start() -> Result<Kwin, String> {
        let desk = Arc::new(Mutex::new(Desk::default()));
        let conn = connection::Builder::session()
            .and_then(|b| b.name(SERVICE))
            .and_then(|b| b.serve_at(PATH, Listener { desk: desk.clone() }))
            .and_then(|b| b.build())
            .map_err(|e| format!("D-Bus: {e}"))?;
        let mut k = Kwin { conn, desk, script: None };
        if let Err(e) = k.load_script() {
            eprintln!("slime: couldn't load the KWin script ({e}); he won't see your windows");
        }
        Ok(k)
    }

    fn load_script(&mut self) -> Result<(), String> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("slime-companion-{}.js", std::process::id()));
        std::fs::write(&path, SCRIPT.replace("%PID%", &std::process::id().to_string())).map_err(|e| e.to_string())?;
        self.script = Some(path.clone());
        // A script left behind by a run that crashed.
        let _ = self.call("/Scripting", "org.kde.kwin.Scripting", "unloadScript", &(PLUGIN,));
        let reply = self.call("/Scripting", "org.kde.kwin.Scripting", "loadScript", &(path.to_string_lossy().into_owned(), PLUGIN))?;
        let id: i32 = reply.body().deserialize().map_err(|e| e.to_string())?;
        if id < 0 {
            return Err("KWin refused the script".into());
        }
        self.call(&format!("/Scripting/Script{id}"), "org.kde.kwin.Script", "run", &())?;
        Ok(())
    }

    fn call<B>(&self, path: &str, iface: &str, method: &str, body: &B) -> Result<zbus::Message, String>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn.call_method(Some("org.kde.KWin"), path, Some(iface), method, body).map_err(|e| e.to_string())
    }

    /// Text on the clipboard (through Klipper).
    pub fn clipboard(&self) -> Option<String> {
        let reply = self
            .conn
            .call_method(Some("org.kde.klipper"), "/klipper", Some("org.kde.klipper.klipper"), "getClipboardContents", &())
            .ok()?;
        reply.body().deserialize::<String>().ok()
    }

    /// A screenshot of KWin window `uuid`, or of a screen area: opaque BGRA pixels and size.
    pub fn screenshot(&self, what: Shoot) -> Result<(Vec<u32>, usize, usize), String> {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err("no pipe".into());
        }
        let (mut read, write) = unsafe { (std::fs::File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let mut options: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
        options.insert("include-cursor", false.into());
        options.insert("include-decoration", true.into());
        options.insert("include-shadow", false.into());
        options.insert("native-resolution", true.into());
        let (dest, path, iface) = (Some("org.kde.KWin"), "/org/kde/KWin/ScreenShot2", Some("org.kde.KWin.ScreenShot2"));
        let fd = Fd::from(&write);
        let reply = match &what {
            Shoot::Window(uuid) => self.conn.call_method(dest, path, iface, "CaptureWindow", &(uuid.as_str(), &options, fd)),
            Shoot::Area(r) => self.conn.call_method(
                dest,
                path,
                iface,
                "CaptureArea",
                &(r.left, r.top, (r.right - r.left) as u32, (r.bottom - r.top) as u32, &options, fd),
            ),
        };
        drop(write);
        let reply = reply.map_err(|e| match e.to_string() {
            s if s.contains("NoAuthorized") || s.contains("not authorized") || s.contains("NotAuthorized") => {
                "KWin didn't allow it (the slime's .desktop file may be missing; run it once more)".to_string()
            }
            s => format!("KWin said: {s}"),
        })?;
        let info: HashMap<String, OwnedValue> = reply.body().deserialize().map_err(|e| e.to_string())?;
        let num = |k: &str| info.get(k).and_then(|v| u32::try_from(v).ok()).unwrap_or(0) as usize;
        let (w, h, stride, format) = (num("width"), num("height"), num("stride"), num("format"));
        let mut data = Vec::new();
        read.read_to_end(&mut data).map_err(|e| e.to_string())?;
        if w == 0 || h == 0 || data.len() < stride * h {
            return Err("the screenshot came back empty".into());
        }
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let row = &data[y * stride..y * stride + w * 4];
            for c in row.chunks_exact(4) {
                let p = match format {
                    // QImage RGBX8888 / RGBA8888 (premultiplied or not): bytes R, G, B, A.
                    16..=18 => (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32,
                    // RGB32 / ARGB32 (premultiplied or not): a native-endian 0xAARRGGBB.
                    _ => u32::from_le_bytes([c[0], c[1], c[2], c[3]]) & 0xFF_FFFF,
                };
                px.push(p | 0xFF00_0000);
            }
        }
        Ok((px, w, h))
    }
}

/// What to screenshot.
pub enum Shoot {
    Window(String),
    Area(Rect),
}

impl Drop for Kwin {
    fn drop(&mut self) {
        let _ = self.call("/Scripting", "org.kde.kwin.Scripting", "unloadScript", &(PLUGIN,));
        if let Some(p) = &self.script {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Makes sure Plasma knows the slime: a launcher in the app menu, which also lets KWin
/// hand him screenshots (KWin only gives them to apps whose .desktop file asks).
pub fn install_desktop_file(icon_png: Option<&[u8]>) {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let apps = data.join("applications");
    let icon = data.join("icons/hicolor/32x32/apps/slime-companion.png");
    if let Some(png) = icon_png {
        if !icon.exists() {
            let _ = std::fs::create_dir_all(icon.parent().unwrap());
            let _ = std::fs::write(&icon, png);
        }
    }
    let file = apps.join("slime-companion.desktop");
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Slime\nComment=A gooey pixel-art slime that lives on your desktop\n\
         Exec={}\nIcon=slime-companion\nTerminal=false\nCategories=Amusement;\n\
         X-KDE-DBUS-Restricted-Interfaces=org.kde.KWin.ScreenShot2\n",
        exe.display()
    );
    if std::fs::read_to_string(&file).ok().as_deref() == Some(text.as_str()) {
        return;
    }
    let _ = std::fs::create_dir_all(&apps);
    if std::fs::write(&file, text).is_ok() {
        // So KWin sees it now rather than at the next login.
        let _ = std::process::Command::new("kbuildsycoca6")
            .arg("--noincremental")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

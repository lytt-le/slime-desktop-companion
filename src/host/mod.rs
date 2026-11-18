//! The platform layer: the slime's on-screen windows, the mouse and keyboard, the tray
//! icon, the clipboard, what's on the desktop and screenshots. Everything else (the sim,
//! drawing, the panel, chat) is the same on every platform and talks to the desktop only
//! through `Host`.
//!
//! - Windows: raw Win32 layered windows (`windows.rs`).
//! - Linux: Wayland layer-shell surfaces, with a KWin script reporting the windows and the
//!   cursor (`wayland.rs`, `kwin.rs`, `tray.rs`). Needs KDE Plasma 6.

use crate::physics::V2;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{rect, Host};
#[cfg(all(windows, test))]
pub use self::windows::{scan, tuck_behind, wide};

#[cfg(target_os = "linux")]
mod evdev;
#[cfg(target_os = "linux")]
mod kwin;
#[cfg(target_os = "linux")]
mod tray;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
pub use self::wayland::Host;

/// What happened since the last frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    GrabStart,
    GrabEnd,
    /// Right-clicked while held: pin him there.
    Pin,
    /// Open the settings panel (at the cursor).
    Menu,
    Reset,
    /// A left click anywhere on the desktop (screen px).
    Click(V2),
    /// A character typed on the panel (UTF-16 unit, including control codes: 0x08
    /// backspace, 0x0d enter, 0x16 paste).
    Char(u16),
    /// Mouse on the settings panel, in pixels from its corner (drags only need x).
    PanelDown(i32, i32),
    PanelDrag(i32),
    PanelUp,
    /// The panel lost focus or Esc was pressed.
    PanelClose,
    /// The app is being closed from outside (tray, session ending).
    Quit,
}

/// The slime's see-through windows, bottom to top.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    /// Goo left on the desktop (click-through).
    Goo,
    /// The slime himself (only his pixels catch the mouse).
    Body,
    /// Z's, the cigarette, the pin (click-through).
    Fx,
    /// The speech bubble (click-through).
    Bubble,
    /// The settings panel (takes the keyboard while open).
    Panel,
}

impl Layer {
    #[cfg_attr(windows, allow(dead_code))]
    pub const ALL: [Layer; 5] = [Layer::Goo, Layer::Body, Layer::Fx, Layer::Bubble, Layer::Panel];

    pub fn index(self) -> usize {
        self as usize
    }
}

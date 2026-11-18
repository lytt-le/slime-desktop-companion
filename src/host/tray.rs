//! The tray icon (a StatusNotifierItem, which Plasma shows in its system tray). Clicking
//! it opens the panel at the cursor; its menu also has Reset and Quit.

use std::sync::mpsc::Sender;

use ksni::blocking::{Handle, TrayMethods};

use super::Event;

struct Tray {
    tx: Sender<Event>,
    icon: ksni::Icon,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "slime-companion".into()
    }

    fn title(&self) -> String {
        "Slime".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "Slime".into(), description: "Click for chat and settings".into(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(Event::Menu);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let item = |label: &str, e: Event| -> ksni::MenuItem<Self> {
            StandardItem {
                label: label.into(),
                activate: Box::new(move |t: &mut Self| {
                    let _ = t.tx.send(e.clone());
                }),
                ..Default::default()
            }
            .into()
        };
        vec![item("Chat and settings", Event::Menu), item("Reset slime", Event::Reset), ksni::MenuItem::Separator, item("Quit", Event::Quit)]
    }
}

/// Puts the icon (premultiplied BGRA, `size` square) in the tray. `None` if there's no tray.
pub fn spawn(tx: Sender<Event>, px: &[u32], size: usize) -> Option<Handle<impl ksni::Tray>> {
    // SNI wants straight (not premultiplied) ARGB, big-endian.
    let mut data = Vec::with_capacity(size * size * 4);
    for &p in px.iter().take(size * size) {
        let a = p >> 24;
        let un = |c: u32| if a == 0 { 0 } else { (c * 255 / a).min(255) } as u8;
        data.extend_from_slice(&[a as u8, un((p >> 16) & 255), un((p >> 8) & 255), un(p & 255)]);
    }
    let icon = ksni::Icon { width: size as i32, height: size as i32, data };
    Tray { tx, icon }.spawn().ok()
}

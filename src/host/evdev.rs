//! Click-to-jump on Wayland: apps can't see clicks on other windows, so this reads the
//! mice directly from `/dev/input`. That needs the user in the `input` group; without it
//! there are no clicks to jump to (everything else still works).

use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

const EV_KEY: u16 = 1;
const BTN_LEFT: u16 = 0x110;

pub struct Clicks {
    mice: Vec<File>,
}

/// Whether the device has a left mouse button.
fn has_left_button(f: &File) -> bool {
    let mut bits = [0u8; 96];
    // EVIOCGBIT(EV_KEY, len): _IOC(_IOC_READ, 'E', 0x20 + EV_KEY, len)
    let req = (2u64 << 30) | ((bits.len() as u64) << 16) | (0x45 << 8) | (0x20 + EV_KEY as u64);
    let n = unsafe { libc::ioctl(f.as_raw_fd(), req as _, bits.as_mut_ptr()) };
    n > 0 && bits[(BTN_LEFT / 8) as usize] & (1 << (BTN_LEFT % 8)) != 0
}

impl Clicks {
    /// The mice we can read, if any.
    pub fn open() -> Option<Clicks> {
        let mice: Vec<File> = std::fs::read_dir("/dev/input")
            .ok()?
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("event"))
            .filter_map(|e| std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(e.path()).ok())
            .filter(has_left_button)
            .collect();
        (!mice.is_empty()).then_some(Clicks { mice })
    }

    /// How many left-button presses since the last call.
    pub fn presses(&mut self) -> usize {
        let mut n = 0;
        let mut buf = [0u8; 24 * 64];
        for f in &mut self.mice {
            while let Ok(got) = f.read(&mut buf) {
                if got == 0 {
                    break;
                }
                // struct input_event { timeval (16), u16 type, u16 code, i32 value }
                for ev in buf[..got].chunks_exact(24) {
                    let kind = u16::from_ne_bytes([ev[16], ev[17]]);
                    let code = u16::from_ne_bytes([ev[18], ev[19]]);
                    let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                    if kind == EV_KEY && code == BTN_LEFT && value == 1 {
                        n += 1;
                    }
                }
            }
        }
        n
    }
}

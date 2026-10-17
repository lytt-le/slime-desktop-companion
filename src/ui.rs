//! Pixel-art speech bubble and settings panel, drawn with the 5x7 font in chunky
//! pixels (`p` screen pixels per font pixel) into premultiplied BGRA buffers.

use super::font::{pixel, ADVANCE, GLYPH_H, GLYPH_W};

/// Characters per bubble line, and bubble lines per page.
pub const BUBBLE_COLS: usize = 26;
pub const BUBBLE_ROWS: usize = 6;

const PAD: usize = 4; // font pixels
const LINE_GAP: usize = 2;
const TAIL: usize = 5;

type Rgb = (u8, u8, u8);
const FILL: Rgb = (244, 252, 236);
const BORDER: Rgb = (28, 84, 40);
const TEXT: Rgb = (30, 48, 32);
const HINT: Rgb = (140, 160, 140);
const PROMPT: Rgb = (60, 170, 70);

fn px(c: Rgb, a: f32) -> u32 {
    let a8 = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let m = |v: u8| (v as u32 * a8 + 127) / 255;
    (a8 << 24) | (m(c.0) << 16) | (m(c.1) << 8) | m(c.2)
}

struct Canvas<'a> {
    buf: &'a mut [u32],
    stride: usize,
    w: usize,
    h: usize,
    p: usize,
}

impl Canvas<'_> {
    fn clear(&mut self) {
        for y in 0..self.h {
            self.buf[y * self.stride..y * self.stride + self.w].fill(0);
        }
    }

    /// Fills a rectangle given in font pixels.
    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: u32) {
        let p = self.p;
        for yy in y * p..((y + h) * p).min(self.h) {
            for xx in x * p..((x + w) * p).min(self.w) {
                self.buf[yy * self.stride + xx] = c;
            }
        }
    }

    fn text(&mut self, x: usize, y: usize, s: &str, c: Rgb) {
        let col = px(c, 1.0);
        for (i, ch) in s.chars().enumerate() {
            for gy in 0..GLYPH_H {
                for gx in 0..GLYPH_W {
                    if pixel(ch, gx, gy) {
                        self.rect(x + i * ADVANCE + gx, y + gy, 1, 1, col);
                    }
                }
            }
        }
    }

    /// A pixel box: notched corners, 1px outline, and a 1px drop shadow.
    fn panel(&mut self, w: usize, h: usize) {
        let shadow = px((0, 0, 0), 0.28);
        self.rect(2, h, w - 2, 1, shadow);
        self.rect(w, 2, 1, h - 1, shadow);
        let border = px(BORDER, 1.0);
        let fill = px(FILL, 0.97);
        self.rect(1, 0, w - 2, h, border);
        self.rect(0, 1, w, h - 2, border);
        self.rect(1, 1, w - 2, h - 2, fill);
    }
}

/// Size in screen pixels of a bubble holding `lines`.
pub fn bubble_size(lines: &[String], p: usize) -> (usize, usize) {
    let cols = lines.iter().map(|l| l.len()).max().unwrap_or(1).max(3);
    let w = cols * ADVANCE - 1 + PAD * 2;
    let h = lines.len().max(1) * (GLYPH_H + LINE_GAP) - LINE_GAP + PAD * 2;
    ((w + 1) * p, (h + TAIL + 1) * p)
}

/// Draws a speech bubble with the first `shown` characters of `lines` revealed. The
/// tail points down from `tail_at` (0..1 across the bubble). Returns the size drawn.
pub fn draw_bubble(buf: &mut [u32], stride: usize, lines: &[String], shown: usize, tail_at: f32, p: usize) -> (usize, usize) {
    let (sw, sh) = bubble_size(lines, p);
    let mut cv = Canvas { buf, stride, w: sw, h: sh, p };
    cv.clear();
    let w = sw / p - 1;
    let h = sh / p - 1 - TAIL;
    cv.panel(w, h);
    // Tail: a stepped triangle hanging under the bubble.
    let tx = ((w as f32 * tail_at) as usize).clamp(4, w.saturating_sub(8));
    let border = px(BORDER, 1.0);
    let fill = px(FILL, 0.97);
    for i in 0..TAIL {
        let width = (TAIL - i).max(1);
        cv.rect(tx + i, h - 1 + i, width + 2, 1, border);
        if width > 1 {
            cv.rect(tx + i + 1, h - 1 + i, width, 1, fill);
        }
    }
    let mut left = shown;
    for (row, line) in lines.iter().enumerate() {
        let take = left.min(line.len());
        cv.text(PAD, PAD + row * (GLYPH_H + LINE_GAP), &line[..take], TEXT);
        left -= take;
        if left == 0 {
            break;
        }
        left = left.saturating_sub(1); // the wrapped space
    }
    (sw, sh)
}

// --- Settings panel (opened by right-clicking the slime) ---

/// Something clickable on the settings panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Close,
    /// One of the tabs along the top.
    Tab(usize),
    // Chat tab.
    /// "Ask about this page": take a screenshot.
    Shoot,
    /// Remove the attached screenshot.
    DropShot,
    // Settings tab.
    Slider,
    Jump,
    Facts,
    Idle,
    Web,
    ProviderPrev,
    ProviderNext,
    ModelPrev,
    ModelNext,
    PasteKey,
    ClearKey,
    Reset,
    Quit,
    // Customisation tab.
    Swatch(usize),
    /// One of the Customisation sliders.
    Tune(usize),
    ResetLook,
}

pub const TABS: [&str; 3] = ["Chat", "Settings", "Customisation"];
pub const TAB_CHAT: usize = 0;
pub const TAB_LOOK: usize = 2;
pub const LOOK_N: usize = 10;
pub const SWATCH_N: usize = 9;

/// What the panel shows.
pub struct PanelView<'a> {
    pub tab: usize,
    /// Chat tab: the conversation so far (true = the user), whether he's thinking, the
    /// text being typed, the cursor blink, and which brain answers.
    pub chat: &'a [(bool, String)],
    pub thinking: bool,
    pub input: &'a str,
    pub cursor_on: bool,
    pub brain_line: &'a str,
    /// The screenshot waiting to be sent with the next question.
    pub shot: Option<ShotView<'a>>,
    /// Size slider position, 0..1, and where normal size sits on it.
    pub size: f32,
    pub size_normal: f32,
    pub size_pct: u32,
    pub jump: bool,
    pub facts: bool,
    pub idle: bool,
    pub web: bool,
    pub provider: &'a str,
    pub model: &'a str,
    pub key: &'a str,
    /// Show Paste / Clear (API providers only), and whether there's a key to clear.
    pub key_buttons: bool,
    pub key_clearable: bool,
    pub status: &'a str,
    /// Customisation sliders: labels, positions (0..1) and their standard positions.
    pub look_labels: [&'a str; LOOK_N],
    pub look: [f32; LOOK_N],
    pub look_normal: [f32; LOOK_N],
    /// Colour presets, and which one matches the current colour.
    pub swatches: [Rgb; SWATCH_N],
    pub swatch_on: Option<usize>,
    pub hover: Option<Hit>,
}

/// An attached screenshot: what it's of, and its thumbnail (opaque BGRA, screen pixels).
pub struct ShotView<'a> {
    pub label: &'a str,
    pub thumb: &'a [u32],
    pub w: usize,
    pub h: usize,
}

/// Largest thumbnail, in font pixels.
pub const THUMB_MAX: (usize, usize) = (44, 22);

/// The first three Customisation sliders are colour (hue, vividness, clarity); the rest feel.
const COLOUR_SLIDERS: usize = 3;

const PANEL_COLS: usize = 34;
const PANEL_W: usize = PANEL_COLS * ADVANCE - 1 + PAD * 2;
const HEADER_H: usize = 13;
const TAB_Y: usize = HEADER_H + 2;
const TAB_H: usize = 12;
// Chat tab: the log (rows of text), the input line, and the brain line at the bottom.
const CHAT_LOG_Y: usize = 30;
const CHAT_LOG_ROWS: usize = 11;
const CHAT_LINE_H: usize = GLYPH_H + 2;
const CHAT_COLS: usize = PANEL_COLS - 1;
/// The screenshot row (button, or thumbnail and label), between the log and the input.
const ROW_SHOT: usize = 149;
const ROW_INPUT: usize = 178;
// Settings tab rows.
const ROW_SIZE: usize = 33;
const ROW_JUMP: usize = 46;
const ROW_FACTS: usize = 58;
const ROW_IDLE: usize = 70;
const ROW_BRAIN: usize = 84;
const ROW_PROVIDER: usize = 97;
const ROW_MODEL: usize = 110;
const ROW_WEB: usize = 123;
const ROW_KEY: usize = 136;
const ROW_STATUS: usize = 149;
// Customisation tab rows.
const ROW_COLOUR: usize = 31;
const ROW_SWATCHES: usize = 42;
const SWATCH_W: usize = 21;
const ROW_FEEL: usize = 94;
/// Bottom row (every tab).
const ROW_BUTTONS: usize = 194;
const PANEL_H: usize = ROW_BUTTONS + GLYPH_H + PAD + 2;
/// Size slider track ends, in font pixels.
const SLIDER_X0: usize = PAD + 6 * ADVANCE;
const SLIDER_X1: usize = PAD + 29 * ADVANCE;
/// Customisation slider track ends.
const TUNE_X0: usize = PAD + 9 * ADVANCE;
const TUNE_X1: usize = PAD + 33 * ADVANCE;
/// Status lines fit this many characters.
pub const STATUS_COLS: usize = PANEL_COLS;

const BUTTON: Rgb = (226, 244, 214);
const HOVER: Rgb = (196, 232, 178);
const TRACK: Rgb = (150, 180, 150);
const TAB_OFF: Rgb = (206, 228, 196);

/// Left edge of text column `c`.
const fn col(c: usize) -> usize {
    PAD + c * ADVANCE
}

/// Row of Customisation slider `i`.
fn tune_row(i: usize) -> usize {
    if i < COLOUR_SLIDERS {
        57 + 12 * i
    } else {
        ROW_FEEL + 12 + 12 * (i - COLOUR_SLIDERS)
    }
}

/// Left edge and width of tab `i`.
fn tab_rect(i: usize) -> (usize, usize) {
    let w = |t: &str| t.len() * ADVANCE + 7;
    let x = PAD - 1 + TABS[..i].iter().map(|t| w(t) + 2).sum::<usize>();
    (x, w(TABS[i]))
}

pub fn panel_size(p: usize) -> (usize, usize) {
    ((PANEL_W + 1) * p, (PANEL_H + 1) * p)
}

/// Clickable areas on `tab`, in font pixels: (hit, x, y, w, h). `has_shot`: a screenshot
/// is attached (the Chat tab then offers to remove it instead of taking one).
fn panel_hits(tab: usize, key_buttons: bool, has_shot: bool) -> Vec<(Hit, usize, usize, usize, usize)> {
    let full = PANEL_W - 2 * PAD + 2;
    let mut v = vec![(Hit::Close, PANEL_W - 15, 0, 15, HEADER_H)];
    for i in 0..TABS.len() {
        let (x, w) = tab_rect(i);
        v.push((Hit::Tab(i), x, TAB_Y, w, TAB_H));
    }
    if tab == TAB_CHAT {
        if has_shot {
            v.push((Hit::DropShot, col(PANEL_COLS - 3) - 1, ROW_SHOT + 5, 3 * ADVANCE + 1, GLYPH_H + 4));
        } else {
            v.push((Hit::Shoot, PAD - 1, ROW_SHOT + 5, full, GLYPH_H + 4));
        }
        return v;
    }
    if tab == TAB_LOOK {
        for i in 0..SWATCH_N {
            v.push((Hit::Swatch(i), PAD - 1 + i * (SWATCH_W + 2), ROW_SWATCHES, SWATCH_W, 11));
        }
        for i in 0..LOOK_N {
            v.push((Hit::Tune(i), TUNE_X0 - 4, tune_row(i) - 3, TUNE_X1 - TUNE_X0 + 8, GLYPH_H + 5));
        }
        v.push((Hit::ResetLook, PAD - 1, ROW_BUTTONS - 2, full, GLYPH_H + 4));
        return v;
    }
    v.extend([
        (Hit::Slider, SLIDER_X0 - 4, ROW_SIZE - 3, SLIDER_X1 - SLIDER_X0 + 8, GLYPH_H + 5),
        (Hit::Jump, PAD - 1, ROW_JUMP - 2, full, GLYPH_H + 3),
        (Hit::Facts, PAD - 1, ROW_FACTS - 2, full, GLYPH_H + 3),
        (Hit::Idle, PAD - 1, ROW_IDLE - 2, full, GLYPH_H + 3),
        (Hit::ProviderPrev, col(9), ROW_PROVIDER - 2, 3 * ADVANCE, GLYPH_H + 4),
        (Hit::ProviderNext, col(12), ROW_PROVIDER - 2, col(PANEL_COLS) - col(12), GLYPH_H + 4),
        (Hit::ModelPrev, col(9), ROW_MODEL - 2, 3 * ADVANCE, GLYPH_H + 4),
        (Hit::ModelNext, col(12), ROW_MODEL - 2, col(PANEL_COLS) - col(12), GLYPH_H + 4),
        (Hit::Web, PAD - 1, ROW_WEB - 2, full, GLYPH_H + 3),
        (Hit::Reset, PAD - 1, ROW_BUTTONS - 2, 13 * ADVANCE, GLYPH_H + 4),
        (Hit::Quit, col(PANEL_COLS - 8) - 1, ROW_BUTTONS - 2, 8 * ADVANCE + 1, GLYPH_H + 4),
    ]);
    if key_buttons {
        v.push((Hit::PasteKey, col(21) - 3, ROW_KEY - 2, 5 * ADVANCE + 5, GLYPH_H + 4));
        v.push((Hit::ClearKey, col(28) - 3, ROW_KEY - 2, 5 * ADVANCE + 5, GLYPH_H + 4));
    }
    v
}

/// What's under (`x`, `y`) screen pixels from the panel's corner.
pub fn panel_hit(x: i32, y: i32, tab: usize, key_buttons: bool, has_shot: bool, p: usize) -> Option<Hit> {
    if x < 0 || y < 0 {
        return None;
    }
    let (fx, fy) = (x as usize / p, y as usize / p);
    panel_hits(tab, key_buttons, has_shot)
        .into_iter()
        .find(|&(_, hx, hy, hw, hh)| fx >= hx && fx < hx + hw && fy >= hy && fy < hy + hh)
        .map(|h| h.0)
}

/// Position (0..1) of slider `hit` for screen x relative to the panel.
pub fn slider_at(hit: Hit, x: i32, p: usize) -> f32 {
    let (x0, x1) = if hit == Hit::Slider { (SLIDER_X0, SLIDER_X1) } else { (TUNE_X0, TUNE_X1) };
    let fx = x as f32 / p as f32;
    ((fx - x0 as f32) / (x1 - x0) as f32).clamp(0.0, 1.0)
}

/// How a slider's track is coloured.
enum Track {
    Plain,
    /// The colour wheel.
    Hue,
    /// Grey to full colour at this hue.
    Vivid(f32),
}

fn rgb((r, g, b): (f32, f32, f32)) -> Rgb {
    (r.round().clamp(0.0, 255.0) as u8, g.round().clamp(0.0, 255.0) as u8, b.round().clamp(0.0, 255.0) as u8)
}

impl Canvas<'_> {
    /// Copies opaque screen pixels (`w` x `h`) to font-pixel position (`x`, `y`).
    fn blit(&mut self, x: usize, y: usize, pixels: &[u32], w: usize, h: usize) {
        let (sx, sy) = (x * self.p, y * self.p);
        for row in 0..h.min(self.h.saturating_sub(sy)) {
            let n = w.min(self.w.saturating_sub(sx));
            let at = (sy + row) * self.stride + sx;
            self.buf[at..at + n].copy_from_slice(&pixels[row * w..row * w + n]);
        }
    }

    /// A box with notched corners and a 1px outline.
    fn boxed(&mut self, x: usize, y: usize, w: usize, h: usize, fill: Rgb) {
        let border = px(BORDER, 1.0);
        self.rect(x + 1, y, w - 2, h, border);
        self.rect(x, y + 1, w, h - 2, border);
        self.rect(x + 1, y + 1, w - 2, h - 2, px(fill, 1.0));
    }

    fn button(&mut self, x: usize, y: usize, w: usize, label: &str, hover: bool) {
        self.boxed(x, y, w, GLYPH_H + 4, if hover { HOVER } else { BUTTON });
        let tw = label.len() * ADVANCE - 1;
        self.text(x + (w - tw) / 2, y + 2, label, TEXT);
    }

    fn checkbox(&mut self, y: usize, label: &str, on: bool, hover: bool) {
        if hover {
            self.rect(PAD - 1, y - 2, PANEL_W - 2 * PAD + 2, GLYPH_H + 3, px(HOVER, 1.0));
        }
        self.boxed(PAD, y, 8, 8, BUTTON);
        if on {
            // A little pixel tick.
            let c = px(PROMPT, 1.0);
            for (dx, dy) in [(1, 3), (2, 4), (3, 5), (2, 3), (3, 4), (4, 3), (5, 2), (6, 1), (4, 4), (5, 3), (6, 2)] {
                self.rect(PAD + dx, y + dy, 1, 1, c);
            }
        }
        self.text(col(2), y, label, TEXT);
    }

    /// `label  < value >`, cycling through choices.
    fn selector(&mut self, y: usize, label: &str, value: &str, hover: Option<Hit>, prev: Hit, next: Hit) {
        self.text(PAD, y, label, TEXT);
        let x = col(9);
        let w = col(PANEL_COLS) - x;
        self.boxed(x, y - 2, w, GLYPH_H + 4, if hover == Some(next) { HOVER } else { BUTTON });
        if hover == Some(prev) {
            self.rect(x + 1, y - 1, 3 * ADVANCE - 1, GLYPH_H + 2, px(HOVER, 1.0));
        }
        self.text(col(10) - 2, y, "<", PROMPT);
        self.text(col(12), y, value, TEXT);
        self.text(col(PANEL_COLS - 1) - 2, y, ">", PROMPT);
    }

    fn divider(&mut self, y: usize, label: &str) {
        self.text(PAD, y, label, PROMPT);
        let x = PAD + label.len() * ADVANCE + 2;
        self.rect(x, y + 3, PANEL_W - PAD - x, 1, px(TRACK, 1.0));
    }

    /// A slider track from `x0` to `x1` at text row `y`, with its knob at `t` (0..1) and
    /// a notch at `notch` (the standard setting).
    #[allow(clippy::too_many_arguments)]
    fn slider(&mut self, x0: usize, x1: usize, y: usize, t: f32, notch: f32, hover: bool, track: Track) {
        let len = x1 - x0;
        let knob = x0 + (t.clamp(0.0, 1.0) * len as f32).round() as usize;
        match track {
            Track::Plain => {
                self.rect(x0, y + 3, len, 2, px(TRACK, 1.0));
                self.rect(x0, y + 3, knob - x0, 2, px(PROMPT, 1.0));
            }
            Track::Hue | Track::Vivid(_) => {
                for x in x0..x1 {
                    let f = (x - x0) as f32 / len as f32;
                    let c = match track {
                        Track::Vivid(h) => super::draw::hue_rgb(h, f * 0.8, 0.5),
                        _ => super::draw::hue_rgb(f, 0.65, 0.5),
                    };
                    self.rect(x, y + 2, 1, 4, px(rgb(c), 1.0));
                }
            }
        }
        let n = x0 + (notch.clamp(0.0, 1.0) * len as f32).round() as usize;
        self.rect(n, y + 7, 1, 2, px(TRACK, 1.0));
        self.boxed(knob - 3, y - 2, 7, GLYPH_H + 3, if hover { HOVER } else { FILL });
    }

    fn tabs(&mut self, active: usize, hover: Option<Hit>) {
        let border = px(BORDER, 1.0);
        self.rect(PAD - 1, TAB_Y + TAB_H - 1, PANEL_W - 2 * PAD + 2, 1, border);
        for (i, label) in TABS.iter().enumerate() {
            let (x, w) = tab_rect(i);
            let fill = if i == active {
                FILL
            } else if hover == Some(Hit::Tab(i)) {
                HOVER
            } else {
                TAB_OFF
            };
            // Notched top corners; the active tab opens into the page below.
            self.rect(x + 1, TAB_Y, w - 2, TAB_H, border);
            self.rect(x, TAB_Y + 1, w, TAB_H - 1, border);
            let h = if i == active { TAB_H - 1 } else { TAB_H - 2 };
            self.rect(x + 1, TAB_Y + 1, w - 2, h, px(fill, 1.0));
            self.text(x + 4, TAB_Y + 2, label, if i == active { TEXT } else { HINT });
        }
    }
}

/// Draws the settings panel. Returns its size in screen pixels.
pub fn draw_panel(buf: &mut [u32], stride: usize, v: &PanelView, p: usize) -> (usize, usize) {
    let (sw, sh) = panel_size(p);
    let mut cv = Canvas { buf, stride, w: sw, h: sh, p };
    cv.clear();
    cv.panel(PANEL_W, PANEL_H);
    // Header strip.
    cv.rect(1, 1, PANEL_W - 2, HEADER_H - 1, px(BORDER, 1.0));
    cv.text(PAD, 3, "SLIME", FILL);
    if v.hover == Some(Hit::Close) {
        cv.rect(PANEL_W - 15, 1, 14, HEADER_H - 1, px(PROMPT, 1.0));
    }
    cv.text(PANEL_W - PAD - GLYPH_W - 2, 3, "x", FILL);
    cv.tabs(v.tab, v.hover);
    match v.tab {
        TAB_CHAT => draw_chat_tab(&mut cv, v),
        TAB_LOOK => draw_look_tab(&mut cv, v),
        _ => draw_settings_tab(&mut cv, v),
    }
    (sw, sh)
}

fn draw_settings_tab(cv: &mut Canvas, v: &PanelView) {
    let hover = |h: Hit| v.hover == Some(h);
    cv.text(PAD, ROW_SIZE, "Size", TEXT);
    cv.slider(SLIDER_X0, SLIDER_X1, ROW_SIZE, v.size, v.size_normal, hover(Hit::Slider), Track::Plain);
    let pct = format!("{}%", v.size_pct);
    cv.text(col(PANEL_COLS) - pct.len() * ADVANCE - 2, ROW_SIZE, &pct, TEXT);

    cv.checkbox(ROW_JUMP, "Jump to clicks", v.jump, hover(Hit::Jump));
    cv.checkbox(ROW_FACTS, "Fun facts", v.facts, hover(Hit::Facts));
    cv.checkbox(ROW_IDLE, "Idle antics", v.idle, hover(Hit::Idle));

    cv.divider(ROW_BRAIN, "Brain");
    cv.selector(ROW_PROVIDER, "Provider", v.provider, v.hover, Hit::ProviderPrev, Hit::ProviderNext);
    cv.selector(ROW_MODEL, "Model", v.model, v.hover, Hit::ModelPrev, Hit::ModelNext);
    cv.checkbox(ROW_WEB, "Search the web", v.web, hover(Hit::Web));
    cv.text(PAD, ROW_KEY, "Key", TEXT);
    let room = if v.key_buttons { 11 } else { PANEL_COLS - 10 };
    cv.text(col(9), ROW_KEY, &v.key.chars().take(room).collect::<String>(), HINT);
    if v.key_buttons {
        cv.button(col(21) - 3, ROW_KEY - 2, 5 * ADVANCE + 5, "Paste", hover(Hit::PasteKey));
        if v.key_clearable {
            cv.button(col(28) - 3, ROW_KEY - 2, 5 * ADVANCE + 5, "Clear", hover(Hit::ClearKey));
        }
    }
    for (i, line) in super::font::wrap(v.status, STATUS_COLS).iter().take(2).enumerate() {
        cv.text(PAD, ROW_STATUS + i * (GLYPH_H + 2), line, PROMPT);
    }

    cv.rect(PAD, ROW_BUTTONS - 6, PANEL_W - 2 * PAD, 1, px(TRACK, 1.0));
    cv.button(PAD - 1, ROW_BUTTONS - 2, 13 * ADVANCE, "Reset slime", hover(Hit::Reset));
    cv.button(col(PANEL_COLS - 8) - 1, ROW_BUTTONS - 2, 8 * ADVANCE + 1, "Quit", hover(Hit::Quit));
}

/// The chat log as lines to draw: the user's messages after a `>` in green, his in the
/// text colour, a gap between messages; only the newest that fit.
pub fn chat_lines(chat: &[(bool, String)], thinking: bool) -> Vec<(bool, String)> {
    let mut lines: Vec<(bool, String)> = Vec::new();
    for (user, text) in chat {
        if !lines.is_empty() {
            lines.push((false, String::new()));
        }
        let wrapped = super::font::wrap(text, CHAT_COLS - 2);
        for (i, l) in wrapped.into_iter().enumerate() {
            let lead = if !*user { "" } else if i == 0 { "> " } else { "  " };
            lines.push((*user, format!("{lead}{l}")));
        }
    }
    if thinking {
        if !lines.is_empty() {
            lines.push((false, String::new()));
        }
        lines.push((false, "...".into()));
    }
    let skip = lines.len().saturating_sub(CHAT_LOG_ROWS);
    lines.split_off(skip)
}

fn draw_chat_tab(cv: &mut Canvas, v: &PanelView) {
    let full = PANEL_W - 2 * PAD + 2;
    cv.boxed(PAD - 1, CHAT_LOG_Y - 1, full, CHAT_LOG_ROWS * CHAT_LINE_H + 4, (236, 246, 228));
    let lines = chat_lines(v.chat, v.thinking);
    if lines.is_empty() {
        cv.text(PAD + 2, CHAT_LOG_Y + 2, "Ask me anything!", HINT);
        cv.text(PAD + 2, CHAT_LOG_Y + 2 + CHAT_LINE_H, "Type below and press Enter.", HINT);
    }
    for (i, (user, line)) in lines.iter().enumerate() {
        cv.text(PAD + 2, CHAT_LOG_Y + 2 + i * CHAT_LINE_H, line, if *user { PROMPT } else { TEXT });
    }
    // The screenshot: a button to take one, or the one that will go with the next question.
    match &v.shot {
        None => cv.button(PAD - 1, ROW_SHOT + 5, full, "Ask about this page", v.hover == Some(Hit::Shoot)),
        Some(shot) => {
            let p = cv.p;
            let (tw, th) = (shot.w.div_ceil(p), shot.h.div_ceil(p));
            cv.boxed(PAD - 1, ROW_SHOT, tw + 2, th + 2, BORDER);
            cv.blit(PAD, ROW_SHOT + 1, shot.thumb, shot.w, shot.h);
            let x = PAD + tw + 5;
            let cols = (col(PANEL_COLS - 3) - 2 - x) / ADVANCE;
            cv.text(x, ROW_SHOT + 2, "Screenshot of", HINT);
            cv.text(x, ROW_SHOT + 2 + CHAT_LINE_H, &super::font::to_ascii(shot.label).chars().take(cols).collect::<String>(), TEXT);
            cv.button(col(PANEL_COLS - 3) - 1, ROW_SHOT + 5, 3 * ADVANCE + 1, "x", v.hover == Some(Hit::DropShot));
        }
    }
    // The input line, showing the end of what's typed.
    cv.boxed(PAD - 1, ROW_INPUT - 3, full, GLYPH_H + 6, FILL);
    cv.text(PAD + 2, ROW_INPUT, ">", PROMPT);
    let room = CHAT_COLS - 3;
    let x = PAD + 2 + 2 * ADVANCE;
    if v.input.is_empty() {
        cv.text(x, ROW_INPUT, if v.shot.is_some() { "ask about the screenshot..." } else { "ask me anything..." }, HINT);
    } else {
        let start = v.input.len().saturating_sub(room);
        cv.text(x, ROW_INPUT, &v.input[start..], TEXT);
    }
    if v.cursor_on {
        let shown = v.input.len().min(room);
        cv.rect(x + shown * ADVANCE, ROW_INPUT + GLYPH_H - 1, GLYPH_W, 1, px(TEXT, 1.0));
    }
    cv.text(PAD, ROW_BUTTONS, &v.brain_line.chars().take(PANEL_COLS).collect::<String>(), HINT);
}

fn draw_look_tab(cv: &mut Canvas, v: &PanelView) {
    let hover = |h: Hit| v.hover == Some(h);
    cv.divider(ROW_COLOUR, "Colour");
    for (i, &c) in v.swatches.iter().enumerate() {
        let x = PAD - 1 + i * (SWATCH_W + 2);
        let on = v.swatch_on == Some(i);
        if on || hover(Hit::Swatch(i)) {
            // A frame around the chosen (or hovered) colour.
            cv.boxed(x, ROW_SWATCHES, SWATCH_W, 11, if on { BORDER } else { TRACK });
            cv.boxed(x + 2, ROW_SWATCHES + 2, SWATCH_W - 4, 7, c);
        } else {
            cv.boxed(x, ROW_SWATCHES, SWATCH_W, 11, c);
        }
    }
    for i in 0..LOOK_N {
        if i == COLOUR_SLIDERS {
            cv.divider(ROW_FEEL, "Feel");
        }
        let y = tune_row(i);
        cv.text(PAD, y, v.look_labels[i], TEXT);
        let track = match i {
            0 => Track::Hue,
            1 => Track::Vivid(v.look[0]),
            _ => Track::Plain,
        };
        cv.slider(TUNE_X0, TUNE_X1, y, v.look[i], v.look_normal[i], hover(Hit::Tune(i)), track);
    }
    cv.rect(PAD, ROW_BUTTONS - 6, PANEL_W - 2 * PAD, 1, px(TRACK, 1.0));
    cv.button(PAD - 1, ROW_BUTTONS - 2, PANEL_W - 2 * PAD + 2, "Back to the classic slime", hover(Hit::ResetLook));
}

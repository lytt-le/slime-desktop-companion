//! What the settings panel remembers between runs: %APPDATA%\SlimeCompanion\settings.json.

use serde_json::{json, Value};

use super::brain::{data_dir, Provider};
use super::draw::{Palette, BASE_HUE};
use super::sim::Tuning;
use super::ui::{SECTIONS, SECTION_N};

pub const MIN_SIZE: f32 = 0.5;
pub const MAX_SIZE: f32 = 2.5;

/// The Customisation sliders: (settings key, label). Each is 0..1 with the standard
/// slime at 0.5 (hue: the classic green).
pub const LOOK: [(&str, &str); 10] = [
    ("hue", "Colour"),
    ("vivid", "Vivid"),
    ("clarity", "Clarity"),
    ("runny", "Runny"),
    ("wobble", "Wobble"),
    ("splatter", "Splatter"),
    ("trail", "Trail"),
    ("bubbles", "Bubbles"),
    ("grip", "Grip"),
    ("weight", "Weight"),
];
pub const HUE: usize = 0;
pub const VIVID: usize = 1;
pub const LOOK_DEFAULT: [f32; 10] = [BASE_HUE, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
/// Colour presets: (hue, vivid). Green, teal, blue, purple, pink, red, orange, gold, grey.
pub const SWATCHES: [(f32, f32); 9] =
    [(BASE_HUE, 0.5), (0.47, 0.5), (0.6, 0.5), (0.76, 0.5), (0.9, 0.55), (0.0, 0.55), (0.07, 0.6), (0.13, 0.55), (BASE_HUE, 0.0)];

/// The swatches' colours, as drawn on the panel.
pub fn swatch_colours() -> [(u8, u8, u8); 9] {
    SWATCHES.map(|(h, v)| {
        let c = Palette::tinted(h, knob(v, 0.0, 1.0, 1.6), 1.0).shallow;
        (c.0.round().clamp(0.0, 255.0) as u8, c.1.round().clamp(0.0, 255.0) as u8, c.2.round().clamp(0.0, 255.0) as u8)
    })
}

/// Slider position `t` (0..1) to a value: `lo` at 0, `def` at the middle, `hi` at 1.
fn knob(t: f32, lo: f32, def: f32, hi: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        lo + (def - lo) * t * 2.0
    } else {
        def + (hi - def) * (t - 0.5) * 2.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub provider: Provider,
    /// Chosen model index for each provider, in `Provider::ALL` order.
    pub models: [usize; 3],
    pub size: f32,
    pub jump: bool,
    pub facts: bool,
    /// Walks, hops, wobbles and the like by himself now and then.
    pub idle: bool,
    /// Let the brain search the web.
    pub web: bool,
    /// Customisation slider positions, in `LOOK` order.
    pub look: [f32; 10],
    /// Easter eggs: his colour slides round the wheel (three clicks on "Colour", until
    /// another colour is picked), and he never puts the cigarette down (three clicks on
    /// "Brain").
    pub rainbow: bool,
    pub nootropic: bool,
    /// Keepy uppies: bounce him on the cursor while he's in the air; the most bounces in
    /// a row so far.
    pub keepy: bool,
    pub keepy_best: u32,
    /// Panel sections closed with their titles (in `ui::SECTIONS` order).
    pub collapsed: [bool; SECTION_N],
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { provider: Provider::ClaudePlan, models: [0; 3], size: 1.0, jump: true, facts: true, idle: true, web: true, look: LOOK_DEFAULT, rainbow: false, nootropic: false, keepy: false, keepy_best: 0, collapsed: [false; SECTION_N] }
    }
}

fn slot(p: Provider) -> usize {
    Provider::ALL.iter().position(|&q| q == p).unwrap_or(0)
}

impl Settings {
    pub fn model(&self) -> (&'static str, &'static str) {
        let models = self.provider.models();
        models[self.models[slot(self.provider)].min(models.len() - 1)]
    }

    pub fn cycle_provider(&mut self, step: i32) {
        let n = Provider::ALL.len() as i32;
        self.provider = Provider::ALL[(slot(self.provider) as i32 + step).rem_euclid(n) as usize];
    }

    pub fn cycle_model(&mut self, step: i32) {
        let n = self.provider.models().len() as i32;
        let i = &mut self.models[slot(self.provider)];
        *i = (*i as i32 + step).rem_euclid(n) as usize;
    }

    pub fn palette(&self) -> Palette {
        let l = &self.look;
        Palette::tinted(l[HUE], knob(l[VIVID], 0.0, 1.0, 1.6), knob(l[2], 0.35, 0.94, 1.0))
    }

    /// How the slime moves, from the Feel sliders; the middle of each is the standard slime.
    pub fn tuning(&self) -> Tuning {
        let l = &self.look;
        let d = Tuning::default();
        let (runny, wobble, splatter, trail, bubbles, grip, weight) = (l[3], l[4], l[5], l[6], l[7], l[8], l[9]);
        Tuning {
            viscosity: knob(runny, 0.6, d.viscosity, 0.08),
            runny: knob(runny, 0.25, d.runny, 0.95),
            spill: knob(runny, 0.08, d.spill, 0.5),
            dome_aspect: knob(runny, 1.1, d.dome_aspect, 0.55),
            st_gravity: knob(runny, 0.15, d.st_gravity, 0.7),
            jelly_damping: knob(wobble, 0.6, d.jelly_damping, 0.05),
            jelly_kick: knob(wobble, 0.3, 1.0, 1.8),
            shed: knob(splatter, 0.0, 1.0, 5.0),
            goo_life: knob(trail, 0.2, 1.0, 4.0),
            bubble_max: knob(bubbles, 0.0, d.bubble_max as f32, 20.0).round() as usize,
            bubble_rate: knob(bubbles, 0.0, 1.0, 4.0),
            ground_friction: knob(grip, 1.0, d.ground_friction, 16.0),
            friction: knob(grip, 0.05, d.friction, 0.6),
            weight: knob(weight, 0.45, 1.0, 1.8),
        }
    }

    pub fn to_json(&self) -> Value {
        let models: serde_json::Map<String, Value> = Provider::ALL
            .iter()
            .map(|&p| (p.id().to_string(), json!(p.models()[self.models[slot(p)].min(p.models().len() - 1)].0)))
            .collect();
        json!({
            "provider": self.provider.id(),
            "models": models,
            "size": self.size,
            "jump_to_clicks": self.jump,
            "fun_facts": self.facts,
            "idle_antics": self.idle,
            "web_search": self.web,
            "rainbow": self.rainbow,
            "nootropic": self.nootropic,
            "keepy_uppies": self.keepy,
            "keepy_uppies_best": self.keepy_best,
            "collapsed": SECTIONS.iter().zip(self.collapsed).filter(|s| s.1).map(|s| s.0.label()).collect::<Vec<_>>(),
            "look": LOOK.iter().zip(self.look).map(|(l, v)| (l.0.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
        })
    }

    /// Reads what it can; anything missing or unknown keeps its default.
    pub fn from_json(v: &Value) -> Settings {
        let mut s = Settings::default();
        if let Some(p) = v["provider"].as_str().and_then(Provider::from_id) {
            s.provider = p;
        }
        for (i, p) in Provider::ALL.iter().enumerate() {
            if let Some(id) = v["models"][p.id()].as_str() {
                if let Some(k) = p.models().iter().position(|m| m.0 == id) {
                    s.models[i] = k;
                }
            }
        }
        if let Some(size) = v["size"].as_f64() {
            s.size = (size as f32).clamp(MIN_SIZE, MAX_SIZE);
        }
        s.jump = v["jump_to_clicks"].as_bool().unwrap_or(s.jump);
        s.facts = v["fun_facts"].as_bool().unwrap_or(s.facts);
        s.idle = v["idle_antics"].as_bool().unwrap_or(s.idle);
        s.web = v["web_search"].as_bool().unwrap_or(s.web);
        s.rainbow = v["rainbow"].as_bool().unwrap_or(s.rainbow);
        s.nootropic = v["nootropic"].as_bool().unwrap_or(s.nootropic);
        s.keepy = v["keepy_uppies"].as_bool().unwrap_or(s.keepy);
        s.keepy_best = v["keepy_uppies_best"].as_u64().map_or(s.keepy_best, |n| n.min(u32::MAX as u64) as u32);
        if let Some(names) = v["collapsed"].as_array() {
            for (i, sec) in SECTIONS.iter().enumerate() {
                s.collapsed[i] = names.iter().any(|n| n.as_str() == Some(sec.label()));
            }
        }
        for (i, l) in LOOK.iter().enumerate() {
            if let Some(x) = v["look"][l.0].as_f64() {
                s.look[i] = (x as f32).clamp(0.0, 1.0);
            }
        }
        s
    }

    pub fn load() -> Settings {
        let text = data_dir().and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok());
        match text.and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(v) => Settings::from_json(&v),
            None => Settings::default(),
        }
    }

    pub fn save(&self) {
        if let Some(dir) = data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("settings.json"), serde_json::to_string_pretty(&self.to_json()).unwrap_or_default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_cycles() {
        let mut s = Settings::default();
        s.cycle_provider(-1);
        assert_eq!(s.provider, Provider::DeepSeek);
        assert_eq!(s.model().0, "deepseek-flash");
        s.cycle_model(1);
        assert_eq!(s.model().0, "deepseek-v4-pro");
        s.size = 1.7;
        s.facts = false;
        s.idle = false;
        s.web = false;
        s.rainbow = true;
        s.nootropic = true;
        s.keepy = true;
        s.keepy_best = 17;
        s.collapsed[1] = true;
        s.look[3] = 0.9;
        s.look[HUE] = 0.6;
        let back = Settings::from_json(&s.to_json());
        assert_eq!(back, s);
        // Junk is ignored.
        let junk = Settings::from_json(&json!({"provider": "nope", "size": 99, "models": {"anthropic": "gpt"}}));
        assert_eq!(junk.provider, Provider::ClaudePlan);
        assert_eq!(junk.size, MAX_SIZE);
    }

    #[test]
    fn middle_sliders_are_the_standard_slime() {
        let s = Settings::default();
        assert_eq!(s.tuning(), Tuning::default());
        let (a, b) = (s.palette(), Palette::default());
        for (x, y) in [(a.outline, b.outline), (a.shallow, b.shallow), (a.deep, b.deep), (a.glint, b.glint), (a.goo, b.goo)] {
            assert!((x.0 - y.0).abs() < 1.0 && (x.1 - y.1).abs() < 1.0 && (x.2 - y.2).abs() < 1.0, "{x:?} vs {y:?}");
        }
        assert!((a.alpha - b.alpha).abs() < 1e-6);
        // Ends of the sliders do something sensible.
        let mut t = Settings::default();
        t.look = [0.6, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0];
        let (tu, pa) = (t.tuning(), t.palette());
        assert!(tu.viscosity < 0.1 && tu.shed == 0.0 && tu.bubble_max == 0 && tu.weight < 0.5);
        assert!((pa.shallow.0 - pa.shallow.1).abs() < 1.0, "vivid 0 should be grey: {:?}", pa.shallow);
    }
}

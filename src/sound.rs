//! Sound effects, all synthesised: wet splats when he lands or hits a wall, a squelch
//! when he's picked up, boings for hops and jumps, blorps and wobbles, a plip for each
//! bubble, a pin going in, and a boom (with the goo raining down after) when he bursts.
//! They're mixed on a thread of their own and played through the default output, which
//! is only held open while something's playing (an open stream can keep the PC awake).

use super::settings::VOLUME_DEFAULT;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use windows_sys::Win32::Media::Audio::{
    waveOutClose, waveOutOpen, waveOutPrepareHeader, waveOutReset, waveOutUnprepareHeader, waveOutWrite, CALLBACK_NULL,
    HWAVEOUT, WAVEFORMATEX, WAVEHDR, WAVE_FORMAT_PCM, WAVE_MAPPER, WHDR_DONE,
};

pub const RATE: u32 = 44100;
/// Overall loudness (the mix is soft-clipped, so a few at once don't crackle).
const MASTER: f32 = 0.55;
/// Output buffers: how many, and samples in each (about 12 ms).
const BUFFERS: usize = 6;
const BUFFER_LEN: usize = 512;
/// The device is let go after this long with nothing playing (s).
const CLOSE_AFTER: f32 = 1.5;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sfx {
    /// Landing or hitting something, how hard (0..1).
    Splat(f32),
    /// Picked up.
    Squelch,
    /// A little bloop (poked, talking, back in one piece).
    Blorp,
    /// Wobbling about like jelly.
    Wobble,
    /// A bubble popping.
    Plip,
    /// Springing up, how high (0..1; keepy uppies climb in pitch).
    Boing(f32),
    /// Bursting.
    Boom,
    /// A pin pushed in.
    Thunk,
    /// Goo slurping back together.
    Slurp,
}

impl Sfx {
    /// Which kind (for not playing the same thing over and over), and the least time
    /// between two of a kind (s).
    fn kind(self) -> (usize, f32) {
        match self {
            Sfx::Splat(_) => (0, 0.12),
            Sfx::Squelch => (1, 0.08),
            Sfx::Blorp => (2, 0.15),
            Sfx::Wobble => (3, 0.8),
            Sfx::Plip => (4, 0.07),
            Sfx::Boing(_) => (5, 0.1),
            Sfx::Boom => (6, 1.0),
            Sfx::Thunk => (7, 0.2),
            Sfx::Slurp => (8, 1.0),
        }
    }
}
const KINDS: usize = 9;

/// A sound to play: what, its seed, in how long (s), and how loud.
type Cue = (Sfx, u32, f32, f32);

/// Plays sound effects (or quietly does nothing if there's no sound device).
pub struct Sounds {
    /// Off (turned off in the settings, or he's hidden): nothing plays.
    pub on: bool,
    /// The volume slider (0..1; the default is as loud as the sounds were made).
    pub volume: f32,
    tx: Sender<Cue>,
    last: [Option<Instant>; KINDS],
    seed: u32,
}

impl Sounds {
    pub fn new() -> Sounds {
        let (tx, rx) = channel();
        std::thread::spawn(move || mixer(rx));
        Sounds { on: true, volume: VOLUME_DEFAULT, tx, last: [None; KINDS], seed: 0x2468_ace1 }
    }

    /// How much to scale the sounds by for the volume slider (loudness follows the
    /// square, so the slider feels even).
    fn gain(&self) -> f32 {
        (self.volume.clamp(0.0, 1.0) / VOLUME_DEFAULT).powi(2)
    }

    pub fn play(&mut self, s: Sfx) {
        self.play_after(s, 0.0);
    }

    /// Plays `s` in `delay` seconds.
    pub fn play_after(&mut self, s: Sfx, delay: f32) {
        if !self.on || self.volume <= 0.0 {
            return;
        }
        let (k, gap) = s.kind();
        let now = Instant::now() + Duration::from_secs_f32(delay);
        if self.last[k].is_some_and(|t| now.saturating_duration_since(t).as_secs_f32() < gap) {
            return;
        }
        self.last[k] = Some(now);
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let _ = self.tx.send((s, self.seed, delay, self.gain()));
    }
}

/// The mixing thread: waits for something to play, opens the device, mixes everything
/// playing into its buffers as they come free, and lets the device go once it's quiet.
fn mixer(rx: Receiver<Cue>) {
    let mut voices: Vec<(Vec<f32>, usize)> = Vec::new();
    let add = |voices: &mut Vec<(Vec<f32>, usize)>, (s, seed, delay, gain): Cue| {
        let mut v = vec![0.0; (delay * RATE as f32) as usize];
        v.extend(render(s, seed).into_iter().map(|x| x * gain));
        voices.push((v, 0));
    };
    loop {
        if voices.is_empty() {
            match rx.recv() {
                Ok(m) => add(&mut voices, m),
                Err(_) => return,
            }
        }
        let Some(mut out) = (unsafe { Out::open() }) else {
            // No device: drop what's queued and wait for the next.
            voices.clear();
            continue;
        };
        let mut quiet = 0.0f32;
        while quiet < CLOSE_AFTER {
            while let Ok(m) = rx.try_recv() {
                add(&mut voices, m);
            }
            let Some(buf) = (unsafe { out.free() }) else {
                std::thread::sleep(Duration::from_millis(3));
                continue;
            };
            let mut mix = [0.0f32; BUFFER_LEN];
            for (v, at) in &mut voices {
                let n = (v.len() - *at).min(BUFFER_LEN);
                for (m, s) in mix.iter_mut().zip(&v[*at..*at + n]) {
                    *m += s;
                }
                *at += n;
            }
            voices.retain(|(v, at)| *at < v.len());
            for (d, m) in buf.iter_mut().zip(mix) {
                *d = ((m * MASTER).tanh() * 32000.0) as i16;
            }
            unsafe { out.write() };
            quiet = if voices.is_empty() { quiet + BUFFER_LEN as f32 / RATE as f32 } else { 0.0 };
        }
    }
}

/// The open output device and its buffers, written round in turn.
struct Out {
    hwo: HWAVEOUT,
    headers: Box<[WAVEHDR; BUFFERS]>,
    data: Box<[[i16; BUFFER_LEN]; BUFFERS]>,
    queued: [bool; BUFFERS],
    next: usize,
}

impl Out {
    unsafe fn open() -> Option<Out> {
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 1,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 2,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut hwo: HWAVEOUT = std::ptr::null_mut();
        if waveOutOpen(&mut hwo, WAVE_MAPPER, &fmt, 0, 0, CALLBACK_NULL) != 0 {
            return None;
        }
        let mut out = Out { hwo, headers: Box::new(std::mem::zeroed()), data: Box::new([[0; BUFFER_LEN]; BUFFERS]), queued: [false; BUFFERS], next: 0 };
        for i in 0..BUFFERS {
            let h = &mut out.headers[i];
            h.lpData = out.data[i].as_mut_ptr() as *mut u8;
            h.dwBufferLength = (BUFFER_LEN * 2) as u32;
            waveOutPrepareHeader(hwo, h, std::mem::size_of::<WAVEHDR>() as u32);
        }
        Some(out)
    }

    /// The next buffer, if the device has finished with it.
    unsafe fn free(&mut self) -> Option<&mut [i16; BUFFER_LEN]> {
        let i = self.next;
        // (The driver sets the flag from its own thread; the header is packed.)
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
        let done = std::ptr::addr_of!(self.headers[i].dwFlags).read_unaligned() & WHDR_DONE != 0;
        (!self.queued[i] || done).then(|| &mut self.data[i])
    }

    /// Sends the buffer just filled.
    unsafe fn write(&mut self) {
        let i = self.next;
        waveOutWrite(self.hwo, &mut self.headers[i], std::mem::size_of::<WAVEHDR>() as u32);
        self.queued[i] = true;
        self.next = (i + 1) % BUFFERS;
    }
}

impl Drop for Out {
    fn drop(&mut self) {
        unsafe {
            waveOutReset(self.hwo);
            for h in self.headers.iter_mut() {
                waveOutUnprepareHeader(self.hwo, h, std::mem::size_of::<WAVEHDR>() as u32);
            }
            waveOutClose(self.hwo);
        }
    }
}

// --- Synthesis ---

struct Noise(u32);

impl Noise {
    /// White noise, -1..1.
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }

    /// 0..1.
    fn unit(&mut self) -> f32 {
        self.next() * 0.5 + 0.5
    }
}

/// A state-variable filter: low-pass, band-pass and high-pass at once.
#[derive(Default)]
struct Svf {
    low: f32,
    band: f32,
}

impl Svf {
    fn step(&mut self, x: f32, cutoff: f32, q: f32) -> (f32, f32, f32) {
        let f = 2.0 * (std::f32::consts::PI * cutoff.clamp(20.0, 6000.0) / RATE as f32).sin();
        let high = x - self.low - self.band / q;
        self.band += f * high;
        self.low += f * self.band;
        (self.low, self.band, high)
    }
}

/// A sine oscillator that follows a changing pitch.
#[derive(Default)]
struct Osc(f32);

impl Osc {
    fn step(&mut self, hz: f32) -> f32 {
        self.0 = (self.0 + hz / RATE as f32).fract();
        (self.0 * std::f32::consts::TAU).sin()
    }
}

fn len(secs: f32) -> usize {
    (secs * RATE as f32) as usize
}

fn time(i: usize) -> f32 {
    i as f32 / RATE as f32
}

/// Adds `src` into `dst` from sample `at`, scaled.
fn mix_in(dst: &mut Vec<f32>, src: &[f32], at: usize, gain: f32) {
    if dst.len() < at + src.len() {
        dst.resize(at + src.len(), 0.0);
    }
    for (d, s) in dst[at..].iter_mut().zip(src) {
        *d += s * gain;
    }
}

/// Renders one sound effect (mono samples at `RATE`, roughly -1..1), a little different
/// each time for each `seed`.
pub fn render(s: Sfx, seed: u32) -> Vec<f32> {
    let mut n = Noise(seed | 1);
    match s {
        Sfx::Splat(k) => splat(&mut n, k.clamp(0.0, 1.0)),
        Sfx::Squelch => squelch(&mut n),
        Sfx::Blorp => blorp(&mut n),
        Sfx::Wobble => wobble(&mut n),
        Sfx::Plip => plip(&mut n, 0.35),
        Sfx::Boing(k) => boing(&mut n, k.clamp(0.0, 1.0)),
        Sfx::Boom => boom(&mut n),
        Sfx::Thunk => thunk(&mut n),
        Sfx::Slurp => slurp(&mut n),
    }
}

/// A wet squelch: a sticky, resonant "schlp" sweeping down through the middle, fluttering,
/// and crackling with tiny bubbles bursting, over only a hint of weight; a harder hit is
/// longer and flicks droplets off.
fn splat(n: &mut Noise, k: f32) -> Vec<f32> {
    let dur = 0.14 + 0.24 * k;
    let (hi, lo) = (2600.0 + 900.0 * n.unit(), 450.0 + 150.0 * n.unit());
    let sweep = 0.05 + 0.05 * k;
    let flutter = 45.0 + 30.0 * n.unit();
    let (mut schlp, mut ring1, mut ring2, mut osc) = (Svf::default(), Svf::default(), Svf::default(), Osc::default());
    let mut out: Vec<f32> = (0..len(dur))
        .map(|i| {
            let t = time(i);
            // The schlp: resonant noise sliding down, wobbling as it goes.
            let (_, band, _) = schlp.step(n.next(), lo + (hi - lo) * (-t / sweep).exp(), 3.5);
            let wet = band * (1.0 + 0.7 * (t * flutter * std::f32::consts::TAU).sin()) * (-t / (0.04 + 0.07 * k)).exp();
            // Crackle: sparse clicks (bubbles bursting), ringing at a couple of wet pitches.
            let click = if n.unit() < 0.03 * (-t / (0.03 + 0.05 * k)).exp() { n.next() * 1.5 } else { 0.0 };
            let (_, c1, _) = ring1.step(click, 1700.0, 6.0);
            let (_, c2, _) = ring2.step(click, 3300.0, 6.0);
            // Just a little weight behind it, more for a hard hit.
            let weight = osc.step(190.0 * (0.7 + 0.3 * (-t / 0.02).exp())) * (-t / 0.025).exp() * 0.3 * k;
            (1.3 * wet + 0.5 * (c1 + c2) + weight) * (t / 0.003).min(1.0)
        })
        .collect();
    for _ in 0..(k * 6.0) as usize {
        let at = len(0.03 + 0.2 * n.unit());
        let drop = plip(n, 0.12 + 0.12 * k);
        mix_in(&mut out, &drop, at, 1.0);
    }
    let gain = 0.3 + 0.7 * k;
    out.iter_mut().for_each(|s| *s *= gain);
    out
}

/// Pulled at: resonant noise sweeping up, wet and gurgly.
fn squelch(n: &mut Noise) -> Vec<f32> {
    let (dur, from, to) = (0.22, 300.0 + 100.0 * n.unit(), 1600.0 + 600.0 * n.unit());
    let mut svf = Svf::default();
    let mut wob = 0.0f32;
    (0..len(dur))
        .map(|i| {
            let t = time(i);
            let cut = from * (to / from).powf((t / dur).min(1.0));
            let (_, band, _) = svf.step(n.next(), cut, 6.0);
            if i % 600 == 0 {
                wob = 0.4 + 0.6 * n.unit();
            }
            let env = (t / 0.008).min(1.0) * (-t / 0.07).exp();
            band * env * wob * 1.4
        })
        .collect()
}

/// A bloop: a round tone bending up, like a bubble leaving a jar of slime.
fn blorp(n: &mut Noise) -> Vec<f32> {
    let f0 = 170.0 + 90.0 * n.unit();
    let mut osc = Osc::default();
    let mut osc2 = Osc::default();
    (0..len(0.2))
        .map(|i| {
            let t = time(i);
            let f = f0 * (1.0 + 1.3 * (1.0 - (-t / 0.045).exp()));
            let env = (t / 0.005).min(1.0) * (-t / 0.06).exp();
            (osc.step(f) + 0.3 * osc2.step(f * 2.0)) * env * 0.55
        })
        .collect()
}

/// Wub-wub-wub: a low tone rocking in pitch and loudness, with a little wet noise.
fn wobble(n: &mut Noise) -> Vec<f32> {
    let dur = 0.9;
    let rate = 5.0 + n.unit();
    let mut osc = Osc::default();
    let mut svf = Svf::default();
    (0..len(dur))
        .map(|i| {
            let t = time(i);
            let w = (t * rate * std::f32::consts::TAU).sin();
            let tone = osc.step(95.0 * (1.0 + 0.18 * w));
            let (low, _, _) = svf.step(n.next(), 500.0 + 300.0 * w, 1.0);
            let pulse = (0.5 - 0.5 * w).powf(1.5);
            let env = (t / 0.05).min(1.0) * ((dur - t) / 0.2).clamp(0.0, 1.0);
            (0.6 * tone + 0.5 * low) * pulse * env
        })
        .collect()
}

/// A tiny pop: a click and a short chirp up.
fn plip(n: &mut Noise, gain: f32) -> Vec<f32> {
    let f0 = 900.0 + 900.0 * n.unit();
    let mut osc = Osc::default();
    (0..len(0.05))
        .map(|i| {
            let t = time(i);
            let click = if t < 0.0015 { n.next() * 0.5 } else { 0.0 };
            (osc.step(f0 * (1.0 + 2.5 * t / 0.05)) * (-t / 0.012).exp() + click) * gain
        })
        .collect()
}

/// A springy boing: a squish, then a tone swooping up with a quiver; higher for `k`.
fn boing(n: &mut Noise, k: f32) -> Vec<f32> {
    let f0 = (130.0 + 170.0 * k) * (0.95 + 0.1 * n.unit());
    let mut osc = Osc::default();
    let mut out: Vec<f32> = (0..len(0.38))
        .map(|i| {
            let t = time(i);
            let f = f0 * (1.0 + 0.9 * (1.0 - (-t / 0.06).exp())) * (1.0 + 0.04 * (t * 16.0 * std::f32::consts::TAU).sin());
            osc.step(f) * (t / 0.004).min(1.0) * (-t / 0.13).exp() * 0.5
        })
        .collect();
    let squish = splat(n, 0.15);
    mix_in(&mut out, &squish[..squish.len().min(len(0.05))], 0, 0.8);
    out
}

/// The big one: a crack, a deep falling thump and rumble, and then the goo pattering
/// and splatting back down all around.
fn boom(n: &mut Noise) -> Vec<f32> {
    let dur = 2.6;
    let (mut osc, mut hp, mut lp1, mut lp2) = (Osc::default(), Svf::default(), Svf::default(), Svf::default());
    let mut out: Vec<f32> = (0..len(dur))
        .map(|i| {
            let t = time(i);
            let x = n.next();
            let (_, _, high) = hp.step(x, 2000.0, 0.7);
            let crack = high * (-t / 0.012).exp() * 0.9;
            let thump = osc.step(28.0 + 75.0 * (-t / 0.09).exp()) * (-t / 0.45).exp() * 1.1;
            let (l1, _, _) = lp1.step(x, 180.0, 0.7);
            let (l2, _, _) = lp2.step(l1, 140.0, 0.7);
            let rumble = l2 * (1.0 - (-t / 0.02).exp()) * (-t / 0.8).exp() * 6.0;
            crack + thump + rumble
        })
        .collect();
    // The goo coming down: splats and droplets, thick at first, then thinning out.
    for _ in 0..30 {
        let at = 0.2 + 1.8 * n.unit().powf(1.6);
        let k = 0.08 + 0.3 * n.unit() * (1.0 - at / 2.2);
        let s = if n.unit() < 0.6 { splat(n, k) } else { plip(n, 0.2) };
        mix_in(&mut out, &s, len(at), 0.7);
    }
    out
}

/// A pin pushed in: a sharp tick and a little ring, into something soft.
fn thunk(n: &mut Noise) -> Vec<f32> {
    let (mut a, mut b, mut hp) = (Osc::default(), Osc::default(), Svf::default());
    let mut out: Vec<f32> = (0..len(0.15))
        .map(|i| {
            let t = time(i);
            let (_, _, high) = hp.step(n.next(), 3000.0, 0.7);
            let tick = high * (-t / 0.003).exp();
            let ring = (a.step(1250.0) + 0.6 * b.step(2900.0)) * (-t / 0.035).exp() * 0.25;
            tick + ring
        })
        .collect();
    let squish = splat(n, 0.25);
    mix_in(&mut out, &squish, len(0.01), 0.7);
    out
}

/// Goo slurping back together: bubbly noise sweeping up, fading in and out.
fn slurp(n: &mut Noise) -> Vec<f32> {
    let dur = 1.3;
    let mut svf = Svf::default();
    let mut wob = 0.5f32;
    (0..len(dur))
        .map(|i| {
            let t = time(i);
            let cut = 250.0 * (1500.0f32 / 250.0).powf(t / dur);
            let (_, band, _) = svf.step(n.next(), cut, 5.0);
            if i % 900 == 0 {
                wob = 0.2 + 0.8 * n.unit();
            }
            let env = (t / 0.25).min(1.0) * ((dur - t) / 0.3).clamp(0.0, 1.0);
            band * wob * env * 0.9
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real device: opens, plays its buffers through (silence, so nothing's heard)
    /// and hands each back. Ignored: needs a sound device.
    #[test]
    #[ignore]
    fn plays_through_the_device() {
        unsafe {
            let mut out = Out::open().expect("no sound device");
            let start = Instant::now();
            let mut written = 0;
            while written < BUFFERS * 3 {
                assert!(start.elapsed().as_secs() < 5, "the device stopped taking buffers after {written}");
                match out.free() {
                    Some(buf) => {
                        buf.fill(0);
                        out.write();
                        written += 1;
                    }
                    None => std::thread::sleep(Duration::from_millis(3)),
                }
            }
            eprintln!("SFX device took {written} buffers in {:?}", start.elapsed());
        }
    }
}

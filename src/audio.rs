//! Sound effects, synthesized in code (no sound files): footsteps and block sounds per
//! material, eating, pickups, hurting, and the animals. A tiny mixer plays them through
//! cpal, panned and attenuated by where they happen relative to the listener.
//!
//! Each sound is rendered once per variant (a few takes at slightly different pitches, so
//! repeated steps don't sound identical) and cached as mono samples at the device's rate.
//! See "Synthesis" below for how they are made.

use std::{
    f32::consts::{PI, TAU},
    sync::{Arc, Mutex, PoisonError},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glam::DVec3;

use crate::{mobs::MobKind, util::FastMap, world::block::Material};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Sound {
    Step(Material),
    /// A hit on a block being mined.
    Dig(Material),
    Break(Material),
    Place(Material),
    Eat,
    Burp,
    /// An item picked up.
    Pop,
    /// Putting on armor.
    Equip,
    /// The player getting hurt.
    Hurt,
    /// An animal's call.
    Mob(MobKind),
    MobHurt(MobKind),
    /// A rocket engine (one overlapping piece of its rumble, replayed while it burns).
    Engine,
    Explosion,
    /// Lightning's thunder, a long rolling rumble.
    Thunder,
}

/// Sounds that play continuously, at a changing volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ambience {
    Rain,
    Wind,
    /// A tornado.
    Roar,
}

const AMBIENCES: usize = 3;

/// Takes of each sound.
const VARIANTS: u32 = 4;
/// Sounds further away than this aren't heard (m).
const RANGE: f64 = 24.0;

struct Voice {
    samples: Arc<Vec<f32>>,
    position: usize,
    gain: [f32; 2],
}

/// A loop playing at a volume that follows its target smoothly.
struct Loop {
    samples: Arc<Vec<f32>>,
    position: usize,
    gain: f32,
    target: f32,
}

/// What the audio callback plays.
struct Mixer {
    voices: Vec<Voice>,
    loops: Vec<Loop>,
    /// Everything is multiplied by this.
    master: f32,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            voices: Vec::new(),
            loops: Vec::new(),
            master: 1.0,
        }
    }
}

pub struct Audio {
    /// None when there's no audio device (the game still runs, silently).
    stream: Option<cpal::Stream>,
    mixer: Arc<Mutex<Mixer>>,
    sample_rate: f32,
    cache: FastMap<(Sound, u32), Arc<Vec<f32>>>,
    rng: u64,
}

impl Audio {
    /// `enabled: false` stays silent (unattended captures shouldn't make noise).
    pub fn new(enabled: bool) -> Self {
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let opened = if enabled {
            open_stream(mixer.clone())
        } else {
            Err(anyhow::anyhow!("disabled"))
        };
        let (stream, sample_rate) = match opened {
            Ok((stream, rate)) => (Some(stream), rate),
            Err(e) => {
                log::warn!("no sound: {e}");
                (None, 48_000.0)
            }
        };
        if stream.is_some() {
            let loops = [Ambience::Rain, Ambience::Wind, Ambience::Roar].map(|kind| Loop {
                samples: Arc::new(ambience_loop(kind, sample_rate)),
                position: 0,
                gain: 0.0,
                target: 0.0,
            });
            mixer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .loops
                .extend(loops);
        }
        Self {
            stream,
            mixer,
            sample_rate,
            cache: FastMap::default(),
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// The volume setting, 0..1 (on a curve: halfway sounds about half as loud).
    pub fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.mixer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .master = volume * volume;
    }

    /// Sets how loud a continuous sound should be (0 = silent); it fades there by itself.
    pub fn set_ambience(&mut self, kind: Ambience, gain: f32) {
        if self.stream.is_none() {
            return;
        }
        let index = match kind {
            Ambience::Rain => 0,
            Ambience::Wind => 1,
            Ambience::Roar => 2,
        };
        debug_assert!(index < AMBIENCES);
        let mut mixer = self.mixer.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(l) = mixer.loops.get_mut(index) {
            l.target = gain.clamp(0.0, 1.0);
        }
    }

    /// Plays `sound` at `relative` (from the listener, m); `right` is the listener's right.
    pub fn play(&mut self, sound: Sound, relative: DVec3, right: DVec3) {
        self.play_gain(sound, relative, right, 1.0);
    }

    /// `play`, `gain` times louder (or quieter).
    pub fn play_gain(&mut self, sound: Sound, relative: DVec3, right: DVec3, gain: f32) {
        if self.stream.is_none() {
            return;
        }
        let distance = relative.length();
        let range = range(sound);
        if distance > range {
            return;
        }
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let variant = (self.rng >> 33) as u32 % VARIANTS;
        let rate = self.sample_rate;
        let samples = self
            .cache
            .entry((sound, variant))
            .or_insert_with(|| Arc::new(synthesize(sound, variant, rate)))
            .clone();
        let volume = (1.0 - distance / range).powi(2) as f32 * volume(sound) * gain;
        let pan = if distance > 0.5 {
            (relative.dot(right) / distance) as f32
        } else {
            0.0
        };
        // Never fully on one side: that sounds like a broken headphone.
        let pan = pan * 0.8;
        let gain = [
            volume * ((1.0 - pan) * 0.5).sqrt(),
            volume * ((1.0 + pan) * 0.5).sqrt(),
        ];
        let mut mixer = self.mixer.lock().unwrap_or_else(PoisonError::into_inner);
        if mixer.voices.len() < 48 {
            mixer.voices.push(Voice {
                samples,
                position: 0,
                gain,
            });
        }
    }
}

/// Loudness of each kind of sound (every sound is normalized to the same peak).
fn volume(sound: Sound) -> f32 {
    match sound {
        Sound::Step(_) => 0.2,
        Sound::Dig(_) => 0.35,
        Sound::Break(_) => 0.5,
        Sound::Place(_) => 0.42,
        Sound::Eat => 0.32,
        Sound::Burp => 0.3,
        Sound::Pop => 0.28,
        Sound::Equip => 0.38,
        Sound::Hurt => 0.45,
        Sound::Mob(_) => 0.42,
        Sound::MobHurt(_) => 0.5,
        Sound::Engine => 0.5,
        Sound::Explosion => 0.9,
        Sound::Thunder => 1.0,
    }
}

/// Loudness of each loop at full strength (RMS). Noise that never stops needs far less level
/// than a short sound to fill the mix, so these are measured as loudness, not by peak: wind
/// and rain together stay under a footstep (see `weather_stays_under_the_game`).
fn ambience_level(kind: Ambience) -> f32 {
    match kind {
        Ambience::Rain => 0.02,
        Ambience::Wind => 0.018,
        Ambience::Roar => 0.05,
    }
}

/// How far away a sound is heard (m).
fn range(sound: Sound) -> f64 {
    match sound {
        Sound::Engine => 600.0,
        Sound::Explosion => 1_500.0,
        Sound::Thunder => 14_000.0,
        _ => RANGE,
    }
}

fn open_stream(voices: Arc<Mutex<Mixer>>) -> anyhow::Result<(cpal::Stream, f32)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no output device"))?;
    let supported = device.default_output_config()?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let rate = config.sample_rate as f32;
    let error = |e| log::warn!("audio stream error: {e}");
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _| mix(&voices, data, channels, |x| x),
            error,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _| mix(&voices, data, channels, |x| (x * 32767.0) as i16),
            error,
            None,
        )?,
        other => anyhow::bail!("unsupported sample format {other:?}"),
    };
    stream.play()?;
    Ok((stream, rate))
}

/// Mixes the playing voices into an interleaved buffer. Loud moments are rounded off
/// (tanh) instead of clipping harshly.
fn mix<T: Copy>(mixer: &Mutex<Mixer>, data: &mut [T], channels: usize, convert: impl Fn(f32) -> T) {
    let mut mixer = mixer.lock().unwrap_or_else(PoisonError::into_inner);
    let Mixer {
        voices,
        loops,
        master,
    } = &mut *mixer;
    let master = *master;
    for frame in data.chunks_mut(channels) {
        let mut out = [0.0f32; 2];
        for l in loops.iter_mut() {
            // About a third of a second to reach a new level.
            l.gain += (l.target - l.gain) * 0.00012;
            if l.gain > 1e-4 {
                let s = l.samples[l.position] * l.gain;
                out[0] += s;
                out[1] += s;
            }
            l.position = (l.position + 1) % l.samples.len();
        }
        for v in voices.iter_mut() {
            if let Some(&s) = v.samples.get(v.position) {
                out[0] += s * v.gain[0];
                out[1] += s * v.gain[1];
                v.position += 1;
            }
        }
        for (c, sample) in frame.iter_mut().enumerate() {
            let x = if channels == 1 {
                (out[0] + out[1]) * 0.5
            } else {
                out[c.min(1)]
            };
            *sample = convert((x * master).tanh());
        }
    }
    voices.retain(|v| v.position < v.samples.len());
}

/// A long, seamless loop of weather noise (mono, at its `ambience_level`).
fn ambience_loop(kind: Ambience, rate: f32) -> Vec<f32> {
    let seconds = 7.0;
    let total = (seconds * rate) as usize;
    let fade = (0.6 * rate) as usize;
    let mut rng = 0x7a31_c2d5u32 ^ kind as u32;
    let mut noise = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    // One-pole low-pass coefficient for a cutoff in Hz.
    let k = |hz: f32| 1.0 - (-TAU * hz / rate).exp();
    let mut raw = vec![0.0f32; total + fade];
    let (mut a, mut b) = (0.0f32, 0.0f32);
    let mut swell = 0.0f32;
    for (i, out) in raw.iter_mut().enumerate() {
        let n = noise();
        let t = i as f32 / rate;
        *out = match kind {
            Ambience::Rain => {
                // A steady hiss with its top rolled off.
                a += (n - a) * k(3_400.0);
                b += (a - b) * k(420.0);
                a - b
            }
            Ambience::Wind => {
                // Low, breathing noise.
                a += (n - a) * k(620.0);
                b += (a - b) * k(120.0);
                swell += (noise().abs() - swell) * k(0.7);
                (a - b) * (0.45 + 1.3 * swell + 0.2 * (t * 1.3).sin())
            }
            Ambience::Roar => {
                // Deep and heavy.
                a += (n - a) * k(240.0);
                b += (a - b) * k(46.0);
                (a - b) * (0.8 + 0.2 * (t * 7.0).sin())
            }
        };
    }
    let rms = (raw.iter().map(|x| x * x).sum::<f32>() / raw.len() as f32).sqrt();
    let level = ambience_level(kind) / rms.max(1e-6);
    // The tail fades into the head, so the loop has no click.
    let mut out = Vec::with_capacity(total);
    for i in 0..total {
        let mut s = raw[i];
        if i < fade {
            let f = i as f32 / fade as f32;
            s = raw[i] * f + raw[total + i] * (1.0 - f);
        }
        out.push(s * level);
    }
    out
}

// ---- Synthesis ------------------------------------------------------------------------
//
// Sounds are built from a few physically inspired pieces, kept low and soft:
// - `thump`: a sine that drops quickly in pitch, the weight of an impact
// - `ring`: decaying inharmonic modes, how struck stone, wood or glass rings
// - `rustle`: band-passed noise, optionally in grains (crunch, crackle, rustle)
// - `voiced`: a glottal pulse train through vowel formants (animals, the player)
// Every piece is normalized before it is weighted, and every finished sound is high-passed
// (no rumble), gently low-passed (no hiss) and normalized. The first version was mostly bright
// filtered noise, 2-5 kHz, and sounded thin and high-pitched.

/// Noise, timing and the take's pitch for one sound.
struct Synth {
    rate: f32,
    rng: u32,
    /// Every frequency is scaled by this (each take is slightly higher or lower).
    pitch: f32,
}

impl Synth {
    /// White noise in -1..1.
    fn noise(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Uniform in 0..1.
    fn random(&mut self) -> f32 {
        (self.noise() + 1.0) * 0.5
    }

    fn len(&self, seconds: f32) -> usize {
        (seconds * self.rate) as usize
    }

    fn time(&self, i: usize) -> f32 {
        i as f32 / self.rate
    }
}

#[derive(Clone, Copy)]
enum Pass {
    Low,
    High,
    Band,
}

/// RBJ biquad filter (the band-pass has 0 dB peak gain).
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn new(pass: Pass, freq: f32, q: f32, rate: f32) -> Self {
        let mut f = Self {
            b: [0.0; 3],
            a: [0.0; 2],
            x: [0.0; 2],
            y: [0.0; 2],
        };
        f.tune(pass, freq, q, rate);
        f
    }

    /// Changes the response, keeping the state (for sweeping formants).
    fn tune(&mut self, pass: Pass, freq: f32, q: f32, rate: f32) {
        let w = TAU * freq.clamp(10.0, rate * 0.45) / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b = match pass {
            Pass::Low => [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            Pass::High => [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            Pass::Band => [alpha, 0.0, -alpha],
        };
        self.b = b.map(|v| v / a0);
        self.a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

fn filter(samples: &mut [f32], pass: Pass, freq: f32, rate: f32) {
    let mut f = Biquad::new(pass, freq, std::f32::consts::FRAC_1_SQRT_2, rate);
    for s in samples {
        *s = f.run(*s);
    }
}

/// Scales to a peak of `peak`.
fn normalize(mut samples: Vec<f32>, peak: f32) -> Vec<f32> {
    let max = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if max > 1e-9 {
        for s in &mut samples {
            *s *= peak / max;
        }
    }
    samples
}

/// Adds `part` (normalized, times `gain`) into `out`, starting `seconds` in.
fn add(out: &mut Vec<f32>, part: Vec<f32>, seconds: f32, gain: f32, rate: f32) {
    let start = (seconds * rate) as usize;
    let part = normalize(part, gain);
    if out.len() < start + part.len() {
        out.resize(start + part.len(), 0.0);
    }
    for (o, p) in out[start..].iter_mut().zip(part) {
        *o += p;
    }
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The weight of an impact: a sine starting about twice `freq` and settling onto it.
fn thump(s: &mut Synth, freq: f32, decay: f32) -> Vec<f32> {
    let freq = freq * s.pitch;
    let mut phase = 0.0f32;
    (0..s.len(decay * 7.0))
        .map(|i| {
            let t = s.time(i);
            let f = freq * (1.0 + 1.2 * (-t / 0.01).exp());
            phase = (phase + f / s.rate).fract();
            (phase * TAU).sin() * (t / 0.0015).min(1.0) * (-t / decay).exp()
        })
        .collect()
}

/// A struck solid ringing: modes as (frequency, decay, amplitude).
fn ring(s: &mut Synth, modes: &[(f32, f32, f32)]) -> Vec<f32> {
    let longest = modes.iter().map(|m| m.1).fold(0.0, f32::max);
    let mut out = vec![0.0; s.len(longest * 7.0)];
    for &(freq, decay, amp) in modes {
        let freq = freq * s.pitch * (1.0 + s.noise() * 0.02);
        let start = s.random() * TAU;
        for (i, o) in out.iter_mut().enumerate() {
            let t = i as f32 / s.rate;
            *o += amp * (TAU * freq * t + start).sin() * (-t / decay).exp() * (t / 0.001).min(1.0);
        }
    }
    out
}

/// Noise band-passed around `center` (Hz, with `q`) over `seconds`, fading with `decay`.
/// With `grains` (bursts per second) it crunches and crackles instead of hissing.
fn rustle(s: &mut Synth, seconds: f32, center: f32, q: f32, grains: f32, decay: f32) -> Vec<f32> {
    let mut band = Biquad::new(Pass::Band, center * s.pitch, q, s.rate);
    let fade = (-1.0 / (0.005 * s.rate)).exp();
    let mut grain = 1.0f32;
    (0..s.len(seconds))
        .map(|i| {
            let t = s.time(i);
            if grains > 0.0 {
                grain *= fade;
                if s.random() < grains / s.rate {
                    grain = 0.4 + s.random() * 0.6;
                }
            }
            band.run(s.noise()) * grain * (t / 0.003).min(1.0) * (-t / decay).exp()
        })
        .collect()
}

/// A voice: glottal pulses at `pitch(t)` Hz through three formants `formants(t)` (Hz), shaped
/// by `amp(t)`. `breath` adds air, `rough` a growl (subharmonic).
fn voiced(
    s: &mut Synth,
    seconds: f32,
    pitch: impl Fn(f32) -> f32,
    formants: impl Fn(f32) -> [f32; 3],
    amp: impl Fn(f32) -> f32,
    breath: f32,
    rough: f32,
) -> Vec<f32> {
    const GAINS: [f32; 3] = [1.0, 0.55, 0.25];
    const Q: [f32; 3] = [4.0, 6.0, 8.0];
    let rate = s.rate;
    let first = formants(0.0);
    let mut bands: [Biquad; 3] =
        std::array::from_fn(|j| Biquad::new(Pass::Band, first[j], Q[j], rate));
    let (mut phase, mut sub, mut wobble, mut last_flow, mut chest) =
        (0.0f32, 0.0f32, 0.0, 0.0, 0.0);
    let chest_k = 1.0 - (-TAU * 350.0 / rate).exp();
    (0..s.len(seconds))
        .map(|i| {
            let t = s.time(i);
            // A little natural unsteadiness in pitch.
            wobble += (s.noise() - wobble) * 0.0015;
            let f0 = (pitch(t) * s.pitch * (1.0 + 0.6 * wobble)).max(20.0);
            phase += f0 / rate;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            sub = (sub + 0.5 * f0 / rate).fract();
            // Rosenberg glottal flow; its slope is what the mouth radiates.
            let flow = if phase < 0.6 {
                0.5 * (1.0 - (PI * phase / 0.6).cos())
            } else if phase < 0.95 {
                (0.5 * PI * (phase - 0.6) / 0.35).cos()
            } else {
                0.0
            };
            let mut source = (flow - last_flow) * rate / f0 * 0.25;
            last_flow = flow;
            source *= 1.0 - rough * 0.5 * (1.0 + (sub * TAU).sin());
            source += s.noise() * breath;
            if i % 32 == 0 {
                let f = formants(t);
                for (j, band) in bands.iter_mut().enumerate() {
                    band.tune(Pass::Band, f[j], Q[j], rate);
                }
            }
            let mut y = 0.0;
            for (band, gain) in bands.iter_mut().zip(GAINS) {
                y += band.run(source) * gain;
            }
            chest += (source - chest) * chest_k;
            (y + chest * 0.4) * amp(t)
        })
        .collect()
}

/// Rises over `attack` and falls over the last `release` of `seconds`.
fn envelope(attack: f32, release: f32, seconds: f32) -> impl Fn(f32) -> f32 {
    move |t| (t / attack).min(1.0) * ((seconds - t) / release).clamp(0.0, 1.0)
}

/// One impact on a material, `strength` 0..1 (a step is light, placing a block heavy).
fn impact(s: &mut Synth, m: Material, strength: f32) -> Vec<f32> {
    let k = 0.6 + 0.4 * strength;
    let rate = s.rate;
    let mut out = Vec::new();
    let body = 0.4 + 0.6 * strength;
    let cutoff = match m {
        Material::Grass => {
            add(&mut out, thump(s, 120.0, 0.02 * k), 0.0, 0.6 * body, rate);
            let rustling = rustle(s, 0.16 * k, 1000.0, 0.7, 260.0, 0.045 * k);
            add(&mut out, rustling, 0.0, 0.7, rate);
            2800.0
        }
        Material::Dirt => {
            add(&mut out, thump(s, 105.0, 0.03 * k), 0.0, 0.75 * body, rate);
            let crumbs = rustle(s, 0.12 * k, 450.0, 0.6, 140.0, 0.03 * k);
            add(&mut out, crumbs, 0.0, 0.6, rate);
            1600.0
        }
        Material::Stone => {
            add(&mut out, thump(s, 140.0, 0.02 * k), 0.0, 0.7 * body, rate);
            let modes = [
                (320.0, 0.022 * k, 0.5),
                (760.0, 0.014 * k, 0.35),
                (1420.0, 0.009 * k, 0.2),
            ];
            add(&mut out, ring(s, &modes), 0.0, 0.9, rate);
            add(
                &mut out,
                rustle(s, 0.05, 1800.0, 1.0, 0.0, 0.008),
                0.0,
                0.3,
                rate,
            );
            4500.0
        }
        Material::Wood => {
            add(&mut out, thump(s, 130.0, 0.03 * k), 0.0, 0.6 * body, rate);
            let modes = [
                (185.0, 0.05 * k, 0.7),
                (410.0, 0.03 * k, 0.45),
                (820.0, 0.018 * k, 0.2),
            ];
            add(&mut out, ring(s, &modes), 0.0, 1.0, rate);
            add(
                &mut out,
                rustle(s, 0.05, 900.0, 0.9, 0.0, 0.012),
                0.0,
                0.3,
                rate,
            );
            3000.0
        }
        Material::Sand => {
            add(&mut out, thump(s, 100.0, 0.02 * k), 0.0, 0.5 * body, rate);
            let hiss = rustle(s, 0.2 * k, 1300.0, 0.5, 900.0, 0.06 * k);
            add(&mut out, hiss, 0.0, 0.7, rate);
            2400.0
        }
        Material::Gravel => {
            add(&mut out, thump(s, 110.0, 0.025 * k), 0.0, 0.6 * body, rate);
            let crunch = rustle(s, 0.18 * k, 1100.0, 0.9, 1400.0, 0.05 * k);
            add(&mut out, crunch, 0.0, 0.9, rate);
            3000.0
        }
        Material::Snow => {
            add(&mut out, thump(s, 100.0, 0.025 * k), 0.0, 0.5 * body, rate);
            let crunch = rustle(s, 0.18 * k, 800.0, 1.3, 700.0, 0.06 * k);
            add(&mut out, crunch, 0.0, 0.8, rate);
            2200.0
        }
        Material::Glass => {
            add(&mut out, thump(s, 170.0, 0.015 * k), 0.0, 0.5 * body, rate);
            let modes = [
                (1700.0, 0.05 * k, 0.3),
                (2450.0, 0.035 * k, 0.22),
                (3300.0, 0.025 * k, 0.12),
            ];
            add(&mut out, ring(s, &modes), 0.0, 0.6, rate);
            6000.0
        }
        Material::Wool => {
            add(&mut out, thump(s, 95.0, 0.04 * k), 0.0, 0.6 * body, rate);
            let fluff = rustle(s, 0.12 * k, 380.0, 0.6, 0.0, 0.035 * k);
            add(&mut out, fluff, 0.0, 0.5, rate);
            900.0
        }
        Material::Leaves => {
            add(&mut out, thump(s, 120.0, 0.015 * k), 0.0, 0.25 * body, rate);
            let rustling = rustle(s, 0.22 * k, 1100.0, 0.6, 450.0, 0.08 * k);
            add(&mut out, rustling, 0.0, 0.7, rate);
            2400.0
        }
    };
    filter(&mut out, Pass::Low, cutoff, rate);
    out
}

/// A block coming apart: a heavy impact, then smaller pieces settling.
fn shatter(s: &mut Synth, m: Material) -> Vec<f32> {
    let rate = s.rate;
    let mut out = Vec::new();
    add(&mut out, impact(s, m, 1.0), 0.0, 1.0, rate);
    let pieces = 4 + (s.random() * 3.0) as usize;
    let take = s.pitch;
    for i in 0..pieces {
        let at = 0.04 + s.random() * 0.2;
        // Smaller pieces sound a little higher, and later ones are quieter.
        s.pitch = take * (1.0 + s.random() * 0.25);
        let gain = (0.45 - i as f32 * 0.05).max(0.15) * (0.6 + s.random() * 0.4);
        add(&mut out, impact(s, m, 0.2), at, gain, rate);
    }
    s.pitch = take;
    if m == Material::Glass {
        for _ in 0..10 {
            let f = 1800.0 + s.random() * 2200.0;
            let decay = 0.02 + s.random() * 0.04;
            let tinkle = ring(s, &[(f, decay, 1.0)]);
            let at = s.random() * 0.25;
            add(&mut out, tinkle, at, 0.15, rate);
        }
    }
    out
}

fn synthesize(sound: Sound, variant: u32, rate: f32) -> Vec<f32> {
    let mut synth = Synth {
        rate,
        rng: 0x1234_5678 ^ (variant.wrapping_mul(0x9e37_79b9)) ^ (rate as u32),
        pitch: 1.0 + (variant as f32 - 1.5) * 0.035,
    };
    let s = &mut synth;
    let mut out = match sound {
        Sound::Thunder => {
            // A sharp first crack and then rolling booms, each dying away: low-passed noise
            // shaped by a handful of random bursts.
            let len = s.len(6.0);
            let mut bursts = vec![(0.0f32, 1.0f32, 0.35f32)];
            for _ in 0..7 {
                let onset = 0.1 + s.random() * 3.6;
                let strength = 0.3 + s.random() * 0.6;
                let decay = 0.35 + s.random() * 1.1;
                bursts.push((onset, strength, decay));
            }
            let (mut a, mut b) = (0.0f32, 0.0f32);
            let ka = 1.0 - (-TAU * 240.0 / rate).exp();
            let kb = 1.0 - (-TAU * 90.0 / rate).exp();
            let mut out = Vec::with_capacity(len);
            for i in 0..len {
                let t = i as f32 / rate;
                let n = s.noise();
                a += (n - a) * ka;
                b += (a - b) * kb;
                let mut env = 0.0;
                for &(onset, strength, decay) in &bursts {
                    if t >= onset {
                        env += strength * (-(t - onset) / decay).exp();
                    }
                }
                let crack = if t < 0.12 {
                    n * (1.0 - t / 0.12) * 0.25
                } else {
                    0.0
                };
                let tail = (1.0 - t / 6.0).max(0.0);
                out.push((b * env * 4.0 + crack) * tail.sqrt());
            }
            out
        }
        Sound::Step(m) => {
            // Footsteps are light and dull.
            let mut out = impact(s, m, 0.35);
            filter(&mut out, Pass::Low, 2000.0, rate);
            out
        }
        Sound::Dig(m) => impact(s, m, 0.7),
        Sound::Place(m) => impact(s, m, 1.0),
        Sound::Break(m) => shatter(s, m),
        Sound::Eat => {
            // A chomp, and the crunch after it.
            let mut out = Vec::new();
            let chomp = rustle(s, 0.14, 850.0, 0.9, 1100.0, 0.035);
            add(&mut out, chomp, 0.0, 0.8, rate);
            add(&mut out, thump(s, 140.0, 0.02), 0.0, 0.5, rate);
            let crunch = rustle(s, 0.08, 1200.0, 1.0, 900.0, 0.02);
            add(&mut out, crunch, 0.05, 0.35, rate);
            filter(&mut out, Pass::Low, 2600.0, rate);
            out
        }
        Sound::Burp => voiced(
            s,
            0.38,
            |t| 78.0 - t * 30.0,
            |_| [380.0, 760.0, 2200.0],
            envelope(0.03, 0.15, 0.38),
            0.08,
            0.7,
        ),
        Sound::Pop => {
            // A soft, round blip that rises a little.
            let mut phase = 0.0f32;
            let pitch = s.pitch;
            (0..s.len(0.12))
                .map(|i| {
                    let t = i as f32 / rate;
                    phase += 400.0 * pitch * (1.0 + 0.5 * (1.0 - (-t / 0.03).exp())) / rate;
                    let wave = (phase * TAU).sin() + 0.12 * (phase * 2.0 * TAU).sin();
                    wave * (t / 0.003).min(1.0) * (-t / 0.045).exp()
                })
                .collect()
        }
        Sound::Equip => {
            // Cloth and leather shuffling, then a clasp.
            let mut out = Vec::new();
            let shuffle = rustle(s, 0.2, 650.0, 0.7, 350.0, 0.06);
            add(&mut out, shuffle, 0.0, 0.8, rate);
            let clasp = ring(s, &[(1100.0, 0.015, 0.4), (2300.0, 0.008, 0.2)]);
            add(&mut out, clasp, 0.07, 0.45, rate);
            filter(&mut out, Pass::Low, 3500.0, rate);
            out
        }
        Sound::Hurt => {
            // A short "oof" with a thud.
            let mut out = Vec::new();
            let oof = voiced(
                s,
                0.24,
                |t| 150.0 - t * 180.0,
                |t| [480.0 - t * 300.0, 950.0, 2400.0],
                envelope(0.008, 0.12, 0.24),
                0.25,
                0.15,
            );
            add(&mut out, oof, 0.0, 1.0, rate);
            add(&mut out, thump(s, 90.0, 0.04), 0.0, 0.5, rate);
            out
        }
        Sound::Engine => {
            // Low rumble and crackle, fading in and out so the replays overlap smoothly.
            let mut out = Vec::new();
            let len = 0.7;
            let rumble = rustle(s, len, 90.0, 0.5, 0.0, 10.0);
            add(&mut out, rumble, 0.0, 1.0, rate);
            let roar = rustle(s, len, 420.0, 0.6, 0.0, 10.0);
            add(&mut out, roar, 0.0, 0.45, rate);
            let crackle = rustle(s, len, 900.0, 1.0, 600.0, 10.0);
            add(&mut out, crackle, 0.0, 0.25, rate);
            let shape = envelope(0.2, 0.25, len);
            for (i, x) in out.iter_mut().enumerate() {
                *x *= shape(i as f32 / rate);
            }
            filter(&mut out, Pass::Low, 1800.0, rate);
            out
        }
        Sound::Explosion => {
            let mut out = Vec::new();
            add(&mut out, thump(s, 55.0, 0.25), 0.0, 1.0, rate);
            add(
                &mut out,
                rustle(s, 1.6, 220.0, 0.5, 0.0, 0.45),
                0.0,
                0.9,
                rate,
            );
            add(
                &mut out,
                rustle(s, 1.2, 800.0, 0.8, 500.0, 0.3),
                0.02,
                0.4,
                rate,
            );
            filter(&mut out, Pass::Low, 2500.0, rate);
            out
        }
        Sound::Mob(kind) => call(s, kind, false),
        Sound::MobHurt(kind) => call(s, kind, true),
    };
    // Finish: no rumble, no hiss, no click at the end.
    filter(&mut out, Pass::High, 45.0, rate);
    filter(&mut out, Pass::Low, 7000.0, rate);
    let tail = (0.004 * rate) as usize;
    let n = out.len();
    for (i, x) in out.iter_mut().enumerate().skip(n.saturating_sub(tail)) {
        *x *= (n - i) as f32 / tail as f32;
    }
    normalize(out, 0.9)
}

/// An animal's call, or its cry when hurt.
fn call(s: &mut Synth, kind: MobKind, hurt: bool) -> Vec<f32> {
    let rate = s.rate;
    match kind {
        // A low moo: the mouth opens from "m" to "oo" and closes again, the pitch rises a
        // little and sinks. Hurt: shorter, higher and more open.
        MobKind::Cow => {
            let len = if hurt { 0.5 } else { 1.25 + s.random() * 0.3 };
            let rise = 0.3;
            voiced(
                s,
                len,
                move |t| {
                    if hurt {
                        150.0 - 40.0 * t / len
                    } else {
                        92.0 + 22.0 * smooth(t / rise) - 26.0 * smooth((t - rise) / (len - rise))
                    }
                },
                move |t| {
                    let open = (PI * (t / len).clamp(0.0, 1.0)).sin().powf(0.7);
                    let open = if hurt { 0.4 + 0.6 * open } else { open };
                    [250.0 + 280.0 * open, 650.0 + 350.0 * open, 2300.0]
                },
                envelope(if hurt { 0.03 } else { 0.12 }, 0.3, len),
                0.06,
                if hurt { 0.45 } else { 0.3 },
            )
        }
        // Two or three short, rough, nasal grunts with a snort. Hurt: a squeal.
        MobKind::Pig => {
            if hurt {
                return voiced(
                    s,
                    0.4,
                    |t| (300.0 - t * 100.0) * (1.0 + 0.04 * (TAU * 6.0 * t).sin()),
                    |_| [850.0, 1500.0, 2600.0],
                    envelope(0.02, 0.15, 0.4),
                    0.12,
                    0.15,
                );
            }
            let mut out = Vec::new();
            let grunts = 2 + (s.random() * 2.0) as usize;
            for g in 0..grunts {
                let len = 0.11 + s.random() * 0.05;
                let grunt = voiced(
                    s,
                    len,
                    |t| 115.0 - t * 120.0,
                    |_| [480.0, 1150.0, 2400.0],
                    envelope(0.012, 0.06, len),
                    0.3,
                    0.6,
                );
                let mut grunt = normalize(grunt, 1.0);
                let snort = rustle(s, len, 650.0, 1.4, 0.0, 0.05);
                add(&mut grunt, snort, 0.0, 0.35, rate);
                add(
                    &mut out,
                    grunt,
                    g as f32 * 0.19,
                    1.0 - g as f32 * 0.15,
                    rate,
                );
            }
            out
        }
        // A bleat: "m" opening to "eh", with the quaver sheep have. Hurt: shorter and higher.
        MobKind::Sheep => {
            let len = if hurt { 0.4 } else { 0.75 + s.random() * 0.2 };
            let quaver = if hurt { 9.0 } else { 6.5 };
            let base = if hurt { 230.0 } else { 185.0 };
            let shape = envelope(0.03, 0.2, len);
            voiced(
                s,
                len,
                move |t| (base - 25.0 * t / len) * (1.0 + 0.05 * (TAU * quaver * t).sin()),
                |t| {
                    let open = smooth(t / 0.07);
                    [300.0 + 350.0 * open, 1000.0 + 600.0 * open, 2500.0]
                },
                move |t| shape(t) * (1.0 - 0.35 * (0.5 + 0.5 * (TAU * quaver * t).sin())),
                0.1,
                0.12,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATERIALS: [Material; 10] = [
        Material::Grass,
        Material::Dirt,
        Material::Stone,
        Material::Wood,
        Material::Sand,
        Material::Gravel,
        Material::Snow,
        Material::Glass,
        Material::Wool,
        Material::Leaves,
    ];

    fn all() -> Vec<Sound> {
        let mut all = vec![
            Sound::Eat,
            Sound::Burp,
            Sound::Pop,
            Sound::Equip,
            Sound::Hurt,
            Sound::Engine,
            Sound::Thunder,
            Sound::Explosion,
        ];
        for m in MATERIALS {
            all.extend([
                Sound::Step(m),
                Sound::Dig(m),
                Sound::Break(m),
                Sound::Place(m),
            ]);
        }
        for k in MobKind::ALL {
            all.extend([Sound::Mob(k), Sound::MobHurt(k)]);
        }
        all
    }

    /// The RMS frequency (Hz): how bright a sound is, from the energy of its slope.
    fn brightness(samples: &[f32], rate: f32) -> f32 {
        let energy: f32 = samples.iter().map(|x| x * x).sum();
        let slope: f32 = samples.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
        rate / TAU * (slope / energy).sqrt()
    }

    /// The loudest 50 ms of a sound (RMS): how loud it is while it is actually sounding.
    fn loudness(samples: &[f32], rate: f32) -> f32 {
        let window = (0.05 * rate) as usize;
        samples
            .chunks(window / 2)
            .zip(samples.chunks(window / 2).skip(1))
            .map(|(a, b)| {
                let energy: f32 = a.iter().chain(b).map(|x| x * x).sum();
                (energy / (a.len() + b.len()) as f32).sqrt()
            })
            .fold(0.0, f32::max)
    }

    /// A full storm must not drown out the player's own footsteps, nor a tornado being hurt.
    #[test]
    fn weather_stays_under_the_game() {
        let rate = 48_000.0;
        let heard = |sound| {
            (0..VARIANTS)
                .map(|v| loudness(&synthesize(sound, v, rate), rate) * volume(sound))
                .fold(f32::MAX, f32::min)
        };
        let storm = [Ambience::Rain, Ambience::Wind]
            .map(|kind| loudness(&ambience_loop(kind, rate), rate).powi(2))
            .iter()
            .sum::<f32>()
            .sqrt();
        let step = heard(Sound::Step(Material::Stone));
        assert!(storm < step, "storm {storm} against a step {step}");
        let roar = loudness(&ambience_loop(Ambience::Roar, rate), rate);
        let hurt = heard(Sound::Hurt);
        assert!(roar < hurt * 0.5, "roar {roar} against being hurt {hurt}");
    }

    #[test]
    fn sounds_are_short_soft_and_in_range() {
        for sound in all() {
            for variant in 0..VARIANTS {
                let samples = synthesize(sound, variant, 48_000.0);
                let longest = if sound == Sound::Thunder { 7 } else { 2 };
                assert!(
                    !samples.is_empty() && samples.len() < 48_000 * longest,
                    "{sound:?}"
                );
                let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                assert!(
                    peak.is_finite() && (0.5..=0.91).contains(&peak),
                    "{sound:?} peak {peak}"
                );
                // Nothing shrill (the first version's block sounds were 1.8-4.7 kHz). Glass
                // is allowed its ring.
                let glass = matches!(
                    sound,
                    Sound::Step(Material::Glass)
                        | Sound::Dig(Material::Glass)
                        | Sound::Break(Material::Glass)
                        | Sound::Place(Material::Glass)
                );
                let limit = if glass { 3000.0 } else { 1600.0 };
                let b = brightness(&samples, 48_000.0);
                assert!(b < limit, "{sound:?} is {b:.0} Hz bright");
            }
        }
    }

    /// Writes every sound as a WAV file into `target/sounds` to listen to:
    /// `cargo test --release write_sounds -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn write_sounds() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/sounds");
        std::fs::create_dir_all(&dir).unwrap();
        for sound in all() {
            for variant in 0..2 {
                let rate = 48_000u32;
                let samples = synthesize(sound, variant, rate as f32);
                let data = (samples.len() * 2) as u32;
                let mut wav = Vec::new();
                wav.extend(b"RIFF");
                wav.extend((36 + data).to_le_bytes());
                wav.extend(b"WAVEfmt ");
                wav.extend(16u32.to_le_bytes());
                wav.extend(1u16.to_le_bytes()); // PCM
                wav.extend(1u16.to_le_bytes()); // mono
                wav.extend(rate.to_le_bytes());
                wav.extend((rate * 2).to_le_bytes());
                wav.extend(2u16.to_le_bytes());
                wav.extend(16u16.to_le_bytes());
                wav.extend(b"data");
                wav.extend(data.to_le_bytes());
                for x in &samples {
                    wav.extend(((x * 0.8 * 32767.0) as i16).to_le_bytes());
                }
                let name = format!("{sound:?}-{variant}.wav")
                    .replace('(', "_")
                    .replace(')', "");
                std::fs::write(dir.join(name), wav).unwrap();
                println!(
                    "{sound:?} {variant}: {:.2} s, {:.0} Hz bright",
                    samples.len() as f32 / rate as f32,
                    brightness(&samples, rate as f32)
                );
            }
        }
        println!("written to {}", dir.display());
    }
}

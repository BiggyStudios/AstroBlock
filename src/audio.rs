//! Sound effects, synthesized in code (no sound files): footsteps and block sounds per
//! material, eating, pickups, hurting, and the animals. A tiny mixer plays them through
//! cpal, panned and attenuated by where they happen relative to the listener.
//!
//! Each sound is rendered once per variant (a few random takes, so repeated steps don't
//! sound identical) and cached as mono samples at the device's rate.

use std::sync::{Arc, Mutex, PoisonError};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glam::DVec3;

use crate::{mobs::MobKind, util::FastMap, world::block::Material};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sound {
    Step(Material),
    Break(Material),
    Place(Material),
    Eat,
    Burp,
    /// An item picked up.
    Pop,
    /// The player getting hurt.
    Hurt,
    /// An animal's call.
    Mob(MobKind),
    MobHurt(MobKind),
}

/// Takes of each sound.
const VARIANTS: u32 = 4;
/// Sounds further away than this aren't heard (m).
const RANGE: f64 = 24.0;

struct Voice {
    samples: Arc<Vec<f32>>,
    position: usize,
    gain: [f32; 2],
}

pub struct Audio {
    /// None when there's no audio device (the game still runs, silently).
    stream: Option<cpal::Stream>,
    voices: Arc<Mutex<Vec<Voice>>>,
    sample_rate: f32,
    cache: FastMap<(Sound, u32), Arc<Vec<f32>>>,
    rng: u64,
}

impl Audio {
    /// `enabled: false` stays silent (unattended captures shouldn't make noise).
    pub fn new(enabled: bool) -> Self {
        let voices = Arc::new(Mutex::new(Vec::<Voice>::new()));
        let opened = if enabled {
            open_stream(voices.clone())
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
        Self {
            stream,
            voices,
            sample_rate,
            cache: FastMap::default(),
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// Plays `sound` at `relative` (from the listener, m); `right` is the listener's right.
    pub fn play(&mut self, sound: Sound, relative: DVec3, right: DVec3) {
        if self.stream.is_none() {
            return;
        }
        let distance = relative.length();
        if distance > RANGE {
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
        let volume = (1.0 - distance / RANGE).powi(2) as f32 * volume(sound);
        let pan = if distance > 0.5 {
            (relative.dot(right) / distance) as f32
        } else {
            0.0
        };
        let gain = [
            volume * ((1.0 - pan) * 0.5).sqrt(),
            volume * ((1.0 + pan) * 0.5).sqrt(),
        ];
        let mut voices = self.voices.lock().unwrap_or_else(PoisonError::into_inner);
        if voices.len() < 48 {
            voices.push(Voice {
                samples,
                position: 0,
                gain,
            });
        }
    }
}

/// Loudness of each kind of sound.
fn volume(sound: Sound) -> f32 {
    match sound {
        Sound::Step(_) => 0.35,
        Sound::Break(_) => 0.8,
        Sound::Place(_) => 0.6,
        Sound::Eat => 0.5,
        Sound::Burp | Sound::Pop => 0.4,
        Sound::Hurt => 0.7,
        Sound::Mob(_) => 0.6,
        Sound::MobHurt(_) => 0.8,
    }
}

fn open_stream(voices: Arc<Mutex<Vec<Voice>>>) -> anyhow::Result<(cpal::Stream, f32)> {
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

/// Mixes the playing voices into an interleaved buffer.
fn mix<T: Copy>(
    voices: &Mutex<Vec<Voice>>,
    data: &mut [T],
    channels: usize,
    convert: impl Fn(f32) -> T,
) {
    let mut voices = voices.lock().unwrap_or_else(PoisonError::into_inner);
    for frame in data.chunks_mut(channels) {
        let mut out = [0.0f32; 2];
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
            *sample = convert(x.clamp(-1.0, 1.0));
        }
    }
    voices.retain(|v| v.position < v.samples.len());
}

// ---- Synthesis ------------------------------------------------------------------------

/// A little toolbox for one sound: noise, oscillators and filters at a sample rate.
struct Synth {
    rate: f32,
    rng: u32,
}

impl Synth {
    fn noise(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn len(&self, seconds: f32) -> usize {
        (seconds * self.rate) as usize
    }
}

/// One-pole low-pass filter coefficient for a cutoff (Hz).
fn lp_coefficient(cutoff: f32, rate: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * cutoff / rate).exp()
}

/// RBJ biquad band-pass (formants, resonances).
struct BandPass {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl BandPass {
    fn new(freq: f32, q: f32, rate: f32) -> Self {
        let w = std::f32::consts::TAU * freq / rate;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b: [alpha / a0, 0.0, -alpha / a0],
            a: [-2.0 * w.cos() / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn set(&mut self, freq: f32, q: f32, rate: f32) {
        let fresh = BandPass::new(freq, q, rate);
        self.b = fresh.b;
        self.a = fresh.a;
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

/// Filtered noise in grains: the basis of footsteps and block sounds. `cutoff` shapes the
/// tone (low: soft, high: crisp), `grains` per second makes it crunchy.
fn noise_burst(
    synth: &mut Synth,
    seconds: f32,
    decay: f32,
    cutoff: f32,
    grains: f32,
    knock: Option<f32>,
) -> Vec<f32> {
    let n = synth.len(seconds);
    let k = lp_coefficient(cutoff, synth.rate);
    let (mut lp, mut lp2) = (0.0f32, 0.0f32);
    let mut grain = 1.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / synth.rate;
            if grains > 0.0 && synth.noise().abs() < grains / synth.rate {
                grain = 0.4 + synth.noise().abs() * 1.4;
            }
            grain += (1.0 - grain) * 0.002;
            lp += (synth.noise() - lp) * k;
            lp2 += (lp - lp2) * k;
            let env = (t / 0.004).min(1.0) * (-t / decay).exp();
            let tone = knock.map_or(0.0, |f| {
                (std::f32::consts::TAU * f * t).sin() * (-t / (decay * 0.6)).exp() * 0.8
            });
            (lp2 * 2.5 * grain + tone) * env
        })
        .collect()
}

/// A voiced sound (animal calls, the hurt sound): a buzzy oscillator through a formant
/// filter. `pitch(t)` in Hz, `formant(t)` in Hz, `amp(t)` envelope, over `seconds`.
fn voice(
    synth: &mut Synth,
    seconds: f32,
    pitch: impl Fn(f32) -> f32,
    formant: impl Fn(f32) -> f32,
    amp: impl Fn(f32) -> f32,
) -> Vec<f32> {
    let n = synth.len(seconds);
    let mut phase = 0.0f32;
    let mut bp = BandPass::new(formant(0.0), 3.0, synth.rate);
    let mut low = 0.0f32;
    let k = lp_coefficient(1400.0, synth.rate);
    (0..n)
        .map(|i| {
            let t = i as f32 / synth.rate;
            phase = (phase + pitch(t) / synth.rate).fract();
            let saw = phase * 2.0 - 1.0 + synth.noise() * 0.05;
            if i % 64 == 0 {
                bp.set(formant(t), 3.0, synth.rate);
            }
            low += (saw - low) * k;
            (bp.run(saw) * 1.8 + low * 0.5) * amp(t)
        })
        .collect()
}

fn synthesize(sound: Sound, variant: u32, rate: f32) -> Vec<f32> {
    let mut synth = Synth {
        rate,
        rng: 0x1234_5678 ^ (variant.wrapping_mul(0x9e37_79b9)) ^ (rate as u32),
    };
    // Each take is slightly higher or lower.
    let detune = 1.0 + (variant as f32 - 1.5) * 0.04;
    let s = &mut synth;
    let swell = |attack: f32, release: f32, total: f32| {
        move |t: f32| (t / attack).min(1.0) * ((total - t) / release).clamp(0.0, 1.0)
    };
    match sound {
        Sound::Step(m) | Sound::Break(m) | Sound::Place(m) => {
            // (length, decay, cutoff, grains/s, tonal knock)
            let (len, decay, cutoff, grains, knock) = match m {
                Material::Grass => (0.14, 0.035, 1800.0, 300.0, None),
                Material::Leaves => (0.16, 0.04, 4000.0, 500.0, None),
                Material::Dirt => (0.12, 0.03, 900.0, 120.0, None),
                Material::Stone => (0.09, 0.018, 3200.0, 0.0, Some(190.0)),
                Material::Wood => (0.12, 0.03, 1500.0, 0.0, Some(240.0)),
                Material::Sand => (0.16, 0.05, 5000.0, 60.0, None),
                Material::Gravel => (0.16, 0.04, 2600.0, 700.0, None),
                Material::Snow => (0.16, 0.045, 2000.0, 400.0, None),
                Material::Glass => (0.12, 0.03, 6000.0, 0.0, Some(2600.0)),
                Material::Wool => (0.12, 0.035, 500.0, 0.0, None),
            };
            let (scale, gain) = match sound {
                Sound::Break(_) => (2.2, 1.0),
                Sound::Place(_) => (1.0, 0.9),
                _ => (1.0, 0.6),
            };
            noise_burst(
                s,
                len * scale,
                decay * scale,
                cutoff * detune,
                grains,
                knock.map(|f| f * detune),
            )
            .into_iter()
            .map(|x| x * gain)
            .collect()
        }
        Sound::Eat => noise_burst(s, 0.12, 0.03, 2200.0 * detune, 900.0, None),
        Sound::Burp => voice(
            s,
            0.45,
            |t| (95.0 - t * 40.0) * detune,
            |_| 500.0,
            swell(0.03, 0.15, 0.45),
        ),
        Sound::Pop => {
            let n = s.len(0.07);
            let mut phase = 0.0f32;
            (0..n)
                .map(|i| {
                    let t = i as f32 / rate;
                    phase += (700.0 + t * 9000.0) * detune / rate;
                    (phase * std::f32::consts::TAU).sin() * (1.0 - t / 0.07) * 0.6
                })
                .collect()
        }
        Sound::Hurt => voice(
            s,
            0.25,
            |t| (230.0 - t * 300.0) * detune,
            |t| 700.0 - t * 800.0,
            swell(0.01, 0.12, 0.25),
        ),
        Sound::Mob(kind) | Sound::MobHurt(kind) => {
            let hurt = matches!(sound, Sound::MobHurt(_));
            let p = if hurt { 1.35 * detune } else { detune };
            match kind {
                // A long, low moo that opens up and closes again.
                MobKind::Cow => {
                    let len = if hurt { 0.45 } else { 1.3 };
                    voice(
                        s,
                        len,
                        move |t| (112.0 - t * 14.0 + (t * 30.0).sin() * 2.0) * p,
                        move |t| 380.0 + 520.0 * (std::f32::consts::PI * t / len).sin(),
                        swell(0.15, 0.3, len),
                    )
                }
                // Two short nasal grunts.
                MobKind::Pig => {
                    let len = if hurt { 0.25 } else { 0.45 };
                    voice(
                        s,
                        len,
                        move |t| (170.0 + (t * 50.0).sin() * 15.0) * p,
                        |_| 950.0,
                        move |t| {
                            let grunt = (t * 2.0 / len).fract();
                            (grunt / 0.05).min(1.0) * (1.0 - grunt).powf(2.0)
                        },
                    )
                }
                // A bleat: a fast tremolo.
                MobKind::Sheep => {
                    let len = if hurt { 0.35 } else { 0.85 };
                    voice(
                        s,
                        len,
                        move |t| (270.0 - t * 30.0) * p,
                        |_| 750.0,
                        move |t| {
                            let tremolo = 0.55 + 0.45 * (t * std::f32::consts::TAU * 17.0).sin();
                            swell(0.05, 0.25, len)(t) * tremolo
                        },
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_are_short_and_in_range() {
        let mut all = vec![Sound::Eat, Sound::Burp, Sound::Pop, Sound::Hurt];
        for m in [
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
        ] {
            all.extend([Sound::Step(m), Sound::Break(m), Sound::Place(m)]);
        }
        for k in MobKind::ALL {
            all.extend([Sound::Mob(k), Sound::MobHurt(k)]);
        }
        for sound in all {
            let samples = synthesize(sound, 1, 48_000.0);
            assert!(
                !samples.is_empty() && samples.len() < 48_000 * 2,
                "{sound:?}"
            );
            let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(
                peak.is_finite() && peak > 0.01 && peak < 4.0,
                "{sound:?} peak {peak}"
            );
        }
    }
}

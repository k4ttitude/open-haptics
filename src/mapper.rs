//! Mapper: turns 48 kHz stereo audio into 1 kHz motor samples, one per 48-frame hop.
use crate::profile::{Band, Profile};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::f64::consts::TAU;
use std::fmt;
use std::sync::{Arc, OnceLock};

pub const RATE: usize = 48_000;
/// Audio frames per motor sample: 1 ms.
pub const HOP: usize = 48;
pub const WINDOW: usize = 512;
// shortcut: RANGE_DB is my guess, not Flydigi's level curve; replace when the hpss-mapper-v2 port lands
/// Level above a band's threshold that reaches full drive.
pub const RANGE_DB: f32 = 30.0;

/// Keeps `log10` finite for an empty band.
const POWER_FLOOR: f32 = 1e-12;
const MOTOR_RATE_HZ: f64 = 1000.0;
const CHANNELS: usize = 2;

#[derive(Debug, PartialEq)]
pub enum MapperError {
    PartialHop { len: usize },
    Fft(String),
}

impl fmt::Display for MapperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartialHop { len } => {
                write!(f, "audio block of {len} frames is not a multiple of {HOP}")
            }
            Self::Fft(message) => write!(f, "FFT failed: {message}"),
        }
    }
}

impl std::error::Error for MapperError {}

fn hann() -> &'static [f32; WINDOW] {
    static HANN: OnceLock<[f32; WINDOW]> = OnceLock::new();
    HANN.get_or_init(|| {
        let mut w = [0.0; WINDOW];
        for (n, value) in w.iter_mut().enumerate() {
            *value = (0.5 - 0.5 * (TAU * n as f64 / (WINDOW - 1) as f64).cos()) as f32;
        }
        w
    })
}

/// In-band power of a unit sine, so a full-scale sine reads 0 dB.
fn reference_power() -> f32 {
    static REF: OnceLock<f32> = OnceLock::new();
    *REF.get_or_init(|| {
        let energy: f32 = hann().iter().map(|w| w * w).sum();
        WINDOW as f32 * energy / 4.0
    })
}

/// One planned real FFT with its scratch buffers.
struct Spectrum {
    fft: Arc<dyn RealToComplex<f32>>,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
}

impl Spectrum {
    fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        Self {
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            fft,
        }
    }

    fn power(&mut self, frame: &[f32; WINDOW]) -> Result<Vec<f32>, MapperError> {
        for ((slot, sample), w) in self.input.iter_mut().zip(frame).zip(hann()) {
            *slot = sample * w;
        }
        self.fft
            .process(&mut self.input, &mut self.output)
            .map_err(|e| MapperError::Fft(e.to_string()))?;
        Ok(self.output.iter().map(Complex::norm_sqr).collect())
    }
}

/// `|X|²` per bin of the Hann-windowed frame. Plans an FFT per call, so it suits tests and
/// diagnostics; `Mapper` plans once.
pub fn power_spectrum(frame: &[f32; WINDOW]) -> Result<Vec<f32>, MapperError> {
    Spectrum::new().power(frame)
}

/// Power with `low_hz <= f < high_hz` in dB relative to a full-scale sine.
pub fn band_db(power: &[f32], low_hz: f32, high_hz: f32) -> f32 {
    let bin_hz = RATE as f32 / WINDOW as f32;
    let mut sum = 0.0;
    for (bin, p) in power.iter().enumerate() {
        let hz = bin as f32 * bin_hz;
        if hz >= low_hz && hz < high_hz {
            sum += p;
        }
    }
    10.0 * (sum / reference_power() + POWER_FLOOR).log10()
}

/// Drive for a band level: 0 below the threshold, then linear over `RANGE_DB`, times the gain.
fn target(band: &Band, level_db: f32) -> f64 {
    if level_db < band.threshold_db {
        return 0.0;
    }
    let scale = ((level_db - band.threshold_db) / RANGE_DB).min(1.0);
    f64::from(scale) * 10f64.powf(f64::from(band.gain_db) / 20.0)
}

/// Phase of hop `hop`, computed from the hop count so it never drifts.
fn carrier_phase(carrier_hz: f32, hop: u64, offset: f64) -> f64 {
    let cycles = f64::from(carrier_hz) * hop as f64 / MOTOR_RATE_HZ;
    TAU * cycles.fract() + offset
}

struct BandState {
    band: Band,
    attack: f64,
    release: f64,
    env: [f64; CHANNELS],
}

impl BandState {
    fn new(band: &Band) -> Self {
        Self {
            band: band.clone(),
            attack: (-1.0 / f64::from(band.attack_ms)).exp(),
            release: (-1.0 / f64::from(band.release_ms)).exp(),
            env: [0.0; CHANNELS],
        }
    }

    /// Moves the envelope of `channel` toward the drive for `level_db`; one hop is 1 ms.
    fn follow(&mut self, channel: usize, level_db: f32) -> f64 {
        let target = target(&self.band, level_db);
        let env = self.env[channel];
        let coeff = if target > env {
            self.attack
        } else {
            self.release
        };
        self.env[channel] = target + coeff * (env - target);
        self.env[channel]
    }
}

pub struct Mapper {
    spectrum: Spectrum,
    bands: Vec<BandState>,
    phase_offset: [f64; CHANNELS],
    history: [[f32; WINDOW]; CHANNELS],
    hop: u64,
}

impl Mapper {
    pub fn new(profile: &Profile) -> Self {
        Self {
            spectrum: Spectrum::new(),
            bands: profile.bands.iter().map(BandState::new).collect(),
            phase_offset: [0.0, f64::from(profile.phase_offset_r_deg).to_radians()],
            history: [[0.0; WINDOW]; CHANNELS],
            hop: 0,
        }
    }

    /// Maps whole hops of audio to one motor sample pair per hop.
    pub fn process(&mut self, block: &[[f32; 2]]) -> Result<Vec<[f32; 2]>, MapperError> {
        if !block.len().is_multiple_of(HOP) {
            return Err(MapperError::PartialHop { len: block.len() });
        }
        let mut out = Vec::with_capacity(block.len() / HOP);
        for frames in block.as_chunks::<HOP>().0 {
            out.push(self.hop(frames)?);
            self.hop += 1;
        }
        Ok(out)
    }

    fn hop(&mut self, frames: &[[f32; 2]]) -> Result<[f32; 2], MapperError> {
        let mut sample = [0.0; CHANNELS];
        for (channel, history) in self.history.iter_mut().enumerate() {
            history.copy_within(HOP.., 0);
            for (slot, frame) in history[WINDOW - HOP..].iter_mut().zip(frames) {
                *slot = frame[channel];
            }
            let power = self.spectrum.power(history)?;
            let mut sum = 0.0;
            for state in &mut self.bands {
                let level = band_db(&power, state.band.low_hz, state.band.high_hz);
                let env = state.follow(channel, level);
                let phase =
                    carrier_phase(state.band.carrier_hz, self.hop, self.phase_offset[channel]);
                sum += env * phase.sin();
            }
            sample[channel] = sum.clamp(-1.0, 1.0) as f32;
        }
        Ok(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{DEFAULT_PROFILE, Profile, load_profile};
    use serde_json::Value;
    use std::f64::consts::PI;

    fn balanced() -> Profile {
        load_profile(std::path::Path::new(DEFAULT_PROFILE)).unwrap()
    }

    /// Frame i, channel c is the sum over tones listing c of amp * sin(2*pi*hz*i/48000), in f64,
    /// and 0 from `off_after` seconds on. tools/dsp_reference.py builds the same input.
    fn render(spec: &Value) -> Vec<[f32; 2]> {
        let frames = (spec["seconds"].as_f64().unwrap() * RATE as f64).round() as usize;
        let off = spec["off_after"]
            .as_f64()
            .map_or(frames, |s| (s * RATE as f64).round() as usize);
        let mut audio = vec![[0.0f32; 2]; frames];
        for tone in spec["tones"].as_array().unwrap() {
            let (hz, amp) = (tone["hz"].as_f64().unwrap(), tone["amp"].as_f64().unwrap());
            for ch in tone["channels"].as_array().unwrap() {
                let ch = ch.as_u64().unwrap() as usize;
                for (i, frame) in audio.iter_mut().enumerate().take(off) {
                    let wave = amp * (2.0 * PI * hz * i as f64 / RATE as f64).sin();
                    frame[ch] = (frame[ch] as f64 + wave) as f32;
                }
            }
        }
        audio
    }

    fn tone(hz: f64, amp: f64, seconds: f64) -> Vec<[f32; 2]> {
        render(&serde_json::json!({
            "tones": [{"hz": hz, "amp": amp, "channels": [0, 1]}], "seconds": seconds
        }))
    }

    #[test]
    fn matches_numpy_reference() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mapper");
        let mut count = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let fixture: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let got = Mapper::new(&balanced())
                .process(&render(&fixture["input"]))
                .unwrap();
            let want = fixture["output"].as_array().unwrap();
            assert_eq!(got.len(), want.len(), "{path:?}");
            let mut worst = 0.0f64;
            for (g, w) in got.iter().zip(want) {
                for ch in 0..2 {
                    worst = worst.max((g[ch] as f64 - w[ch].as_f64().unwrap()).abs());
                }
            }
            println!("{path:?} max abs difference {worst:e}");
            assert!(worst < 1e-4, "{path:?} differs by {worst}");
            count += 1;
        }
        assert_eq!(count, 6);
    }

    #[test]
    fn full_scale_in_band_sine_reads_0_db() {
        let mut frame = [0.0f32; WINDOW];
        for (i, s) in frame.iter_mut().enumerate() {
            *s = (2.0 * PI * 2000.0 * i as f64 / RATE as f64).sin() as f32;
        }
        let db = band_db(&power_spectrum(&frame).unwrap(), 1000.0, 3000.0);
        assert!(db.abs() < 0.5, "{db}");
    }

    #[test]
    fn silence_is_exact_zero() {
        let out = Mapper::new(&balanced())
            .process(&vec![[0.0; 2]; 4800])
            .unwrap();
        assert!(out.iter().all(|s| *s == [0.0, 0.0]));
    }

    #[test]
    fn tone_below_threshold_is_silent() {
        let out = Mapper::new(&balanced())
            .process(&tone(2000.0, 0.01, 0.2))
            .unwrap();
        assert!(out.iter().all(|s| *s == [0.0, 0.0]));
    }

    #[test]
    fn right_is_left_inverted_for_180_degrees() {
        let out = Mapper::new(&balanced())
            .process(&tone(2000.0, 0.05, 0.3))
            .unwrap();
        assert!(out.iter().any(|s| s[0].abs() > 0.1));
        assert!(out.iter().all(|s| (s[0] + s[1]).abs() < 1e-6));
    }

    #[test]
    fn full_scale_noise_stays_bounded() {
        let mut state = 12345u32;
        let noise: Vec<[f32; 2]> = (0..48_000)
            .map(|_| {
                [(); 2].map(|()| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0
                })
            })
            .collect();
        let out = Mapper::new(&balanced()).process(&noise).unwrap();
        assert!(
            out.iter()
                .flatten()
                .all(|x| x.is_finite() && x.abs() <= 1.0)
        );
    }

    #[test]
    fn split_blocks_match_one_call() {
        let audio = tone(4500.0, 0.05, 0.1);
        let whole = Mapper::new(&balanced()).process(&audio).unwrap();
        let mut mapper = Mapper::new(&balanced());
        let mut split = Vec::new();
        for block in audio.chunks(240) {
            split.extend(mapper.process(block).unwrap());
        }
        assert_eq!(whole, split);
    }

    #[test]
    fn rejects_partial_hop() {
        let result = Mapper::new(&balanced()).process(&[[0.0; 2]; 100]);
        assert!(matches!(result, Err(MapperError::PartialHop { len: 100 })));
    }
}

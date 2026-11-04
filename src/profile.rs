//! Mapper profile: the bands that turn audio into motor drive, read from a JSON file.
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

/// Path to the bundled balanced profile; the CLI's `--profile` default.
pub const DEFAULT_PROFILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/profiles/balanced.json");

/// The motor stream is 1 kHz, so carriers stop below its 500 Hz Nyquist limit.
const MAX_CARRIER_HZ: f32 = 400.0;
/// Half of the 48 kHz audio input rate.
const MAX_BAND_HZ: f32 = 24_000.0;
const MIN_ENVELOPE_MS: f32 = 1.0;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Band {
    pub low_hz: f32,
    pub high_hz: f32,
    pub carrier_hz: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub threshold_db: f32,
    pub gain_db: f32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(default)]
    pub bands: Vec<Band>,
    #[serde(default)]
    pub phase_offset_r_deg: f32,
}

/// The file nests the profile under `mapper`; other keys are ignored.
#[derive(Deserialize)]
struct ProfileFile {
    mapper: Profile,
}

#[derive(Debug)]
pub enum ProfileError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    NoBands,
    NotFinite,
    NegativeLow,
    BandOrder,
    Carrier,
    BandLimit,
    Envelope,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse { path, source } => write!(f, "cannot parse {}: {source}", path.display()),
            Self::NoBands => f.write_str("profile has no mapper bands"),
            Self::NotFinite => f.write_str("profile values must be finite numbers"),
            Self::NegativeLow => f.write_str("lowHz must not be negative"),
            Self::BandOrder => f.write_str("mapper band highHz must exceed lowHz"),
            Self::Carrier => write!(f, "carrierHz must be between 1 and {MAX_CARRIER_HZ}"),
            Self::BandLimit => write!(f, "highHz must not exceed {MAX_BAND_HZ}"),
            Self::Envelope => write!(
                f,
                "attackMs and releaseMs must be at least {MIN_ENVELOPE_MS}"
            ),
        }
    }
}

impl std::error::Error for ProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl Band {
    fn is_finite(&self) -> bool {
        [
            self.low_hz,
            self.high_hz,
            self.carrier_hz,
            self.attack_ms,
            self.release_ms,
            self.threshold_db,
            self.gain_db,
        ]
        .iter()
        .all(|v| v.is_finite())
    }

    fn validate(&self) -> Result<(), ProfileError> {
        // JSON numbers beyond f32 range become infinite in the cast.
        if !self.is_finite() {
            return Err(ProfileError::NotFinite);
        }
        if !(0.0..).contains(&self.low_hz) {
            return Err(ProfileError::NegativeLow);
        }
        if self.high_hz <= self.low_hz {
            return Err(ProfileError::BandOrder);
        }
        if !(1.0..=MAX_CARRIER_HZ).contains(&self.carrier_hz) {
            return Err(ProfileError::Carrier);
        }
        if self.high_hz > MAX_BAND_HZ {
            return Err(ProfileError::BandLimit);
        }
        if !(MIN_ENVELOPE_MS..).contains(&self.attack_ms)
            || !(MIN_ENVELOPE_MS..).contains(&self.release_ms)
        {
            return Err(ProfileError::Envelope);
        }
        Ok(())
    }
}

impl Profile {
    fn validate(&self) -> Result<(), ProfileError> {
        if !self.phase_offset_r_deg.is_finite() {
            return Err(ProfileError::NotFinite);
        }
        if self.bands.is_empty() {
            return Err(ProfileError::NoBands);
        }
        self.bands.iter().try_for_each(Band::validate)
    }
}

pub fn load_profile(path: &Path) -> Result<Profile, ProfileError> {
    let text = std::fs::read_to_string(path).map_err(|source| ProfileError::Read {
        path: path.to_owned(),
        source,
    })?;
    let file: ProfileFile = serde_json::from_str(&text).map_err(|source| ProfileError::Parse {
        path: path.to_owned(),
        source,
    })?;
    file.mapper.validate()?;
    Ok(file.mapper)
}

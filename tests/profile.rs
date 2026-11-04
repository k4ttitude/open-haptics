use open_haptics::profile::{DEFAULT_PROFILE, Profile, ProfileError, load_profile};
use std::path::Path;

fn load_json(json: &str) -> Result<Profile, ProfileError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.json");
    std::fs::write(&path, json).unwrap();
    load_profile(&path)
}

/// One band with the given overrides applied to a valid base band.
fn one_band(low: f32, high: f32, carrier: f32, attack: f32, release: f32) -> String {
    format!(
        r#"{{"mapper":{{"bands":[{{"lowHz":{low},"highHz":{high},"carrierHz":{carrier},
        "attackMs":{attack},"releaseMs":{release},"thresholdDb":-30,"gainDb":0}}]}}}}"#
    )
}

#[test]
fn loads_balanced() {
    let profile = load_profile(Path::new(DEFAULT_PROFILE)).unwrap();
    let carriers: Vec<f32> = profile.bands.iter().map(|b| b.carrier_hz).collect();
    assert_eq!(carriers, [60.0, 90.0, 170.0]);
    let first = &profile.bands[0];
    assert_eq!(
        (
            first.low_hz,
            first.high_hz,
            first.attack_ms,
            first.release_ms
        ),
        (1000.0, 3000.0, 1.0, 20.0)
    );
    assert_eq!((first.threshold_db, first.gain_db), (-32.0, 6.0));
    assert_eq!(profile.phase_offset_r_deg, 180.0);
}

#[test]
fn phase_offset_defaults_to_zero() {
    let profile = load_json(&one_band(100.0, 200.0, 60.0, 1.0, 1.0)).unwrap();
    assert_eq!(profile.phase_offset_r_deg, 0.0);
}

#[test]
fn rejects_bad_bands() {
    let cases = [
        (
            one_band(3000.0, 1000.0, 60.0, 1.0, 1.0),
            "mapper band highHz must exceed lowHz",
        ),
        (
            one_band(100.0, 200.0, 0.0, 1.0, 1.0),
            "carrierHz must be between 1 and 400",
        ),
        (
            one_band(100.0, 200.0, 401.0, 1.0, 1.0),
            "carrierHz must be between 1 and 400",
        ),
        (
            one_band(100.0, 24001.0, 60.0, 1.0, 1.0),
            "highHz must not exceed 24000",
        ),
        (
            one_band(100.0, 200.0, 60.0, 0.0, 1.0),
            "attackMs and releaseMs must be at least 1",
        ),
        (
            one_band(100.0, 200.0, 60.0, 1.0, 0.5),
            "attackMs and releaseMs must be at least 1",
        ),
        (
            r#"{"mapper":{"bands":[]}}"#.to_string(),
            "profile has no mapper bands",
        ),
    ];
    for (json, message) in cases {
        assert_eq!(load_json(&json).unwrap_err().to_string(), message, "{json}");
    }
}

#[test]
fn errors_name_the_file() {
    let missing = load_profile(Path::new("/nonexistent/p.json"))
        .unwrap_err()
        .to_string();
    assert!(missing.contains("/nonexistent/p.json"), "{missing}");
    let garbled = load_json("not json").unwrap_err().to_string();
    assert!(garbled.contains("p.json"), "{garbled}");
}

/// A valid one-band profile with `field` set to the raw JSON `literal`.
fn with_field(field: &str, literal: &str) -> String {
    let mut values = serde_json::json!({
        "lowHz": 100, "highHz": 200, "carrierHz": 60, "attackMs": 1,
        "releaseMs": 1, "thresholdDb": -30, "gainDb": 0
    });
    let band = values.as_object_mut().unwrap();
    if field == "phaseOffsetRDeg" {
        return format!(r#"{{"mapper":{{"{field}":{literal},"bands":[{values}]}}}}"#);
    }
    band.remove(field);
    let rest = values.to_string();
    let rest = rest.trim_start_matches('{').trim_end_matches('}');
    format!(r#"{{"mapper":{{"bands":[{{"{field}":{literal},{rest}}}]}}}}"#)
}

#[test]
fn rejects_non_finite_and_negative_values() {
    let finite = "profile values must be finite numbers";
    let cases = [
        ("gainDb", "1e40", finite),
        ("thresholdDb", "-1e40", finite),
        ("lowHz", "-1e40", finite),
        ("releaseMs", "1e40", finite),
        ("phaseOffsetRDeg", "1e40", finite),
        ("lowHz", "-1", "lowHz must not be negative"),
    ];
    for (field, literal, message) in cases {
        let json = with_field(field, literal);
        assert_eq!(load_json(&json).unwrap_err().to_string(), message, "{json}");
    }
}

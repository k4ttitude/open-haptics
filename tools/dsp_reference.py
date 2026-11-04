"""Independent numpy reference for the mapper (src/mapper.rs). Writes tests/fixtures/mapper/*.json.

Run from the repo root: uv run --with numpy python tools/dsp_reference.py

Test input formula, shared with the Rust test helper: for frame i at 48 kHz, channel c,
sample = sum over tones listed for c of amp * sin(2*pi*hz*i/48000), in f64, and 0 for
i >= off_after * 48000 when "off_after" is given.
"""

import json
import math
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
RATE = 48_000
HOP = 48
WINDOW = 512
RANGE_DB = 30.0
EPS = 1e-12

CASES = {
    "tone_2k": {"tones": [{"hz": 2000, "amp": 0.05, "channels": [0, 1]}], "seconds": 0.5},
    "tone_4k5": {"tones": [{"hz": 4500, "amp": 0.05, "channels": [0, 1]}], "seconds": 0.5},
    "tone_8k": {"tones": [{"hz": 8000, "amp": 0.05, "channels": [0, 1]}], "seconds": 0.5},
    "step_on_off": {
        "tones": [{"hz": 2000, "amp": 0.05, "channels": [0, 1]}],
        "seconds": 0.5,
        "off_after": 0.2,
    },
    "two_tone_loud": {
        "tones": [
            {"hz": 2000, "amp": 0.5, "channels": [0, 1]},
            {"hz": 4500, "amp": 0.5, "channels": [0, 1]},
        ],
        "seconds": 0.5,
    },
    "left_only": {"tones": [{"hz": 2000, "amp": 0.05, "channels": [0]}], "seconds": 0.5},
}


def make_input(spec):
    frames = round(spec["seconds"] * RATE)
    i = np.arange(frames)
    audio = np.zeros((frames, 2))
    for tone in spec["tones"]:
        wave = tone["amp"] * np.sin(2 * math.pi * tone["hz"] * i / RATE)
        for c in tone["channels"]:
            audio[:, c] += wave
    if "off_after" in spec:
        audio[round(spec["off_after"] * RATE) :] = 0.0
    return audio


def run(profile, audio):
    bands = profile["bands"]
    offsets = [0.0, math.radians(profile["phaseOffsetRDeg"])]
    n = np.arange(WINDOW)
    hann = 0.5 - 0.5 * np.cos(2 * math.pi * n / (WINDOW - 1))
    ref = WINDOW * np.sum(hann**2) / 4
    freqs = np.arange(WINDOW // 2 + 1) * RATE / WINDOW
    hops = len(audio) // HOP
    env = np.zeros((len(bands), 2))
    out = np.zeros((hops, 2))
    for hop in range(hops):
        end = (hop + 1) * HOP
        for ch in range(2):
            frame = np.zeros(WINDOW)
            chunk = audio[max(0, end - WINDOW) : end, ch]
            frame[WINDOW - len(chunk) :] = chunk
            power = np.abs(np.fft.rfft(frame * hann)) ** 2
            total = 0.0
            for b, band in enumerate(bands):
                mask = (freqs >= band["lowHz"]) & (freqs < band["highHz"])
                level = 10 * math.log10(power[mask].sum() / ref + EPS)
                if level < band["thresholdDb"]:
                    target = 0.0
                else:
                    scale = min((level - band["thresholdDb"]) / RANGE_DB, 1.0)
                    target = scale * 10 ** (band["gainDb"] / 20)
                ms = band["attackMs"] if target > env[b, ch] else band["releaseMs"]
                coeff = math.exp(-1 / ms)
                env[b, ch] = target + coeff * (env[b, ch] - target)
                phase = 2 * math.pi * math.fmod(band["carrierHz"] * hop / 1000, 1.0) + offsets[ch]
                total += env[b, ch] * math.sin(phase)
            out[hop, ch] = min(max(total, -1.0), 1.0)
    return out


def main():
    profile = json.loads((ROOT / "profiles" / "balanced.json").read_text())["mapper"]
    dest = ROOT / "tests" / "fixtures" / "mapper"
    dest.mkdir(parents=True, exist_ok=True)
    for name, spec in CASES.items():
        out = run(profile, make_input(spec))
        fixture = {"profile": "balanced", "input": spec, "output": out.tolist()}
        (dest / f"{name}.json").write_text(json.dumps(fixture))
        print(name, out.shape, float(np.abs(out).max()))


if __name__ == "__main__":
    main()

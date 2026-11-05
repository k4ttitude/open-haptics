# open-haptics

open-haptics makes a Flydigi Apex 6 Pro's grips vibrate in time with whatever plays on your PipeWire output. It does on Linux what Flydigi's Windows-only xHaptics Engine does. I wrote the HID protocol from a decompile of engine 5.0.2.8 and checked it on a real pad.

> [!WARNING]
> This is experimental. Commands that write to the controller need `--experimental`. It has run on one Apex 6 Pro over the 2.4 GHz dongle. Streaming live audio (`run`) has not yet been tested on hardware.

## Requirements

- Linux with PipeWire and `pw-record`
- Rust 1.85 or newer
- A Flydigi Apex 6 Pro (USB `37d7:2502`) with linear (LRA) grips

## Build

```bash
cargo build --release
```

## Use

```bash
open-haptics info                         # read-only: device, UUID, motor map
open-haptics tone --experimental          # 2 s test buzz: 80 Hz left, 120 Hz right
open-haptics run --experimental           # system audio to grips until Ctrl+C
open-haptics run --experimental --gain 2  # stronger (0 to 4, default 1)
```

`run` uses the built-in `profiles/balanced.json`. `--profile PATH` loads another profile in the same format.

Close the Flydigi Space Station web page while you use open-haptics, because it can hold the controller over WebHID.

## How it keeps your controller safe

Before its first write, open-haptics saves the controller's motor map to `~/.local/state/open-haptics/motor-map-<uuid>.bin`. On exit it puts the original motor modes back, including after Ctrl+C, SIGTERM, a closed terminal or a write error, and then deletes the file. If the process is killed with `kill -9`, the file stays and `run` refuses to start until you run:

```bash
open-haptics restore --experimental
```

Turning the controller off also brings it back to its normal state.

## Permissions

If `info` reports `permission denied`, add a udev rule instead of using sudo:

```
# /etc/udev/rules.d/70-open-haptics.rules
KERNEL=="hidraw*", ATTRS{idVendor}=="37d7", ATTRS{idProduct}=="2502", TAG+="uaccess"
```

## Development

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

The mapper tests compare against fixtures from an independent numpy implementation. To regenerate them, run `uv run --with numpy python tools/dsp_reference.py`.

open-haptics is not affiliated with Flydigi.

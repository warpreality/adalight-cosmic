# adalight-cosmic

Ambient backlight (Adalight protocol) driven by native Wayland screen capture,
targeting **CachyOS + COSMIC** (cosmic-comp). Written in Rust.

Captures one monitor via the `ext-image-copy-capture-v1` staging protocol —
**no portal, no permission dialog** — averages the screen edges, and drives a
WS2812 (or similar) strip through an Arduino running a FastLED Adalight sketch
over USB serial.

## Why ext-image-copy-capture

COSMIC does not expose `wlr-screencopy`. It exposes the newer upstream staging
protocol `ext_image_copy_capture_manager_v1` + `ext_output_image_capture_source_manager_v1`
(verified via `wayland-info`). Any client of the compositor may capture through
it without a portal prompt, which is exactly what a background daemon wants.

## Firmware

Flash `firmware/adalight_nano/adalight_nano.ino` to an Arduino Nano
(ATmega328P). It speaks the Adalight protocol and runs an R→G→B self-test on
boot so you can verify strip/wiring/power independently of the serial link.

Settings at the top of the sketch **must** match `config.toml`:

- `NUM_LEDS` = sum of your config segments (default 77 = 20+37+20).
- `DATA_PIN` = Arduino digital pin driving the strip (default `2` = D2).
- `LED_TYPE` = strip chipset (default `WS2812B`).
- `BAUD` = must equal `[serial] baud` (default 500000).

```bash
arduino-cli lib install FastLED
arduino-cli compile --fqbn arduino:avr:nano firmware/adalight_nano
arduino-cli upload -p /dev/ttyUSB0 --fqbn arduino:avr:nano firmware/adalight_nano
```

**Power note:** address strips draw up to ~60mA/LED at full white. Do not power
more than a handful of LEDs from the Nano's USB — use a separate 5V PSU and tie
its ground to the Nano's ground.

## Build

Requires a Rust toolchain (`rustup`), plus system dev headers for libudev
(pulled by the `serialport` crate) — on CachyOS/Arch:

```bash
sudo pacman -S --needed base-devel systemd-libs
cargo build --release
```

The binary lands at `target/release/adalight-cosmic`.

## Configure

```bash
mkdir -p ~/.config/adalight-cosmic
cp config.example.toml ~/.config/adalight-cosmic/config.toml
$EDITOR ~/.config/adalight-cosmic/config.toml
```

Key fields:

- `serial.device` — your controller, `/dev/ttyUSB0`.
- `serial.baud` — **must match your Arduino sketch** (classic Adalight = 115200;
  for 60fps with many LEDs bump both sketch and this to 500000/1000000).
- `serial.color_order` — leave `RGB`; only change if colors come out swapped.
- `capture.output` — `DP-1` or `HDMI-A-1` (from `wayland-info`).
- `leds.segment` — describe the LED chain **in physical order** from the first
  LED, one segment per edge. `edge` + `count` + `direction`
  (`ltr`/`rtl` for top/bottom, `ttb`/`btt` for left/right).

You are in the `uucp` group, which owns `/dev/ttyUSB0` on Arch, so **no sudo is
needed** for serial access.

## Run

```bash
# foreground, with FPS stats
./target/release/adalight-cosmic --stats

# override the monitor without editing config
./target/release/adalight-cosmic --output HDMI-A-1
```

## Autostart (systemd --user)

```bash
install -Dm755 target/release/adalight-cosmic ~/.local/bin/adalight-cosmic
install -Dm644 systemd/adalight-cosmic.service \
  ~/.config/systemd/user/adalight-cosmic.service
systemctl --user daemon-reload
systemctl --user enable --now adalight-cosmic.service
journalctl --user -u adalight-cosmic -f
```

The unit binds to `graphical-session.target`, so it starts with your COSMIC
session (where the Wayland socket exists) and restarts on failure.

## Tuning

- **Colors too dark / muddy?** Averaging is done in linear light already; try
  raising `color.brightness` or lowering `color.gamma` slightly.
- **Flicker in dark scenes?** Raise `color.black_level` and/or `color.smoothing`.
- **Laggy / smeary motion?** Lower `color.smoothing` toward 0.
- **High CPU on 4K?** Raise `capture.subsample` (e.g. 6–8) and/or lower
  `capture.band_depth`.
- **Not hitting target FPS?** Serial baud is the usual ceiling — raise it in
  both the sketch and config.

## Architecture

```
capture (ext-image-copy → shm frame)
  └─ geometry (LED chain → edge sampling rects, normalized→pixels)
       └─ color (linear average, white balance, EMA smoothing, gamma, saturation)
            └─ adalight (framing + serial write)
```

## Layout

```
src/
  main.rs            loop, pacing, reconnect
  config.rs          TOML schema + validation
  geometry.rs        LED chain → sampling rectangles
  color.rs           linear averaging, smoothing, gamma/WB/saturation
  adalight.rs        Adalight framing + serial writer
  capture/
    mod.rs           ScreenCapturer trait + Frame
    ext_image.rs     ext-image-copy-capture backend
```

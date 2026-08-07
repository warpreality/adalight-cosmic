mod adalight;
mod capture;
mod color;
mod config;
mod geometry;

use adalight::Adalight;
use anyhow::{Context, Result};
use capture::ext_image::ExtImageCapturer;
use capture::ScreenCapturer;
use clap::Parser;
use color::ColorProcessor;
use config::Config;
use geometry::{build_zones, to_pixels, PixRect};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "adalight-cosmic", about = "Wayland ambient backlight for COSMIC")]
struct Cli {
    /// Path to config.toml
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Override capture output (wl_output name), e.g. DP-1
    #[arg(long)]
    output: Option<String>,

    /// Paint the cursor into the captured frame
    #[arg(long, default_value_t = false)]
    cursor: bool,

    /// Print measured FPS every second
    #[arg(long, default_value_t = false)]
    stats: bool,
}

fn default_config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(dir).join("adalight-cosmic/config.toml")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config/adalight-cosmic/config.toml")
    } else {
        PathBuf::from("config.toml")
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    let cli = Cli::parse();
    let cfg_path = cli.config.clone().unwrap_or_else(default_config_path);
    let mut cfg = Config::load(&cfg_path)
        .with_context(|| format!("loading config from {}", cfg_path.display()))?;
    if let Some(out) = cli.output.clone() {
        cfg.capture.output = out;
    }

    let led_count = cfg.total_leds();
    log::info!(
        "config: {} LEDs, output={}, {} fps target, serial={} @ {}",
        led_count,
        cfg.capture.output,
        cfg.capture.fps,
        cfg.serial.device,
        cfg.serial.baud
    );

    // Precompute normalized zones (chain order).
    let zones = build_zones(&cfg);

    // Open serial. Retry loop so the daemon survives an unplugged controller.
    let mut ada = open_serial_retry(&cfg, led_count);

    // Color processor keeps EMA state across frames.
    let mut proc = ColorProcessor::new(cfg.color.clone(), led_count);

    // Reusable output buffer.
    let mut rgb_out: Vec<u8> = Vec::with_capacity(led_count * 3);

    let frame_budget = Duration::from_secs_f64(1.0 / cfg.capture.fps as f64);
    let step = cfg.capture.subsample;

    // Outer loop: (re)initialize capture on failure (e.g. output vanished).
    loop {
        let mut cap = match ExtImageCapturer::new(&cfg.capture.output, cli.cursor) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("capture init failed: {e:#}; retrying in 1s");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        log::info!("capturing {}x{}", cap.width(), cap.height());

        // Pixel rects for this frame size.
        let rects: Vec<PixRect> = to_pixels(&zones, cap.width(), cap.height());

        let mut frames = 0u32;
        let mut last_report = Instant::now();

        // Inner loop: steady-state capture.
        loop {
            let t0 = Instant::now();

            let cap_result = cap.with_next_frame(&mut |frame| {
                proc.process(
                    frame.data,
                    frame.stride,
                    frame.format,
                    &rects,
                    step,
                    &mut rgb_out,
                );
            });

            if let Err(e) = cap_result {
                log::warn!("capture error: {e:#}; reinitializing");
                break;
            }

            // Push to LEDs; on serial failure, reopen.
            if let Err(e) = ada.write_frame(&rgb_out) {
                log::warn!("serial write failed: {e:#}; reopening port");
                ada = open_serial_retry(&cfg, led_count);
                continue;
            }

            frames += 1;
            if cli.stats && last_report.elapsed() >= Duration::from_secs(1) {
                log::info!("{} fps", frames);
                frames = 0;
                last_report = Instant::now();
            }

            // Frame pacing.
            let elapsed = t0.elapsed();
            if elapsed < frame_budget {
                std::thread::sleep(frame_budget - elapsed);
            }
        }
    }
}

fn open_serial_retry(cfg: &Config, led_count: usize) -> Adalight {
    loop {
        match Adalight::open(&cfg.serial, led_count) {
            Ok(a) => {
                log::info!("serial open: {}", cfg.serial.device);
                return a;
            }
            Err(e) => {
                log::warn!("serial open failed: {e:#}; retrying in 1s");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

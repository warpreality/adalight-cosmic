use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub serial: SerialCfg,
    pub capture: CaptureCfg,
    pub color: ColorCfg,
    pub leds: LedsCfg,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SerialCfg {
    pub device: String,
    pub baud: u32,
    #[serde(default = "default_color_order")]
    pub color_order: ColorOrder,
}

fn default_color_order() -> ColorOrder {
    ColorOrder::Rgb
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum ColorOrder {
    Rgb,
    Rbg,
    Grb,
    Gbr,
    Brg,
    Bgr,
}

impl ColorOrder {
    /// Reorder an (r,g,b) triple into the wire order this chain expects.
    pub fn apply(self, r: u8, g: u8, b: u8) -> [u8; 3] {
        match self {
            ColorOrder::Rgb => [r, g, b],
            ColorOrder::Rbg => [r, b, g],
            ColorOrder::Grb => [g, r, b],
            ColorOrder::Gbr => [g, b, r],
            ColorOrder::Brg => [b, r, g],
            ColorOrder::Bgr => [b, g, r],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaptureCfg {
    pub output: String,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_band")]
    pub band_depth: f32,
    #[serde(default = "default_subsample")]
    pub subsample: u32,
}

fn default_fps() -> u32 {
    60
}
fn default_band() -> f32 {
    0.08
}
fn default_subsample() -> u32 {
    4
}

#[derive(Debug, Clone, Deserialize)]
pub struct ColorCfg {
    #[serde(default = "default_gamma")]
    pub gamma: f32,
    #[serde(default = "one")]
    pub brightness: f32,
    #[serde(default = "one")]
    pub saturation: f32,
    #[serde(default)]
    pub smoothing: f32,
    #[serde(default)]
    pub black_level: u8,
    #[serde(default = "default_wb")]
    pub white_balance: [f32; 3],
}

fn default_gamma() -> f32 {
    2.2
}
fn one() -> f32 {
    1.0
}
fn default_wb() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

#[derive(Debug, Clone, Deserialize)]
pub struct LedsCfg {
    pub segment: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Segment {
    pub edge: Edge,
    pub count: u32,
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Ltr,
    Rtl,
    Ttb,
    Btt,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).context("parsing config TOML")?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn total_leds(&self) -> usize {
        self.leds.segment.iter().map(|s| s.count as usize).sum()
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(self.total_leds() > 0, "no LEDs configured");
        anyhow::ensure!(
            (0.0..0.5).contains(&self.capture.band_depth),
            "capture.band_depth must be in [0.0, 0.5)"
        );
        anyhow::ensure!(
            self.capture.subsample >= 1,
            "capture.subsample must be >= 1"
        );
        anyhow::ensure!(self.capture.fps >= 1, "capture.fps must be >= 1");
        for (edge, dir) in self.leds.segment.iter().map(|s| (s.edge, s.direction)) {
            let ok = match edge {
                Edge::Top | Edge::Bottom => {
                    matches!(dir, Direction::Ltr | Direction::Rtl)
                }
                Edge::Left | Edge::Right => {
                    matches!(dir, Direction::Ttb | Direction::Btt)
                }
            };
            anyhow::ensure!(
                ok,
                "horizontal edges need ltr/rtl, vertical edges need ttb/btt"
            );
        }
        Ok(())
    }
}

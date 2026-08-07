use crate::config::ColorCfg;
use crate::geometry::PixRect;

/// Pixel byte layout of the captured buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Bytes in memory: B, G, R, X  (little-endian XRGB8888 / ARGB8888).
    Bgrx,
    /// Bytes in memory: R, G, B, X  (little-endian XBGR8888 / ABGR8888).
    Rgbx,
}

/// sRGB (0..255) -> linear (0..1). Precomputed LUT for speed.
struct SrgbLut([f32; 256]);

impl SrgbLut {
    fn new() -> Self {
        let mut lut = [0.0f32; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
        }
        SrgbLut(lut)
    }
    #[inline]
    fn to_linear(&self, v: u8) -> f32 {
        self.0[v as usize]
    }
}

pub struct ColorProcessor {
    cfg: ColorCfg,
    lut: SrgbLut,
    // Per-LED smoothed state in linear space.
    ema: Vec<[f32; 3]>,
    initialized: bool,
}

impl ColorProcessor {
    pub fn new(cfg: ColorCfg, led_count: usize) -> Self {
        ColorProcessor {
            cfg,
            lut: SrgbLut::new(),
            ema: vec![[0.0; 3]; led_count],
            initialized: false,
        }
    }

    /// Average one zone in linear space. Returns linear (r,g,b) in 0..1.
    fn average_zone(
        &self,
        buf: &[u8],
        stride: usize,
        fmt: PixelFormat,
        rect: &PixRect,
        step: u32,
    ) -> [f32; 3] {
        let (ro, go, bo) = match fmt {
            PixelFormat::Bgrx => (2usize, 1usize, 0usize),
            PixelFormat::Rgbx => (0usize, 1usize, 2usize),
        };
        let mut acc = [0.0f64; 3];
        let mut n = 0u64;
        let step = step.max(1);
        let mut y = rect.y0;
        while y < rect.y1 {
            let row = y as usize * stride;
            let mut x = rect.x0;
            while x < rect.x1 {
                let p = row + x as usize * 4;
                if p + 3 < buf.len() {
                    acc[0] += self.lut.to_linear(buf[p + ro]) as f64;
                    acc[1] += self.lut.to_linear(buf[p + go]) as f64;
                    acc[2] += self.lut.to_linear(buf[p + bo]) as f64;
                    n += 1;
                }
                x += step;
            }
            y += step;
        }
        if n == 0 {
            return [0.0, 0.0, 0.0];
        }
        [
            (acc[0] / n as f64) as f32,
            (acc[1] / n as f64) as f32,
            (acc[2] / n as f64) as f32,
        ]
    }

    /// Process a full frame into a flat RGB byte vector (3 bytes per LED),
    /// in chain order. `out` is reused across calls.
    pub fn process(
        &mut self,
        buf: &[u8],
        stride: usize,
        fmt: PixelFormat,
        rects: &[PixRect],
        step: u32,
        out: &mut Vec<u8>,
    ) {
        out.clear();
        let s = self.cfg.smoothing.clamp(0.0, 0.999);
        let inv_gamma = 1.0 / self.cfg.gamma.max(0.01);
        let wb = self.cfg.white_balance;

        for (i, rect) in rects.iter().enumerate() {
            let mut lin = self.average_zone(buf, stride, fmt, rect, step);

            // White balance + brightness in linear space.
            lin[0] = (lin[0] * wb[0] * self.cfg.brightness).clamp(0.0, 1.0);
            lin[1] = (lin[1] * wb[1] * self.cfg.brightness).clamp(0.0, 1.0);
            lin[2] = (lin[2] * wb[2] * self.cfg.brightness).clamp(0.0, 1.0);

            // Temporal EMA smoothing (linear).
            let prev = &mut self.ema[i];
            if self.initialized {
                for c in 0..3 {
                    prev[c] = prev[c] * s + lin[c] * (1.0 - s);
                }
            } else {
                *prev = lin;
            }
            let sm = *prev;

            // Encode to gamma-corrected 8-bit.
            let mut rgb = [
                (sm[0].powf(inv_gamma) * 255.0).round(),
                (sm[1].powf(inv_gamma) * 255.0).round(),
                (sm[2].powf(inv_gamma) * 255.0).round(),
            ];

            // Saturation in encoded space (simple luma-based).
            let sat = self.cfg.saturation;
            if (sat - 1.0).abs() > f32::EPSILON {
                let luma = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
                for c in rgb.iter_mut() {
                    *c = (luma + (*c - luma) * sat).clamp(0.0, 255.0);
                }
            }

            // Black-level cutoff to suppress dark noise flicker.
            let bl = self.cfg.black_level as f32;
            let (mut r, mut g, mut b) = (rgb[0], rgb[1], rgb[2]);
            if r < bl && g < bl && b < bl {
                r = 0.0;
                g = 0.0;
                b = 0.0;
            }
            out.push(r as u8);
            out.push(g as u8);
            out.push(b as u8);
        }
        self.initialized = true;
    }
}

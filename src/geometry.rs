use crate::config::{Config, Direction, Edge, Segment};

/// A sampling rectangle in normalized [0,1] screen coordinates.
/// (x0,y0) top-left, (x1,y1) bottom-right, with x1>x0, y1>y0.
#[derive(Debug, Clone, Copy)]
pub struct NormRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

/// A sampling rectangle in pixel coordinates for a given frame size.
#[derive(Debug, Clone, Copy)]
pub struct PixRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

/// Build the per-LED normalized sampling rectangles, in physical chain order.
pub fn build_zones(cfg: &Config) -> Vec<NormRect> {
    let band = cfg.capture.band_depth;
    let mut zones = Vec::with_capacity(cfg.total_leds());
    for seg in &cfg.leds.segment {
        zones.extend(segment_zones(seg, band));
    }
    zones
}

fn segment_zones(seg: &Segment, band: f32) -> Vec<NormRect> {
    let n = seg.count.max(1);
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        // Fractional slot along the edge (0..1) for LED i in ascending
        // geometric order (left->right or top->bottom).
        let a = i as f32 / n as f32;
        let b = (i + 1) as f32 / n as f32;
        let rect = match seg.edge {
            Edge::Top => NormRect {
                x0: a,
                y0: 0.0,
                x1: b,
                y1: band,
            },
            Edge::Bottom => NormRect {
                x0: a,
                y0: 1.0 - band,
                x1: b,
                y1: 1.0,
            },
            Edge::Left => NormRect {
                x0: 0.0,
                y0: a,
                x1: band,
                y1: b,
            },
            Edge::Right => NormRect {
                x0: 1.0 - band,
                y0: a,
                x1: 1.0,
                y1: b,
            },
        };
        out.push(rect);
    }

    // Reorder into the physical chain direction.
    let reversed = matches!(seg.direction, Direction::Rtl | Direction::Btt);
    if reversed {
        out.reverse();
    }
    out
}

/// Convert normalized zones to pixel rects for a concrete frame size.
pub fn to_pixels(zones: &[NormRect], width: u32, height: u32) -> Vec<PixRect> {
    zones
        .iter()
        .map(|z| {
            let x0 = (z.x0 * width as f32).floor() as u32;
            let y0 = (z.y0 * height as f32).floor() as u32;
            let x1 = ((z.x1 * width as f32).ceil() as u32).min(width).max(x0 + 1);
            let y1 = ((z.y1 * height as f32).ceil() as u32)
                .min(height)
                .max(y0 + 1);
            PixRect { x0, y0, x1, y1 }
        })
        .collect()
}

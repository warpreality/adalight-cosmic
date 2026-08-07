use crate::config::{ColorOrder, SerialCfg};
use anyhow::{Context, Result};
use serialport::SerialPort;
use std::io::Write;
use std::time::Duration;

/// Adalight serial writer.
///
/// Frame format:
///   'A','d','a', countHi, countLo, checksum, [R,G,B]*N
///   countHi  = (N-1) >> 8
///   countLo  = (N-1) & 0xFF
///   checksum = countHi ^ countLo ^ 0x55
pub struct Adalight {
    port: Box<dyn SerialPort>,
    color_order: ColorOrder,
    header: [u8; 6],
    frame: Vec<u8>,
    led_count: usize,
}

impl Adalight {
    pub fn open(cfg: &SerialCfg, led_count: usize) -> Result<Self> {
        let port = serialport::new(&cfg.device, cfg.baud)
            .timeout(Duration::from_millis(100))
            .open()
            .with_context(|| format!("opening serial port {}", cfg.device))?;

        let n = led_count as u32;
        let count = n.saturating_sub(1);
        let hi = ((count >> 8) & 0xFF) as u8;
        let lo = (count & 0xFF) as u8;
        let checksum = hi ^ lo ^ 0x55;
        let header = [b'A', b'd', b'a', hi, lo, checksum];

        let mut frame = Vec::with_capacity(6 + led_count * 3);
        frame.extend_from_slice(&header);
        frame.resize(6 + led_count * 3, 0);

        Ok(Adalight {
            port,
            color_order: cfg.color_order,
            header,
            frame,
            led_count,
        })
    }

    /// `rgb` is a flat buffer of length led_count*3 in R,G,B order (chain order).
    pub fn write_frame(&mut self, rgb: &[u8]) -> Result<()> {
        anyhow::ensure!(
            rgb.len() == self.led_count * 3,
            "expected {} bytes, got {}",
            self.led_count * 3,
            rgb.len()
        );

        self.frame[..6].copy_from_slice(&self.header);
        for i in 0..self.led_count {
            let src = &rgb[i * 3..i * 3 + 3];
            let ordered = self.color_order.apply(src[0], src[1], src[2]);
            let dst = 6 + i * 3;
            self.frame[dst..dst + 3].copy_from_slice(&ordered);
        }

        self.port
            .write_all(&self.frame)
            .context("writing adalight frame")?;
        Ok(())
    }
}

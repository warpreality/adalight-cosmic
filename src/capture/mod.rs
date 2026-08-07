use crate::color::PixelFormat;
use anyhow::Result;

pub mod ext_image;

/// A captured frame: borrowed pixel data plus geometry.
pub struct Frame<'a> {
    pub data: &'a [u8],
    #[allow(dead_code)]
    pub width: u32,
    #[allow(dead_code)]
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,
}

/// Backend-agnostic screen capturer.
pub trait ScreenCapturer {
    /// Block until the next frame is captured, then hand it to `f`.
    /// The borrow ends when `f` returns, so the backend can reuse its buffer.
    fn with_next_frame(&mut self, f: &mut dyn FnMut(Frame<'_>)) -> Result<()>;

    fn width(&self) -> u32;
    fn height(&self) -> u32;
}

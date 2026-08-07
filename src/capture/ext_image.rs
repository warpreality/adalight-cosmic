//! Screen capture via the `ext-image-copy-capture-v1` +
//! `ext-image-capture-source-v1` staging protocols (COSMIC / wlroots-ext).
//!
//! Flow:
//!   1. bind wl_output (match by name), wl_shm,
//!      ext_output_image_capture_source_manager_v1,
//!      ext_image_copy_capture_manager_v1
//!   2. create_source(output) -> capture source
//!   3. create_session(source, opts) -> session
//!   4. session emits buffer_size / shm_format / done -> allocate shm buffer
//!   5. per frame: create_frame(); attach_buffer(); capture(); wait ready/failed
//!
//! NOTE: this touches staging Wayland protocols. If it fails to build against
//! your exact wayland-protocols version, the interface paths under
//! `wayland_protocols::ext::*` are the thing to check first.

use crate::color::PixelFormat;
use crate::capture::{Frame, ScreenCapturer};
use anyhow::{anyhow, bail, Context, Result};
use memmap2::MmapMut;
use std::os::fd::AsFd;

use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_output::WlOutput,
    wl_registry::WlRegistry,
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

#[derive(Default)]
struct OutputInfo {
    output: Option<WlOutput>,
    name: Option<String>,
}

/// Globals discovered during registry enumeration.
#[derive(Default)]
struct Globals {
    shm: Option<WlShm>,
    source_mgr: Option<ExtOutputImageCaptureSourceManagerV1>,
    capture_mgr: Option<ExtImageCopyCaptureManagerV1>,
}

/// Frame lifecycle result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameState {
    Pending,
    Ready,
    Failed,
}

struct State {
    globals: Globals,
    // All wl_outputs seen, keyed by registry name, with their advertised names.
    outputs: Vec<OutputInfo>,

    // Session negotiation results.
    buf_width: u32,
    buf_height: u32,
    shm_format: Option<wl_shm::Format>,
    session_done: bool,
    session_stopped: bool,

    frame_state: FrameState,
}

impl State {
    fn new(want_output: String) -> Self {
        let _ = want_output;
        State {
            globals: Globals::default(),
            outputs: Vec::new(),
            buf_width: 0,
            buf_height: 0,
            shm_format: None,
            session_done: false,
            session_stopped: false,
            frame_state: FrameState::Pending,
        }
    }
}

pub struct ExtImageCapturer {
    conn: Connection,
    event_queue: wayland_client::EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,

    _shm: WlShm,
    _pool: WlShmPool,
    buffer: WlBuffer,
    mmap: MmapMut,

    session: ExtImageCopyCaptureSessionV1,

    width: u32,
    height: u32,
    stride: usize,
    format: PixelFormat,
}

impl ExtImageCapturer {
    pub fn new(output_name: &str, paint_cursors: bool) -> Result<Self> {
        let conn = Connection::connect_to_env().context("connecting to Wayland")?;
        let mut event_queue = conn.new_event_queue();
        let qh = event_queue.handle();
        let display = conn.display();
        let _registry = display.get_registry(&qh, ());

        let mut state = State::new(output_name.to_string());

        // Roundtrip #1: enumerate globals + outputs.
        event_queue.roundtrip(&mut state)?;
        // Roundtrip #2: let wl_output names arrive.
        event_queue.roundtrip(&mut state)?;

        let shm = state
            .globals
            .shm
            .clone()
            .ok_or_else(|| anyhow!("compositor has no wl_shm"))?;
        let source_mgr = state.globals.source_mgr.clone().ok_or_else(|| {
            anyhow!("no ext_output_image_capture_source_manager_v1")
        })?;
        let capture_mgr = state.globals.capture_mgr.clone().ok_or_else(|| {
            anyhow!("no ext_image_copy_capture_manager_v1")
        })?;

        // Find the requested output.
        let output = state
            .outputs
            .iter()
            .find(|o| o.name.as_deref() == Some(output_name))
            .and_then(|o| o.output.clone())
            .ok_or_else(|| {
                anyhow!(
                    "output '{}' not found; available: {}",
                    output_name,
                    state
                        .outputs
                        .iter()
                        .filter_map(|o| o.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;

        // Create capture source from the output.
        let source: ExtImageCaptureSourceV1 =
            source_mgr.create_source(&output, &qh, ());

        // Create the capture session.
        let opts = if paint_cursors {
            ext_image_copy_capture_manager_v1::Options::PaintCursors
        } else {
            ext_image_copy_capture_manager_v1::Options::empty()
        };
        let session = capture_mgr.create_session(&source, opts, &qh, ());

        // Roundtrip until the session reports its constraints (done).
        state.session_done = false;
        state.buf_width = 0;
        state.buf_height = 0;
        state.shm_format = None;
        while !state.session_done {
            event_queue.blocking_dispatch(&mut state)?;
            if state.session_stopped {
                bail!("capture session stopped during negotiation");
            }
        }

        let width = state.buf_width;
        let height = state.buf_height;
        anyhow::ensure!(width > 0 && height > 0, "session gave zero buffer size");
        let wl_format = state
            .shm_format
            .ok_or_else(|| anyhow!("session advertised no shm format"))?;
        let format = map_format(wl_format)
            .ok_or_else(|| anyhow!("unsupported shm format: {:?}", wl_format))?;

        // 4 bytes per pixel for all supported formats.
        let stride = width as usize * 4;
        let size = stride * height as usize;

        // Allocate a shared-memory buffer.
        let mem = memfd_mmap(size).context("allocating shm buffer")?;
        let pool = shm.create_pool(mem.fd.as_fd(), size as i32, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_format,
            &qh,
            (),
        );

        Ok(ExtImageCapturer {
            conn,
            event_queue,
            qh,
            state,
            _shm: shm,
            _pool: pool,
            buffer,
            mmap: mem.mmap,
            session,
            width,
            height,
            stride,
            format,
        })
    }
}

impl ScreenCapturer for ExtImageCapturer {
    fn with_next_frame(&mut self, f: &mut dyn FnMut(Frame<'_>)) -> Result<()> {
        // Create a one-shot frame, attach the buffer, request capture.
        self.state.frame_state = FrameState::Pending;
        let frame = self.session.create_frame(&self.qh, ());
        frame.attach_buffer(&self.buffer);
        frame.damage_buffer(0, 0, self.width as i32, self.height as i32);
        frame.capture();
        self.conn.flush().ok();

        while self.state.frame_state == FrameState::Pending {
            self.event_queue.blocking_dispatch(&mut self.state)?;
            if self.state.session_stopped {
                frame.destroy();
                bail!("capture session stopped");
            }
        }

        let result = self.state.frame_state;
        frame.destroy();

        match result {
            FrameState::Ready => {
                let data: &[u8] = &self.mmap[..self.stride * self.height as usize];
                f(Frame {
                    data,
                    width: self.width,
                    height: self.height,
                    stride: self.stride,
                    format: self.format,
                });
                Ok(())
            }
            FrameState::Failed => Err(anyhow!("frame capture failed")),
            FrameState::Pending => unreachable!(),
        }
    }

    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
}

fn map_format(f: wl_shm::Format) -> Option<PixelFormat> {
    match f {
        // Little-endian XRGB/ARGB8888 => bytes B,G,R,X in memory.
        wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888 => Some(PixelFormat::Bgrx),
        // Little-endian XBGR/ABGR8888 => bytes R,G,B,X in memory.
        wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => Some(PixelFormat::Rgbx),
        _ => None,
    }
}

// --- shared memory helper -------------------------------------------------

struct ShmMem {
    fd: std::fs::File,
    mmap: MmapMut,
}

fn memfd_mmap(size: usize) -> Result<ShmMem> {
    use rustix::fs::{memfd_create, MemfdFlags};
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::fd::IntoRawFd;

    let ofd = memfd_create("adalight-cosmic", MemfdFlags::CLOEXEC)
        .context("memfd_create")?;
    // SAFETY: ofd is a freshly created, owned fd.
    let file: File = unsafe { File::from_raw_fd(ofd.into_raw_fd()) };
    file.set_len(size as u64).context("set_len on shm")?;
    // SAFETY: file is a valid, sized fd backing the mapping.
    let mmap = unsafe { MmapMut::map_mut(&file).context("mmap shm")? };
    Ok(ShmMem { fd: file, mmap })
}

// --- Dispatch impls -------------------------------------------------------

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_registry::Event;
        if let Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_shm" => {
                    state.globals.shm =
                        Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()));
                }
                "wl_output" => {
                    let out = registry.bind::<WlOutput, _, _>(
                        name,
                        version.min(4),
                        qh,
                        (),
                    );
                    state.outputs.push(OutputInfo {
                        output: Some(out),
                        name: None,
                    });
                }
                "ext_output_image_capture_source_manager_v1" => {
                    state.globals.source_mgr = Some(
                        registry
                            .bind::<ExtOutputImageCaptureSourceManagerV1, _, _>(
                                name,
                                version.min(1),
                                qh,
                                (),
                            ),
                    );
                }
                "ext_image_copy_capture_manager_v1" => {
                    state.globals.capture_mgr = Some(
                        registry.bind::<ExtImageCopyCaptureManagerV1, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        ),
                    );
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: <WlOutput as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_output::Event;
        if let Event::Name { name } = event {
            if let Some(info) = state
                .outputs
                .iter_mut()
                .find(|o| o.output.as_ref() == Some(output))
            {
                info.name = Some(name);
            }
        }
    }
}

impl Dispatch<WlShm, ()> for State {
    fn event(_: &mut Self, _: &WlShm, _: <WlShm as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlShmPool, ()> for State {
    fn event(_: &mut Self, _: &WlShmPool, _: <WlShmPool as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(_: &mut Self, _: &WlBuffer, _: <WlBuffer as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ExtOutputImageCaptureSourceManagerV1, _: <ExtOutputImageCaptureSourceManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtImageCaptureSourceV1, ()> for State {
    fn event(_: &mut Self, _: &ExtImageCaptureSourceV1, _: <ExtImageCaptureSourceV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ExtImageCopyCaptureManagerV1, _: <ExtImageCopyCaptureManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: <ExtImageCopyCaptureSessionV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_session_v1::Event;
        match event {
            Event::BufferSize { width, height } => {
                state.buf_width = width;
                state.buf_height = height;
            }
            Event::ShmFormat { format } => {
                if let wayland_client::WEnum::Value(f) = format {
                    // Prefer XRGB/XBGR if offered; keep first supported otherwise.
                    if state.shm_format.is_none() || map_format(f).is_some() {
                        if map_format(f).is_some() {
                            state.shm_format = Some(f);
                        }
                    }
                }
            }
            Event::Done => {
                state.session_done = true;
            }
            Event::Stopped => {
                state.session_stopped = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: <ExtImageCopyCaptureFrameV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use ext_image_copy_capture_frame_v1::Event;
        match event {
            Event::Ready => state.frame_state = FrameState::Ready,
            Event::Failed { .. } => state.frame_state = FrameState::Failed,
            _ => {}
        }
    }
}

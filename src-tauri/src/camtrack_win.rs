//! The cam track on Windows. Two threads: the worker (`clippiboy-cam`) does
//! only what keeps time — tick the grid, scale, submit, push — and the helper
//! (`clippiboy-cam-ctl`) does everything that blocks: opening and closing the
//! camera (a C920 takes two seconds to stop), asking who holds it, starting
//! and stopping the encoder. An old encoder is dropped there, outside the ring
//! lock its own drain takes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{select, Receiver, Sender};
use windows::Win32::Foundation::{RECT, TRUE};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC};
use windows::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;

use super::{CamPlan, CamShared, PlanSender};
use crate::audio::capture::now_100ns;
use crate::camera::grid::{Grid, Out};
use crate::camera::{self, colour_of, cover_crop, pick_device, Backoff, CameraDevice, ClockKind, Error, Event, Frame, OpenMode, Source};
use crate::gpu::GpuDevice;
use crate::mft::{EncoderSettings, RateMode, VideoEncoder};
use crate::model::{CamState, CamStatus, EncoderId};

/// While open and silent this long, the camera counts as taken.
const SILENT: Duration = Duration::from_secs(5);
/// A privacy block is looked at again this often: the user fixes it in the
/// Windows settings and expects the cam back.
const BLOCKED_RETRY: Duration = Duration::from_secs(30);

enum Ask {
    Open { id: Option<String>, name: Option<String>, track: (u32, u32), generation: u64, events: Sender<(u64, Event)> },
    Close(Box<dyn Source>),
    Holder(CameraDevice),
    StartEncoder { session: u64, gpu: Arc<GpuDevice>, settings: EncoderSettings },
    StopEncoder(VideoEncoder),
}

enum Answer {
    Opened { generation: u64, result: Result<(Box<dyn Source>, CameraDevice), (Error, Option<CameraDevice>)> },
    Holder(Option<String>),
    Encoder { session: u64, result: Result<VideoEncoder, String> },
}

/// Both threads join the multithreaded apartment and bring Media Foundation up.
fn join_mta() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let _ = crate::mft::startup();
}

pub(super) fn spawn(shared: Arc<CamShared>, plans: Receiver<CamPlan>, sender: PlanSender) {
    let (ask, asks) = crossbeam_channel::unbounded::<Ask>();
    let (answer, answers) = crossbeam_channel::unbounded::<Answer>();
    let for_helper = shared.clone();
    let helper = std::thread::Builder::new()
        .name("clippiboy-cam-ctl".into())
        .spawn(move || helper(for_helper, asks, answer));
    if let Err(err) = helper {
        log::error!("cam helper thread: {err}");
        *sender.lock() = None;
        return;
    }
    let worker = std::thread::Builder::new().name("clippiboy-cam".into()).spawn(move || {
        join_mta();
        Worker::new(shared, plans, sender, ask, answers).run();
    });
    if let Err(err) = worker {
        log::error!("cam worker thread: {err}");
    }
}

fn helper(shared: Arc<CamShared>, asks: Receiver<Ask>, answer: Sender<Answer>) {
    join_mta();
    // Ends when the worker has gone and every job it left is done.
    while let Ok(ask) = asks.recv() {
        match ask {
            Ask::Open { id, name, track, generation, events } => {
                let list = camera::list();
                let result = match pick_device(&list, id.as_deref(), name.as_deref()).cloned() {
                    None => Err((Error::NoCamera, None)),
                    Some(device) => {
                        let sink = Box::new(move |event| {
                            let _ = events.send((generation, event));
                        });
                        let mode = OpenMode::Controlling { width: track.0, height: track.1 };
                        match camera::open(&device, mode, sink) {
                            Ok(source) => Ok((source, device)),
                            Err(err) => Err((err, Some(device))),
                        }
                    }
                };
                let _ = answer.send(Answer::Opened { generation, result });
            }
            Ask::Close(source) => source.close(),
            Ask::Holder(device) => {
                let _ = answer.send(Answer::Holder(camera::windows::holder(&device)));
            }
            Ask::StartEncoder { session, gpu, settings } => {
                let packets = shared.clone();
                let headers = shared.clone();
                let result = VideoEncoder::start(
                    gpu,
                    settings,
                    move |packet| {
                        packets.ring.lock().push(session, packet);
                    },
                    move |header| headers.ring.lock().set_header(session, header),
                );
                let _ = answer.send(Answer::Encoder { session, result });
            }
            Ask::StopEncoder(encoder) => encoder.stop(),
        }
    }
}

struct Worker {
    shared: Arc<CamShared>,
    plans: Receiver<CamPlan>,
    sender: PlanSender,
    plan: Option<CamPlan>,
    ask: Sender<Ask>,
    answers: Receiver<Answer>,
    events_tx: Sender<(u64, Event)>,
    events: Receiver<(u64, Event)>,
    arrivals: Receiver<()>,
    _watch: Option<camera::windows::ArrivalWatch>,
    generation: u64,
    source: Option<Box<dyn Source>>,
    device: Option<CameraDevice>,
    opening: bool,
    opened_at: Option<Instant>,
    retry_at: Option<Instant>,
    backoff: Backoff,
    state: CamState,
    track: (u32, u32),
    gpu: Option<Arc<GpuDevice>>,
    scaler: Option<Scaler>,
    encoder: Option<(u64, VideoEncoder)>,
    encoder_starting: Option<u64>,
    chosen: Option<EncoderId>,
    grid: Grid<Arc<Frame>>,
    stop_at: Option<Instant>,
    stopped: bool,
    last_frame_at: Option<Instant>,
    clock: Option<ClockKind>,
    last_slot: Option<usize>,
    qpc_last: i64,
    unbiased_last: i64,
    frames_in: u32,
    frames_out: u32,
    second: Instant,
    fps_in: f32,
    fps_out: f32,
}

fn unbiased_100ns() -> i64 {
    let mut t = 0u64;
    unsafe {
        let _ = QueryUnbiasedInterruptTime(&mut t);
    }
    t as i64
}

impl Worker {
    fn new(
        shared: Arc<CamShared>,
        plans: Receiver<CamPlan>,
        sender: PlanSender,
        ask: Sender<Ask>,
        answers: Receiver<Answer>,
    ) -> Self {
        let (events_tx, events) = crossbeam_channel::unbounded();
        let (arrived, arrivals) = crossbeam_channel::unbounded();
        let watch = camera::windows::watch_arrivals(move || {
            let _ = arrived.send(());
        });
        Self {
            shared,
            plans,
            sender,
            plan: None,
            ask,
            answers,
            events_tx,
            events,
            arrivals,
            _watch: watch,
            generation: 0,
            source: None,
            device: None,
            opening: false,
            opened_at: None,
            retry_at: None,
            backoff: Backoff::default(),
            state: CamState::Starting,
            track: (1280, 720),
            gpu: None,
            scaler: None,
            encoder: None,
            encoder_starting: None,
            chosen: None,
            grid: Grid::new(30),
            stop_at: None,
            stopped: true,
            last_frame_at: None,
            clock: None,
            last_slot: None,
            qpc_last: now_100ns(),
            unbiased_last: unbiased_100ns(),
            frames_in: 0,
            frames_out: 0,
            second: Instant::now(),
            fps_in: 0.0,
            fps_out: 0.0,
        }
    }

    /// How much longer QPC ran than unbiased time since the last call: what
    /// the PC spent asleep.
    fn suspended_since(&mut self) -> i64 {
        let (qpc, unbiased) = (now_100ns(), unbiased_100ns());
        let slept = (qpc - self.qpc_last) - (unbiased - self.unbiased_last);
        self.qpc_last = qpc;
        self.unbiased_last = unbiased;
        slept
    }

    fn run(mut self) {
        loop {
            let mut outs = Vec::new();
            let (now, slept) = (now_100ns(), self.suspended_since());
            self.grid.tick(now, slept, &mut outs);
            self.handle(outs);
            self.manage();
            if self.stopped && !self.opening && self.encoder_starting.is_none() && self.try_exit() {
                return;
            }
            self.publish_status();
            select! {
                recv(self.events) -> msg => if let Ok((generation, event)) = msg { self.on_event(generation, event) },
                recv(self.plans) -> msg => if let Ok(plan) = msg { self.on_plan(plan) },
                recv(self.answers) -> msg => if let Ok(answer) = msg { self.on_answer(answer) },
                recv(self.arrivals) -> _ => {
                    if self.source.is_none() {
                        self.retry_at = Some(Instant::now());
                    }
                },
                default(Duration::from_millis(10)) => {},
            }
        }
    }

    /// Ends the worker if no plan is waiting — under the lock `apply` sends
    /// under, so a plan sent meanwhile starts a new worker instead of being lost.
    fn try_exit(&mut self) -> bool {
        let mut sender = self.sender.lock();
        if !self.plans.is_empty() {
            return false;
        }
        *sender = None;
        true
    }

    fn on_plan(&mut self, plan: CamPlan) {
        if plan.run {
            self.stop_at = None;
            if self.stopped {
                self.stopped = false;
                self.state = CamState::Starting;
                self.track = plan.resolution.size();
                *self.shared.track.lock() = self.track;
                self.backoff.reset();
                self.retry_at = None;
                let bytes = crate::config::effective_cam_memory_bytes(
                    plan.ring_seconds,
                    crate::config::cam_bitrate_kbps(plan.resolution),
                );
                self.shared.ring.lock().ring.set_capacity(plan.ring_seconds, bytes);
            } else {
                // Another camera chosen while one runs: let go of it.
                let other = self.device.as_ref().is_some_and(|d| {
                    plan.device_id.as_deref().is_some_and(|id| !id.eq_ignore_ascii_case(&d.id))
                });
                if other {
                    self.close_camera();
                    self.retry_at = None;
                }
                let bytes = crate::config::effective_cam_memory_bytes(
                    plan.ring_seconds,
                    crate::config::cam_bitrate_kbps(self.plan.as_ref().map_or(plan.resolution, |p| p.resolution)),
                );
                self.shared.ring.lock().ring.set_capacity(plan.ring_seconds, bytes);
            }
        } else if !self.stopped {
            match Instant::now().checked_add(plan.stop_after) {
                Some(at) => self.stop_at = Some(self.stop_at.map_or(at, |t| t.min(at))),
                // A repeat of the last plan keeps a stop on its way; with none
                // scheduled, the restart grace applies.
                None if self.stop_at.is_none() => self.stop_at = Some(Instant::now() + Duration::from_secs(3)),
                None => {}
            }
        }
        self.plan = Some(plan);
    }

    fn manage(&mut self) {
        let now = Instant::now();
        if self.stop_at.is_some_and(|t| now >= t) {
            self.stop_at = None;
            self.stop_all();
            return;
        }
        if self.stopped {
            return;
        }
        if self.source.is_none() && !self.opening && self.retry_at.map_or(true, |t| now >= t) {
            self.open_camera();
        }
        // Open, but no picture for a while: another app has the camera.
        if let (Some(_), Some(device)) = (&self.source, &self.device) {
            let since = self.last_frame_at.or(self.opened_at);
            if since.is_some_and(|t| now.duration_since(t) > SILENT) && !matches!(self.state, CamState::Taken { .. }) {
                self.state = CamState::Taken { by: None };
                let _ = self.ask.send(Ask::Holder(device.clone()));
            }
        }
    }

    fn open_camera(&mut self) {
        let Some(plan) = &self.plan else { return };
        self.generation += 1;
        self.opening = true;
        let _ = self.ask.send(Ask::Open {
            id: plan.device_id.clone(),
            name: plan.device_name.clone(),
            track: self.track,
            generation: self.generation,
            events: self.events_tx.clone(),
        });
    }

    fn close_camera(&mut self) {
        if let Some(source) = self.source.take() {
            let _ = self.ask.send(Ask::Close(source));
        }
        self.generation += 1;
        self.device = None;
        self.scaler = None;
        self.last_frame_at = None;
        *self.shared.latest.lock() = None;
        self.shared.preview.clear();
    }

    /// Camera first (the LED goes off), then the encoder, then the ring.
    fn stop_all(&mut self) {
        self.close_camera();
        if let Some(writer) = self.shared.vcam.lock().as_ref() {
            writer.set_state(vcam_protocol::State::Idle, vcam_protocol::IdleReason::CamOff, now_100ns());
        }
        if let Some((_, encoder)) = self.encoder.take() {
            let _ = self.ask.send(Ask::StopEncoder(encoder));
        }
        self.shared.ring.lock().reset(0);
        self.grid = Grid::new(30);
        self.gpu = None;
        self.stopped = true;
        *self.shared.status.lock() = None;
        log::info!("cam stopped");
    }

    fn fail_later(&mut self, error: &Error) {
        let wait = match error {
            Error::Denied => BLOCKED_RETRY,
            _ => self.backoff.next(),
        };
        self.retry_at = Some(Instant::now() + wait);
        self.state = match error {
            Error::Denied => CamState::Blocked,
            Error::Busy { by } => CamState::Taken { by: by.clone() },
            Error::Gone | Error::NoCamera => CamState::NoCamera,
            other => CamState::Failed { message: other.to_string() },
        };
    }

    fn on_answer(&mut self, answer: Answer) {
        match answer {
            Answer::Opened { generation, result } => {
                if generation != self.generation {
                    if let Ok((source, _)) = result {
                        let _ = self.ask.send(Ask::Close(source));
                    }
                    return;
                }
                self.opening = false;
                match result {
                    Ok((source, device)) if !self.stopped => {
                        log::info!("cam: {} open", device.name);
                        self.source = Some(source);
                        self.device = Some(device);
                        self.opened_at = Some(Instant::now());
                        self.last_frame_at = None;
                        self.state = CamState::Starting;
                    }
                    Ok((source, _)) => {
                        let _ = self.ask.send(Ask::Close(source));
                    }
                    Err((error, device)) => {
                        log::info!("cam: could not open the camera: {error}");
                        if let (Error::Busy { .. }, Some(device)) = (&error, device) {
                            let _ = self.ask.send(Ask::Holder(device));
                        }
                        self.fail_later(&error);
                    }
                }
            }
            Answer::Holder(by) => {
                if let CamState::Taken { by: held } = &mut self.state {
                    *held = by;
                }
            }
            Answer::Encoder { session, result } => {
                self.encoder_starting = None;
                match result {
                    Ok(encoder) => {
                        let current = self.shared.ring.lock().session;
                        if session != current || self.stopped {
                            let _ = self.ask.send(Ask::StopEncoder(encoder));
                            return;
                        }
                        self.shared.ring.lock().set_header(session, encoder.sequence_header.clone());
                        self.chosen = Some(encoder.chosen);
                        self.encoder = Some((session, encoder));
                    }
                    Err(err) => {
                        log::warn!("cam encoder: {err}");
                        self.shared.errors.report("cam", format!("The cam's encoder could not start: {err}"));
                        self.state = CamState::Failed { message: err };
                    }
                }
            }
        }
    }

    fn on_event(&mut self, generation: u64, event: Event) {
        if generation != self.generation || self.stopped {
            return;
        }
        match event {
            Event::Frame(frame) => {
                self.last_frame_at = Some(Instant::now());
                if !matches!(self.state, CamState::Live | CamState::Failed { .. }) {
                    self.state = CamState::Live;
                    self.backoff.reset();
                }
                self.clock = Some(frame.probe.clock);
                self.frames_in += 1;
                let frame = Arc::new(frame);
                *self.shared.latest.lock() = Some(frame.clone());
                // ClippiBoy Camera gets the picture as it came, while an app watches.
                if self.shared.vcam_watching.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Some(writer) = self.shared.vcam.lock().as_ref() {
                        writer.publish(&frame.nv12, frame.probe.width, frame.probe.height, frame.probe.qpc_100ns, now_100ns());
                    }
                }
                let size = (frame.probe.width, frame.probe.height);
                self.shared.preview.offer(&frame, cover_crop(size, self.track), Instant::now());
                let mut outs = Vec::new();
                let (now, slept) = (now_100ns(), self.suspended_since());
                self.grid.tick(now, slept, &mut outs);
                self.grid.push(frame.probe.qpc_100ns, frame, &mut outs);
                self.handle(outs);
            }
            Event::FormatChanged { width, height } => {
                log::info!("cam: the camera now runs {width}x{height}");
                // The encoder keeps its size; only the scaler's input side changes.
                self.scaler = None;
            }
            Event::Lost(error) => {
                log::info!("cam: lost the camera: {error}");
                let device = self.device.clone();
                self.close_camera();
                if let (Error::Busy { .. }, Some(device)) = (&error, device) {
                    let _ = self.ask.send(Ask::Holder(device));
                }
                self.fail_later(&error);
            }
        }
    }

    fn ensure_gpu(&mut self) -> Option<Arc<GpuDevice>> {
        if self.gpu.is_none() {
            let wanted = self.plan.as_ref().map(|p| p.encoder);
            match GpuDevice::new(wanted) {
                Ok(gpu) => self.gpu = Some(Arc::new(gpu)),
                Err(err) => {
                    log::warn!("cam graphics device: {err}");
                    self.state = CamState::Failed { message: err };
                }
            }
        }
        self.gpu.clone()
    }

    fn start_encoder(&mut self, session: u64) {
        let Some(gpu) = self.ensure_gpu() else { return };
        let Some(plan) = &self.plan else { return };
        let settings = EncoderSettings {
            width: self.track.0,
            height: self.track.1,
            fps: 30,
            bitrate_kbps: crate::config::cam_bitrate_kbps(plan.resolution),
            quality: 70,
            keyframe_seconds: 1,
            requested: plan.encoder,
            label: "cam",
            queue_depth: crate::mft::QUEUE_DEPTH,
            bframes: Some(0),
            rate: RateMode::PeakVbr,
            // Measured on a C920 at 720p30: 4.1 % of the video-encode engine
            // with the game's settings, 2.9 % this way.
            quality_vs_speed: Some(0),
            low_latency: Some(true),
        };
        self.encoder_starting = Some(session);
        let _ = self.ask.send(Ask::StartEncoder { session, gpu, settings });
    }

    fn handle(&mut self, outs: Vec<Out<Arc<Frame>>>) {
        for out in outs {
            match out {
                Out::Started { origin_100ns } => {
                    let session = {
                        let mut ring = self.shared.ring.lock();
                        ring.reset(origin_100ns);
                        ring.session
                    };
                    if let Some((_, encoder)) = self.encoder.take() {
                        let _ = self.ask.send(Ask::StopEncoder(encoder));
                    }
                    self.last_slot = None;
                    self.start_encoder(session);
                }
                Out::Reset { .. } => {
                    log::info!("cam: the session ended (a sleep or a jump in the camera's clock)");
                    if let Some((_, encoder)) = self.encoder.take() {
                        let _ = self.ask.send(Ask::StopEncoder(encoder));
                    }
                }
                Out::Frame { pts_100ns, frame, duplicate } => self.submit_frame(pts_100ns, &frame, duplicate),
                Out::Filler { pts_100ns } => self.submit_filler(pts_100ns),
                Out::GapOpened { from_100ns } => self.shared.ring.lock().open_gap(from_100ns),
                Out::GapClosed { to_100ns, .. } => self.shared.ring.lock().close_gap(to_100ns),
            }
        }
    }

    /// The encoder of the current session, if it is ready. Frames before it is
    /// are dropped; its first packet then starts after 0, which the cut allows for.
    fn ready_encoder(&self) -> bool {
        let session = self.shared.ring.lock().session;
        self.encoder.as_ref().is_some_and(|(s, _)| *s == session)
    }

    fn ensure_scaler(&mut self, size: (u32, u32)) -> bool {
        if self.scaler.as_ref().is_some_and(|s| s.src == size) {
            return true;
        }
        let Some(gpu) = self.ensure_gpu() else { return false };
        match Scaler::new(&gpu, size, self.track) {
            Ok(scaler) => {
                self.scaler = Some(scaler);
                self.last_slot = None;
                true
            }
            Err(err) => {
                log::warn!("cam scaler: {err}");
                false
            }
        }
    }

    fn submit_frame(&mut self, pts_100ns: i64, frame: &Frame, duplicate: bool) {
        if !self.ready_encoder() || !self.ensure_scaler((frame.probe.width, frame.probe.height)) {
            return;
        }
        let Some(scaler) = self.scaler.as_mut() else { return };
        let slot = match self.last_slot.filter(|_| duplicate) {
            Some(slot) => slot,
            None => match scaler.scale(frame) {
                Ok(slot) => slot,
                Err(err) => {
                    log::debug!("cam scale: {err}");
                    return;
                }
            },
        };
        self.last_slot = Some(slot);
        let texture = scaler.slots[slot].0.clone();
        if let Some((_, encoder)) = &self.encoder {
            if encoder.submit(&texture, pts_100ns) {
                self.frames_out += 1;
            }
        }
    }

    fn submit_filler(&mut self, pts_100ns: i64) {
        if !self.ready_encoder() {
            return;
        }
        if self.scaler.is_none() && !self.ensure_scaler(self.track) {
            return;
        }
        let (Some(scaler), Some((_, encoder))) = (&self.scaler, &self.encoder) else { return };
        if encoder.submit(&scaler.black, pts_100ns) {
            self.frames_out += 1;
        }
    }

    fn publish_status(&mut self) {
        let elapsed = self.second.elapsed();
        if elapsed < Duration::from_secs(1) || self.stopped {
            return;
        }
        let seconds = elapsed.as_secs_f32();
        self.fps_in = self.frames_in as f32 / seconds;
        self.fps_out = self.frames_out as f32 / seconds;
        self.frames_in = 0;
        self.frames_out = 0;
        self.second = Instant::now();
        let stats = self.grid.stats();
        let (ring_seconds, ring_bytes, gaps) = {
            let ring = self.shared.ring.lock();
            (ring.ring.buffered_seconds(), ring.ring.bytes(), ring.gaps.len() as u32)
        };
        *self.shared.status.lock() = Some(CamStatus {
            state: self.state.clone(),
            camera: self.device.as_ref().map(|d| d.name.clone()),
            fps_in: self.fps_in,
            fps_out: self.fps_out,
            duplicates: stats.duplicates,
            fillers: stats.fillers,
            gaps,
            clock: self.clock,
            encoder: self.chosen,
            ring_seconds,
            ring_bytes,
        });
    }
}

/// The camera's NV12 frame onto the cam track: uploaded through a staging
/// texture, cover-cropped and scaled by the video processor into a pool of
/// NV12 textures at the track's size, BT.601 in, BT.709 out.
struct Scaler {
    context: ID3D11DeviceContext,
    video_context: ID3D11VideoContext,
    processor: ID3D11VideoProcessor,
    staging: ID3D11Texture2D,
    input: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    slots: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    black: ID3D11Texture2D,
    src: (u32, u32),
}

fn nv12_desc(width: u32, height: u32, usage: D3D11_USAGE, bind: u32, cpu: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    }
}

fn texture(device: &ID3D11Device, desc: &D3D11_TEXTURE2D_DESC, data: Option<&[u8]>) -> Result<ID3D11Texture2D, String> {
    let init = data.map(|bytes| D3D11_SUBRESOURCE_DATA {
        pSysMem: bytes.as_ptr() as *const _,
        SysMemPitch: desc.Width,
        SysMemSlicePitch: 0,
    });
    let mut out = None;
    unsafe { device.CreateTexture2D(desc, init.as_ref().map(|i| i as *const _), Some(&mut out)) }
        .map_err(|err| format!("NV12 texture: {err}"))?;
    out.ok_or_else(|| "NV12 texture: none was created".into())
}

impl Scaler {
    fn new(gpu: &GpuDevice, src: (u32, u32), dst: (u32, u32)) -> Result<Self, String> {
        use windows::core::Interface;
        let device = gpu.device.clone();
        let video_device: ID3D11VideoDevice = device.cast().map_err(|err| format!("ID3D11VideoDevice: {err}"))?;
        let video_context: ID3D11VideoContext =
            gpu.context.cast().map_err(|err| format!("ID3D11VideoContext: {err}"))?;
        let rate = DXGI_RATIONAL { Numerator: 30, Denominator: 1 };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: src.0,
            InputHeight: src.1,
            OutputFrameRate: rate,
            OutputWidth: dst.0,
            OutputHeight: dst.1,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&desc) }
            .map_err(|err| format!("video processor enumerator: {err}"))?;
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|err| format!("video processor: {err}"))?;
        let crop = cover_crop(src, dst);
        let (bt709, full) = colour_of(None, None);
        let range = |full: bool| if full { crate::convert::RANGE_FULL } else { crate::convert::RANGE_STUDIO };
        unsafe {
            video_context.VideoProcessorSetStreamColorSpace(&processor, 0, &crate::convert::color_space(bt709, range(full)));
            video_context.VideoProcessorSetOutputColorSpace(
                &processor,
                &crate::convert::color_space(true, crate::convert::RANGE_STUDIO),
            );
            // No edge enhancement or noise reduction the driver picks for itself.
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            let source = RECT {
                left: crop.x as i32,
                top: crop.y as i32,
                right: (crop.x + crop.w) as i32,
                bottom: (crop.y + crop.h) as i32,
            };
            let whole = RECT { left: 0, top: 0, right: dst.0 as i32, bottom: dst.1 as i32 };
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, TRUE, Some(&source));
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, TRUE, Some(&whole));
            video_context.VideoProcessorSetOutputTargetRect(&processor, TRUE, Some(&whole));
        }

        let staging = texture(
            &device,
            &nv12_desc(src.0, src.1, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_WRITE.0 as u32),
            None,
        )?;
        let input = texture(&device, &nv12_desc(src.0, src.1, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0), None)?;
        let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } },
        };
        let mut input_view = None;
        unsafe { video_device.CreateVideoProcessorInputView(&input, &enumerator, &input_desc, Some(&mut input_view)) }
            .map_err(|err| format!("video processor input view: {err}"))?;
        let input_view = input_view.ok_or_else(|| "video processor input view: none".to_string())?;

        let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
        };
        let mut slots = Vec::new();
        for _ in 0..crate::mft::QUEUE_DEPTH + crate::mft::ENCODER_HOLD_MAX + 2 {
            let slot = texture(&device, &nv12_desc(dst.0, dst.1, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0), None)?;
            let mut view = None;
            unsafe { video_device.CreateVideoProcessorOutputView(&slot, &enumerator, &output_desc, Some(&mut view)) }
                .map_err(|err| format!("video processor output view: {err}"))?;
            slots.push((slot, view.ok_or_else(|| "video processor output view: none".to_string())?));
        }
        let (w, h) = (dst.0 as usize, dst.1 as usize);
        let mut black = vec![16u8; w * h];
        black.extend(std::iter::repeat(128u8).take(w * h / 2));
        let black = texture(&device, &nv12_desc(dst.0, dst.1, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0), Some(&black))?;
        Ok(Self {
            context: gpu.context.clone(),
            video_context,
            processor,
            staging,
            input,
            input_view,
            slots,
            next: 0,
            black,
            src,
        })
    }

    /// The next pool slot with `frame` in it.
    fn scale(&mut self, frame: &Frame) -> Result<usize, String> {
        let (w, h) = (self.src.0 as usize, self.src.1 as usize);
        if frame.nv12.len() < w * h * 3 / 2 {
            return Err("frame smaller than its size".into());
        }
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&self.staging, 0, D3D11_MAP_WRITE, 0, Some(&mut mapped))
                .map_err(|err| format!("map: {err}"))?;
            let pitch = mapped.RowPitch as usize;
            let dst = mapped.pData as *mut u8;
            for row in 0..h {
                std::ptr::copy_nonoverlapping(frame.nv12.as_ptr().add(row * w), dst.add(row * pitch), w);
            }
            let uv = dst.add(pitch * h);
            for row in 0..h / 2 {
                std::ptr::copy_nonoverlapping(frame.nv12.as_ptr().add(w * h + row * w), uv.add(row * pitch), w);
            }
            self.context.Unmap(&self.staging, 0);
            self.context.CopyResource(&self.input, &self.staging);
        }
        let slot = self.next;
        self.next = (self.next + 1) % self.slots.len();
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: TRUE,
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(self.input_view.clone())),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };
        let streams = [stream];
        let result = unsafe { self.video_context.VideoProcessorBlt(&self.processor, &self.slots[slot].1, 0, &streams) };
        // Hand the view's reference back, as `convert.rs` does.
        let [stream] = streams;
        drop(std::mem::ManuallyDrop::into_inner(stream.pInputSurface));
        result.map_err(|err| format!("VideoProcessorBlt: {err}"))?;
        Ok(slot)
    }
}

//! The playback engine.
//!
//! Video frames are decoded by an `ffmpeg` child process that writes raw RGB
//! frames into a pipe, and the audio track is decoded by a second `ffmpeg`
//! process into a ring buffer that is drained by a [`rodio`] source.
//!
//! Using the command line tools instead of linking against FFmpeg keeps the
//! application free of build time system dependencies: everything that is
//! needed is the very same `ffmpeg` binary the user already uses for cutting.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use parking_lot::{Condvar, Mutex};

use crate::ffmpeg_tools::Tools;
use crate::probe::MediaInfo;

/// Frames are decoded at most this wide, which keeps the memory footprint of
/// the frame queue small while still looking good in the preview.
const MAX_PREVIEW_WIDTH: u32 = 1280;
/// Number of decoded frames that are kept in memory.
const FRAME_QUEUE_CAPACITY: usize = 24;
/// Sample rate of the preview audio stream.
const AUDIO_SAMPLE_RATE: u32 = 48_000;
/// Number of channels of the preview audio stream.
const AUDIO_CHANNELS: u16 = 2;
/// Amount of audio that is buffered ahead of the sound card, in seconds.
const AUDIO_BUFFER_SECONDS: usize = 3;
/// Playback speeds offered by the user interface.
pub const SPEEDS: [f64; 4] = [0.5, 1.0, 1.5, 2.0];

/// A single decoded video frame.
struct Frame {
    /// Presentation timestamp in seconds.
    pts: f64,
    /// Raw RGB24 pixels.
    data: Arc<Vec<u8>>,
}

/// Commands accepted by the video decoding thread.
enum VideoCmd {
    /// Restart decoding at the given position.
    Seek(f64),
    /// Stop decoding and terminate the child process.
    Stop,
}

/// Commands accepted by the audio decoding thread.
enum AudioCmd {
    /// Restart decoding at the given position.
    Seek(f64),
    /// Stop decoding and terminate the child process.
    Stop,
}

/// State that both the control thread and the reader thread of a decoder need.
struct DecoderShared {
    child: Mutex<Option<Child>>,
    request: Mutex<Option<(u64, f64)>>,
    generation: AtomicU64,
}

impl DecoderShared {
    fn new() -> Self {
        Self {
            child: Mutex::new(None),
            request: Mutex::new(None),
            generation: AtomicU64::new(0),
        }
    }

    /// Ask the reader thread to restart at `time` and interrupt the current
    /// child process so that a blocked read returns immediately.
    fn request_seek(&self, time: f64) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.request.lock() = Some((generation, time));
        if let Some(child) = self.child.lock().as_mut() {
            let _ = child.kill();
        }
    }

    /// Terminate the running child process, if any.
    fn terminate(&self) {
        if let Some(child) = self.child.lock().as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Take the pending restart request, if it is newer than `generation`.
    fn take_request(&self, generation: u64) -> Option<(u64, f64)> {
        let request = self.request.lock();
        match *request {
            Some((newer, time)) if newer > generation => Some((newer, time)),
            _ => None,
        }
    }
}

/// Result of trying to take a sample out of the audio ring buffer.
enum Pop {
    /// A decoded sample.
    Sample(f32),
    /// Nothing was available at the moment.
    Empty,
    /// The decoder is finished and the buffer will never fill again.
    Closed,
}

/// A lock protected ring buffer of decoded audio samples.
struct AudioRing {
    samples: Mutex<VecDeque<f32>>,
    signal: Condvar,
    closed: AtomicBool,
    capacity: usize,
}

impl AudioRing {
    fn new(capacity: usize) -> Self {
        Self {
            samples: Mutex::new(VecDeque::with_capacity(capacity)),
            signal: Condvar::new(),
            closed: AtomicBool::new(false),
            capacity,
        }
    }

    /// Append decoded samples, blocking while the buffer is full.
    fn push(&self, values: &[f32]) {
        let mut guard = self.samples.lock();
        while guard.len() + values.len() > self.capacity && !self.closed.load(Ordering::Relaxed) {
            self.signal.wait(&mut guard);
        }
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        guard.extend(values.iter().copied());
        self.signal.notify_all();
    }

    /// Take one sample if one is available.
    fn pop(&self) -> Pop {
        let mut guard = self.samples.lock();
        if let Some(value) = guard.pop_front() {
            return Pop::Sample(value);
        }
        if self.closed.load(Ordering::Relaxed) {
            return Pop::Closed;
        }
        // Wait a short while for the decoder to catch up before reporting an
        // underrun; the sound card callback must not block for long.
        self.signal.wait_for(&mut guard, Duration::from_millis(8));
        if let Some(value) = guard.pop_front() {
            Pop::Sample(value)
        } else if self.closed.load(Ordering::Relaxed) {
            Pop::Closed
        } else {
            Pop::Empty
        }
    }

    /// Drop everything that is buffered.
    fn clear(&self) {
        self.samples.lock().clear();
        self.signal.notify_all();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.signal.notify_all();
    }
}

/// A [`rodio::Source`] that pulls decoded samples out of an [`AudioRing`].
struct PcmSource {
    ring: Arc<AudioRing>,
    channels: u16,
    sample_rate: u32,
}

impl Iterator for PcmSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match self.ring.pop() {
            Pop::Sample(value) => Some(value),
            // An underrun is silent audio, not the end of the stream.
            Pop::Empty => Some(0.0),
            Pop::Closed => None,
        }
    }
}

impl rodio::Source for PcmSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// The sound card and the mixer handle that is needed to create sinks.
struct AudioOutput {
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
}

/// Everything that is needed to play the audio of a media file.
struct AudioSession {
    ring: Arc<AudioRing>,
    sink: rodio::Sink,
    shared: Arc<DecoderShared>,
    commands: Sender<AudioCmd>,
}

/// Mutable playback state.
struct PlaybackState {
    /// Media position that the clock was anchored at.
    anchor_time: f64,
    /// Wall clock moment the anchor was taken at.
    anchor_instant: Instant,
    /// Whether playback is running.
    playing: bool,
    /// Current playback speed.
    speed: f64,
    /// Current volume between 0 and 1.
    volume: f32,
}

/// A loaded media file with its decoder threads.
struct Session {
    info: MediaInfo,
    frames: Arc<Mutex<VecDeque<Arc<Frame>>>>,
    video_commands: Sender<VideoCmd>,
    video_shared: Arc<DecoderShared>,
    audio: Option<AudioSession>,
    state: Arc<Mutex<PlaybackState>>,
}

/// Compute the current media position from the playback state.
///
/// The wall clock is the master clock: the video decoder follows it and the
/// audio is played by the sound card at the same rate.
fn position_of(state: &PlaybackState, duration: f64) -> f64 {
    if state.playing {
        let elapsed = state.anchor_instant.elapsed().as_secs_f64() * state.speed;
        (state.anchor_time + elapsed).clamp(0.0, duration.max(0.0))
    } else {
        state.anchor_time
    }
}

/// The player shown in the preview area.
pub struct Player {
    ctx: egui::Context,
    tools: Option<Tools>,
    session: Option<Session>,
    texture: egui::TextureHandle,
    texture_size: [usize; 2],
    uploaded_pts: Option<f64>,
    audio_output: Option<AudioOutput>,
}

impl Player {
    /// Create a player that paints into `ctx`.
    pub fn new(ctx: egui::Context) -> Self {
        let texture = ctx.load_texture(
            "vidcut-preview",
            egui::ColorImage::from_rgb([1, 1], &[0, 0, 0]),
            egui::TextureOptions::LINEAR,
        );
        Self {
            ctx,
            tools: None,
            session: None,
            texture,
            texture_size: [1, 1],
            uploaded_pts: None,
            audio_output: None,
        }
    }

    /// Provide the FFmpeg tools that the player uses for decoding.
    pub fn set_tools(&mut self, tools: Tools) {
        self.tools = Some(tools);
    }

    /// The FFmpeg tools, if they have been found.
    pub fn tools(&self) -> Option<&Tools> {
        self.tools.as_ref()
    }

    /// `true` when a media file is loaded.
    pub fn is_loaded(&self) -> bool {
        self.session.is_some()
    }

    /// Duration of the loaded file in seconds.
    pub fn duration(&self) -> f64 {
        self.session
            .as_ref()
            .map(|session| session.info.duration)
            .unwrap_or(0.0)
    }

    /// Frames per second of the loaded file.
    pub fn fps(&self) -> f64 {
        self.session
            .as_ref()
            .map(|session| session.info.fps())
            .unwrap_or(25.0)
    }

    /// `true` while the file is playing.
    pub fn is_playing(&self) -> bool {
        self.session
            .as_ref()
            .map(|session| session.state.lock().playing)
            .unwrap_or(false)
    }

    /// Current playback speed.
    pub fn speed(&self) -> f64 {
        self.session
            .as_ref()
            .map(|session| session.state.lock().speed)
            .unwrap_or(1.0)
    }

    /// Current volume between 0 and 1.
    pub fn volume(&self) -> f32 {
        self.session
            .as_ref()
            .map(|session| session.state.lock().volume)
            .unwrap_or(1.0)
    }

    /// The current media position in seconds.
    pub fn position(&self) -> f64 {
        let Some(session) = self.session.as_ref() else {
            return 0.0;
        };
        let state = session.state.lock();
        position_of(&state, session.info.duration)
    }

    /// Load a media file and start decoding it.
    pub fn open(&mut self, path: &Path, info: MediaInfo) -> Result<()> {
        self.close();

        let tools = self
            .tools
            .clone()
            .ok_or_else(|| anyhow!("the ffmpeg tools are not available"))?;

        let size = preview_size(info.video.as_ref().map(|video| (video.width, video.height)));
        let fps = if info.fps() > 0.0 { info.fps() } else { 25.0 };

        let frames: Arc<Mutex<VecDeque<Arc<Frame>>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(FRAME_QUEUE_CAPACITY)));
        let (video_commands, video_receiver) = std::sync::mpsc::channel();
        let video_shared = Arc::new(DecoderShared::new());
        let duration = info.duration;
        let state = Arc::new(Mutex::new(PlaybackState {
            anchor_time: 0.0,
            anchor_instant: Instant::now(),
            playing: false,
            speed: 1.0,
            volume: 1.0,
        }));

        spawn_video_decoder(
            tools.ffmpeg.clone(),
            path.to_path_buf(),
            size,
            fps,
            0.0,
            frames.clone(),
            video_receiver,
            video_shared.clone(),
            state.clone(),
            duration,
        );

        let audio = if info.has_audio() {
            self.start_audio(path)
        } else {
            None
        };

        self.session = Some(Session {
            info,
            frames,
            video_commands,
            video_shared,
            audio,
            state,
        });
        Ok(())
    }

    /// Unload the current file and stop all decoder threads.
    pub fn close(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.video_commands.send(VideoCmd::Stop);
            session.video_shared.terminate();
            if let Some(audio) = session.audio {
                let _ = audio.commands.send(AudioCmd::Stop);
                audio.shared.terminate();
                audio.ring.close();
                audio.sink.stop();
            }
        }
        self.uploaded_pts = None;
    }

    /// Start or pause playback.
    pub fn toggle_play(&mut self) {
        self.set_playing(!self.is_playing());
    }

    /// Start or pause playback.
    pub fn set_playing(&mut self, playing: bool) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        {
            let mut state = session.state.lock();
            if state.playing == playing {
                return;
            }
            state.anchor_time = {
                let elapsed = if state.playing {
                    state.anchor_instant.elapsed().as_secs_f64() * state.speed
                } else {
                    0.0
                };
                (state.anchor_time + elapsed).clamp(0.0, session.info.duration.max(0.0))
            };
            state.anchor_instant = Instant::now();
            state.playing = playing;
        }
        if let Some(audio) = session.audio.as_ref() {
            if playing {
                audio.sink.play();
            } else {
                audio.sink.pause();
            }
        }
        if playing {
            self.ctx.request_repaint();
        }
    }

    /// Jump to `time` in seconds.
    pub fn seek(&mut self, time: f64) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let target = time.clamp(0.0, session.info.duration.max(0.0));
        {
            let mut state = session.state.lock();
            state.anchor_time = target;
            state.anchor_instant = Instant::now();
        }
        let _ = session.video_commands.send(VideoCmd::Seek(target));
        session.video_shared.request_seek(target);
        if let Some(audio) = session.audio.as_ref() {
            audio.ring.clear();
            let _ = audio.commands.send(AudioCmd::Seek(target));
            audio.shared.request_seek(target);
        }
        self.uploaded_pts = None;
        self.ctx.request_repaint();
    }

    /// Move by `delta` seconds relative to the current position.
    pub fn nudge(&mut self, delta: f64) {
        let position = self.position();
        self.seek(position + delta);
    }

    /// Step a whole number of frames forwards or backwards.
    pub fn step_frame(&mut self, delta: i64) {
        let fps = self.fps();
        if fps <= 0.0 {
            return;
        }
        let was_playing = self.is_playing();
        if was_playing {
            self.set_playing(false);
        }
        let frame = ((self.position() * fps).round() as i64 + delta).max(0);
        self.seek(frame as f64 / fps);
        if was_playing {
            self.set_playing(true);
        }
    }

    /// Change the playback speed.
    pub fn set_speed(&mut self, speed: f64) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        {
            let mut state = session.state.lock();
            let elapsed = if state.playing {
                state.anchor_instant.elapsed().as_secs_f64() * state.speed
            } else {
                0.0
            };
            state.anchor_time += elapsed;
            state.anchor_instant = Instant::now();
            state.speed = speed;
        }
        if let Some(audio) = session.audio.as_ref() {
            audio.sink.set_speed(speed as f32);
        }
    }

    /// Change the output volume.
    pub fn set_volume(&mut self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        if let Some(session) = self.session.as_ref() {
            session.state.lock().volume = volume;
            if let Some(audio) = session.audio.as_ref() {
                audio.sink.set_volume(volume);
            }
        }
    }

    /// The texture that holds the currently displayed frame.
    pub fn texture(&self) -> &egui::TextureHandle {
        &self.texture
    }

    /// Size of the preview texture in pixels.
    pub fn texture_size(&self) -> [usize; 2] {
        self.texture_size
    }

    /// Pull the newest decoded frame into the preview texture.
    ///
    /// Returns `true` when the texture changed.
    pub fn refresh(&mut self) -> bool {
        let Some(session) = self.session.as_ref() else {
            return false;
        };
        let position = self.position();
        let frame = pick_frame(&session.frames, position, session.info.fps());
        let Some(frame) = frame else {
            return false;
        };
        if self.uploaded_pts == Some(frame.pts) {
            return false;
        }
        let width = session
            .info
            .video
            .as_ref()
            .map(|video| preview_size(Some((video.width, video.height))).0 as usize)
            .unwrap_or(1);
        let height = session
            .info
            .video
            .as_ref()
            .map(|video| preview_size(Some((video.width, video.height))).1 as usize)
            .unwrap_or(1);
        let image = egui::ColorImage::from_rgb([width, height], frame.data.as_slice());
        self.texture.set(image, egui::TextureOptions::LINEAR);
        self.texture_size = [width, height];
        self.uploaded_pts = Some(frame.pts);
        true
    }

    /// Ask the UI to repaint while the file is playing.
    pub fn tick(&self) {
        if self.is_playing() {
            self.ctx.request_repaint();
        }
    }

    fn start_audio(&mut self, path: &Path) -> Option<AudioSession> {
        if self.audio_output.is_none() {
            self.audio_output = match rodio::OutputStream::try_default() {
                Ok((stream, handle)) => Some(AudioOutput {
                    _stream: stream,
                    handle,
                }),
                Err(error) => {
                    log::warn!("could not open an audio output device: {error}");
                    None
                }
            };
        }
        let output = self.audio_output.as_ref()?;
        let sink = match rodio::Sink::try_new(&output.handle) {
            Ok(sink) => sink,
            Err(error) => {
                log::warn!("could not create the audio sink: {error}");
                return None;
            }
        };
        let ring = Arc::new(AudioRing::new(
            AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS as usize * AUDIO_BUFFER_SECONDS,
        ));
        let shared = Arc::new(DecoderShared::new());
        let (commands, receiver) = std::sync::mpsc::channel();

        spawn_audio_decoder(
            self.tools
                .as_ref()
                .map(|tools| tools.ffmpeg.clone())
                .unwrap_or_else(|| PathBuf::from("ffmpeg")),
            path.to_path_buf(),
            ring.clone(),
            receiver,
            shared.clone(),
        );

        let source = PcmSource {
            ring: ring.clone(),
            channels: AUDIO_CHANNELS,
            sample_rate: AUDIO_SAMPLE_RATE,
        };
        sink.append(source);
        sink.set_volume(1.0);

        Some(AudioSession {
            ring,
            sink,
            shared,
            commands,
        })
    }
}

/// Compute the preview resolution, capped to [`MAX_PREVIEW_WIDTH`].
fn preview_size(source: Option<(u32, u32)>) -> (u32, u32) {
    let Some((width, height)) = source else {
        return (2, 2);
    };
    if width == 0 || height == 0 {
        return (2, 2);
    }
    if width <= MAX_PREVIEW_WIDTH {
        // Keep the dimensions even, some decoders dislike odd sizes.
        return (width.max(2) & !1, height.max(2) & !1);
    }
    let scaled = (height as f64 * MAX_PREVIEW_WIDTH as f64 / width as f64).round() as u32;
    (MAX_PREVIEW_WIDTH & !1, scaled.max(2) & !1)
}

/// Pick the frame that should be visible at `position`.
fn pick_frame(frames: &Mutex<VecDeque<Arc<Frame>>>, position: f64, fps: f64) -> Option<Arc<Frame>> {
    let guard = frames.lock();
    if guard.is_empty() {
        return None;
    }
    let tolerance = 0.5 / fps.max(1.0);
    let mut chosen: Option<Arc<Frame>> = None;
    for frame in guard.iter() {
        if frame.pts <= position + tolerance {
            chosen = Some(frame.clone());
        } else {
            break;
        }
    }
    // The play head may sit slightly before the first decoded frame, for
    // example right after a seek. Showing that frame is better than nothing.
    chosen.or_else(|| guard.front().cloned())
}

/// Spawn the thread that decodes video frames.
#[allow(clippy::too_many_arguments)]
fn spawn_video_decoder(
    ffmpeg: PathBuf,
    input: PathBuf,
    size: (u32, u32),
    fps: f64,
    start: f64,
    frames: Arc<Mutex<VecDeque<Arc<Frame>>>>,
    commands: Receiver<VideoCmd>,
    shared: Arc<DecoderShared>,
    state: Arc<Mutex<PlaybackState>>,
    duration: f64,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let (frame_tx, frame_rx) = std::sync::mpsc::channel::<Arc<Frame>>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

        let reader_shared = shared.clone();
        let reader_frames = frames.clone();
        let reader = std::thread::spawn(move || {
            let frame_bytes = size.0 as usize * size.1 as usize * 3;
            let mut generation = 0_u64;
            let mut position = start;
            let mut stdout: Option<std::process::ChildStdout> = None;
            // The position the decoder starts at before the first restart.
            let mut pending_start = Some(start);

            loop {
                if stdout.is_none() {
                    let target = match reader_shared.take_request(generation) {
                        Some((newer, time)) => {
                            generation = newer;
                            time
                        }
                        None => match pending_start.take() {
                            Some(time) => time,
                            None => {
                                if stop_rx.try_recv().is_ok() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(4));
                                continue;
                            }
                        },
                    };
                    if stop_rx.try_recv().is_ok() {
                        break;
                    }
                    let mut child = match start_video_process(&ffmpeg, &input, size, fps, target) {
                        Ok(child) => child,
                        Err(error) => {
                            log::error!("could not start the video decoder: {error:#}");
                            break;
                        }
                    };
                    stdout = child.stdout.take();
                    *reader_shared.child.lock() = Some(child);
                    position = target;
                }

                // Respect back pressure so that the decoder does not run ahead.
                while reader_frames.lock().len() >= FRAME_QUEUE_CAPACITY {
                    if let Some((newer, time)) = reader_shared.take_request(generation) {
                        generation = newer;
                        position = time;
                        stdout = None;
                        reader_shared.terminate();
                        break;
                    }
                    if stop_rx.try_recv().is_ok() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(4));
                }
                if stdout.is_none() {
                    continue;
                }

                let mut data = vec![0_u8; frame_bytes];
                // Pace the decoder with the playback clock so that the
                // preview stays in sync with the sound.
                loop {
                    let clock = {
                        let guard = state.lock();
                        position_of(&guard, duration)
                    };
                    if clock >= position - 1e-6 {
                        break;
                    }
                    if stop_rx.try_recv().is_ok() {
                        return;
                    }
                    if let Some((newer, time)) = reader_shared.take_request(generation) {
                        generation = newer;
                        position = time;
                        stdout = None;
                        reader_shared.terminate();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                if stdout.is_none() {
                    continue;
                }

                let read = stdout
                    .as_mut()
                    .map(|stream| stream.read_exact(&mut data))
                    .unwrap_or(Ok(()));
                match read {
                    Ok(()) => {
                        let frame = Arc::new(Frame {
                            pts: position,
                            data: Arc::new(data),
                        });
                        position += 1.0 / fps.max(1.0);
                        if frame_tx.send(frame).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        stdout = None;
                        reader_shared.terminate();
                        if reader_shared.take_request(generation).is_none() {
                            break;
                        }
                    }
                }
            }
            reader_shared.terminate();
        });

        // Control thread: translates user commands into decoder restarts.
        let control_shared = shared.clone();
        let control = std::thread::spawn(move || {
            while let Ok(command) = commands.recv() {
                match command {
                    VideoCmd::Seek(time) => control_shared.request_seek(time),
                    VideoCmd::Stop => {
                        control_shared.terminate();
                        let _ = stop_tx.send(());
                        break;
                    }
                }
            }
            control_shared.terminate();
        });

        // Forward the decoded frames into the shared queue.
        while let Ok(frame) = frame_rx.recv() {
            let mut guard = frames.lock();
            guard.push_back(frame);
            while guard.len() > FRAME_QUEUE_CAPACITY {
                guard.pop_front();
            }
        }

        let _ = reader.join();
        let _ = control.join();
    })
}

/// Spawn the thread that decodes the audio track into the ring buffer.
fn spawn_audio_decoder(
    ffmpeg: PathBuf,
    input: PathBuf,
    ring: Arc<AudioRing>,
    commands: Receiver<AudioCmd>,
    shared: Arc<DecoderShared>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let reader_shared = shared.clone();
        let reader = std::thread::spawn(move || {
            let mut generation = 0_u64;
            let mut stdout: Option<std::process::ChildStdout> = None;
            let mut pending: Vec<f32> = Vec::with_capacity(4096);
            // The position the decoder starts at before the first restart.
            let mut pending_start = Some(0.0_f64);

            loop {
                if stdout.is_none() {
                    let target = match reader_shared.take_request(generation) {
                        Some((newer, time)) => {
                            generation = newer;
                            time
                        }
                        None => match pending_start.take() {
                            Some(time) => time,
                            None => {
                                if stop_rx.try_recv().is_ok() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(4));
                                continue;
                            }
                        },
                    };
                    if stop_rx.try_recv().is_ok() {
                        break;
                    }
                    let mut child = match start_audio_process(&ffmpeg, &input, target) {
                        Ok(child) => child,
                        Err(error) => {
                            log::error!("could not start the audio decoder: {error:#}");
                            break;
                        }
                    };
                    stdout = child.stdout.take();
                    *reader_shared.child.lock() = Some(child);
                }

                let mut bytes = vec![0_u8; 8192 * 4];
                let read = stdout
                    .as_mut()
                    .map(|stream| stream.read(&mut bytes))
                    .unwrap_or(Ok(0));
                match read {
                    Ok(0) => {
                        stdout = None;
                        reader_shared.terminate();
                        if reader_shared.take_request(generation).is_none() {
                            break;
                        }
                    }
                    Ok(count) => {
                        let count = count / 4 * 4;
                        pending.clear();
                        for slot in bytes[..count].chunks_exact(4) {
                            pending.push(f32::from_le_bytes([slot[0], slot[1], slot[2], slot[3]]));
                        }
                        if !pending.is_empty() {
                            ring.push(&pending);
                        }
                    }
                    Err(_) => {
                        stdout = None;
                        reader_shared.terminate();
                        if reader_shared.take_request(generation).is_none() {
                            break;
                        }
                    }
                }
            }
            reader_shared.terminate();
            ring.close();
        });

        let control_shared = shared.clone();
        let control = std::thread::spawn(move || {
            while let Ok(command) = commands.recv() {
                match command {
                    AudioCmd::Seek(time) => control_shared.request_seek(time),
                    AudioCmd::Stop => {
                        control_shared.terminate();
                        let _ = stop_tx.send(());
                        break;
                    }
                }
            }
            control_shared.terminate();
        });

        let _ = reader.join();
        let _ = control.join();
    })
}

/// Start the `ffmpeg` process that produces raw RGB frames.
fn start_video_process(
    ffmpeg: &Path,
    input: &Path,
    size: (u32, u32),
    fps: f64,
    start: f64,
) -> Result<Child> {
    let child = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .arg("-ss")
        .arg(format!("{start:.4}"))
        .arg("-i")
        .arg(input)
        .args(["-an", "-sn", "-dn", "-vf"])
        .arg(format!("scale={}:{}", size.0, size.1))
        .arg("-pix_fmt")
        .arg("rgb24")
        .arg("-r")
        .arg(format!("{fps:.6}"))
        .args(["-f", "rawvideo", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {} for decoding", ffmpeg.display()))?;
    Ok(child)
}

/// Start the `ffmpeg` process that produces interleaved float PCM.
fn start_audio_process(ffmpeg: &Path, input: &Path, start: f64) -> Result<Child> {
    let child = Command::new(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .arg("-ss")
        .arg(format!("{start:.4}"))
        .arg("-i")
        .arg(input)
        .args(["-vn", "-sn", "-dn", "-ac", "2", "-ar"])
        .arg(AUDIO_SAMPLE_RATE.to_string())
        .args(["-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {} for decoding", ffmpeg.display()))?;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_size_is_capped_and_even() {
        assert_eq!(preview_size(Some((1920, 1080))), (1280, 720));
        assert_eq!(preview_size(Some((640, 480))), (640, 480));
        assert_eq!(preview_size(Some((641, 481))), (640, 480));
        assert_eq!(preview_size(None), (2, 2));
        assert_eq!(preview_size(Some((0, 0))), (2, 2));
    }

    #[test]
    fn picks_the_frame_below_the_play_head() {
        let mut frames = VecDeque::new();
        frames.push_back(Arc::new(Frame {
            pts: 0.0,
            data: Arc::new(Vec::new()),
        }));
        frames.push_back(Arc::new(Frame {
            pts: 0.04,
            data: Arc::new(Vec::new()),
        }));
        frames.push_back(Arc::new(Frame {
            pts: 0.08,
            data: Arc::new(Vec::new()),
        }));
        let queue = Mutex::new(frames);
        let chosen = pick_frame(&queue, 0.05, 25.0).expect("a frame");
        assert!((chosen.pts - 0.04).abs() < 1e-6);
        let chosen = pick_frame(&queue, 0.0, 25.0).expect("a frame");
        assert!((chosen.pts - 0.0).abs() < 1e-6);
    }

    #[test]
    fn audio_ring_delivers_samples_in_order() {
        let ring = AudioRing::new(16);
        ring.push(&[0.5, -0.5, 1.0]);
        assert!(matches!(ring.pop(), Pop::Sample(value) if (value - 0.5).abs() < 1e-6));
        assert!(matches!(ring.pop(), Pop::Sample(value) if (value + 0.5).abs() < 1e-6));
        assert!(matches!(ring.pop(), Pop::Sample(value) if (value - 1.0).abs() < 1e-6));
        assert!(matches!(ring.pop(), Pop::Empty));
    }

    #[test]
    fn audio_ring_reports_closure() {
        let ring = AudioRing::new(16);
        ring.close();
        assert!(matches!(ring.pop(), Pop::Closed));
    }
}

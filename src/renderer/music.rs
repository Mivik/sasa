use crate::{buffer_is_full, AudioClip, Frame, Renderer};
use anyhow::{Context, Result};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Weak,
};

#[derive(Debug, Clone)]
pub struct MusicParams {
    pub loop_mix_time: f64,
    pub amplifier: f32,
    pub playback_rate: f64,
    pub command_buffer_size: usize,
}
impl Default for MusicParams {
    fn default() -> Self {
        Self {
            loop_mix_time: -1.,
            amplifier: 1.,
            playback_rate: 1.,
            command_buffer_size: 16,
        }
    }
}

struct SharedState {
    position: AtomicU64, // float in bits
    paused: AtomicBool,
}
impl Default for SharedState {
    fn default() -> Self {
        Self {
            position: AtomicU64::default(),
            paused: AtomicBool::new(true),
        }
    }
}

enum MusicCommand {
    Pause,
    Resume,
    SetAmplifier(f32),
    SeekTo(f64),
    SetLowPass(f32),
    FadeIn(f64),
    FadeOut(f64),
}
pub(crate) struct MusicRenderer {
    clip: AudioClip,
    settings: MusicParams,
    state: Weak<SharedState>,
    rx: mpsc::Receiver<MusicCommand>,
    paused: bool,
    index: usize,
    last_sample_rate: u32,
    low_pass: f32,
    last_output: Frame,

    fade_time: i32,
    fade_current: i32,
}
impl MusicRenderer {
    fn prepare(&mut self, sample_rate: u32) {
        if self.last_sample_rate != sample_rate {
            let factor = sample_rate as f32 / self.last_sample_rate as f32;
            self.index = (self.index as f32 * factor).round() as _;
            self.last_sample_rate = sample_rate;
            self.fade_time = (self.fade_time as f32 * factor).round() as _;
            self.fade_current = (self.fade_current as f32 * factor).round() as _;
        }
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                MusicCommand::Pause => {
                    self.paused = true;
                    if let Some(state) = self.state.upgrade() {
                        state.paused.store(true, Ordering::Relaxed);
                    }
                }
                MusicCommand::Resume => {
                    self.paused = false;
                    if let Some(state) = self.state.upgrade() {
                        state.paused.store(false, Ordering::Relaxed);
                    }
                }
                MusicCommand::SetAmplifier(amp) => {
                    self.settings.amplifier = amp;
                }
                MusicCommand::SeekTo(position) => {
                    self.index = (position * sample_rate as f64 / self.settings.playback_rate)
                        .round() as usize;
                }
                MusicCommand::SetLowPass(low_pass) => {
                    self.low_pass = low_pass;
                }
                MusicCommand::FadeIn(time) => {
                    if self.paused {
                        self.paused = false;
                        if let Some(state) = self.state.upgrade() {
                            state.paused.store(false, Ordering::Relaxed);
                        }
                    }
                    self.fade_time = (time * sample_rate as f64).round() as _;
                    self.fade_current = 0;
                }
                MusicCommand::FadeOut(time) => {
                    self.fade_time = (-time * sample_rate as f64).round() as _;
                    self.fade_current = 0;
                }
            }
        }
    }

    #[inline]
    fn frame(&mut self, position: f64, delta: f64) -> Option<Frame> {
        let s = &self.settings;
        if let Some(mut frame) = self.clip.sample(position) {
            if s.loop_mix_time >= 0. {
                let pos = position + s.loop_mix_time - self.clip.length();
                if pos >= 0. {
                    if let Some(new_frame) = self.clip.sample(pos) {
                        frame = frame + new_frame;
                    }
                }
            }
            self.index += 1;
            let mut amp = s.amplifier;
            if self.fade_time != 0 {
                if self.fade_time > 0 {
                    self.fade_current += 1;
                    if self.fade_current >= self.fade_time {
                        self.fade_time = 0;
                    } else {
                        amp *= self.fade_current as f32 / self.fade_time as f32;
                    }
                } else {
                    self.fade_current -= 1;
                    if self.fade_current <= self.fade_time {
                        self.fade_time = 0;
                        self.paused = true;
                        if let Some(state) = self.state.upgrade() {
                            state.paused.store(true, Ordering::Relaxed);
                        }
                        return None;
                    } else {
                        amp *= 1. - self.fade_current as f32 / self.fade_time as f32;
                    }
                }
            }
            Some(frame * amp)
        } else if s.loop_mix_time >= 0. {
            let position = position - self.clip.length() + s.loop_mix_time;
            self.index = (position / delta).round() as _;
            Some(if let Some(frame) = self.clip.sample(position) {
                frame * s.amplifier
            } else {
                Frame::default()
            })
        } else {
            self.paused = true;
            None
        }
    }

    #[inline]
    fn position(&self, delta: f64) -> f64 {
        self.index as f64 * delta
    }

    #[inline(always)]
    fn update_and_get(&mut self, frame: Frame) -> Frame {
        self.last_output = self.last_output * self.low_pass + frame * (1. - self.low_pass);
        self.last_output
    }
}

impl Renderer for MusicRenderer {
    fn alive(&self) -> bool {
        self.state.strong_count() != 0
    }

    fn render_mono(&mut self, sample_rate: u32, data: &mut [f32]) {
        self.prepare(sample_rate);
        if !self.paused {
            let delta = 1. / sample_rate as f64 * self.settings.playback_rate;
            let mut position = self.index as f64 * delta;
            for sample in data.iter_mut() {
                if let Some(frame) = self.frame(position, delta) {
                    *sample += self.update_and_get(frame).avg();
                } else {
                    break;
                }
                position += delta;
            }
            if let Some(state) = self.state.upgrade() {
                state
                    .position
                    .store(self.position(delta).to_bits(), Ordering::Relaxed);
            }
        }
    }

    fn render_stereo(&mut self, sample_rate: u32, data: &mut [f32]) {
        self.prepare(sample_rate);
        if !self.paused {
            let delta = 1. / sample_rate as f64 * self.settings.playback_rate as f64;
            let mut position = self.index as f64 * delta;
            for sample in data.chunks_exact_mut(2) {
                if let Some(frame) = self.frame(position, delta) {
                    let frame = self.update_and_get(frame);
                    sample[0] += frame.0;
                    sample[1] += frame.1;
                } else {
                    break;
                }
                position += delta;
            }
            if let Some(state) = self.state.upgrade() {
                state
                    .position
                    .store(self.position(delta).to_bits(), Ordering::Relaxed);
            }
        }
    }
}

pub struct Music {
    shared: Arc<SharedState>,
    tx: mpsc::SyncSender<MusicCommand>,
}
impl Music {
    pub(crate) fn new(clip: AudioClip, settings: MusicParams) -> (Music, MusicRenderer) {
        let (tx, rx) = mpsc::sync_channel(settings.command_buffer_size);
        let arc = Arc::default();
        let renderer = MusicRenderer {
            clip,
            settings,
            state: Arc::downgrade(&arc),
            rx,
            paused: true,
            index: 0,
            last_sample_rate: 1,
            low_pass: 0.,
            last_output: Frame(0., 0.),

            fade_time: 0,
            fade_current: 0,
        };
        (Self { shared: arc, tx }, renderer)
    }

    pub fn play(&mut self) -> Result<()> {
        self.tx
            .send(MusicCommand::Resume)
            .map_err(buffer_is_full)
            .context("play music")
    }

    pub fn pause(&mut self) -> Result<()> {
        self.tx
            .send(MusicCommand::Pause)
            .map_err(buffer_is_full)
            .context("pause")
    }

    pub fn paused(&mut self) -> bool {
        self.shared.paused.load(Ordering::Relaxed)
    }

    pub fn set_amplifier(&mut self, amp: f32) -> Result<()> {
        self.tx
            .send(MusicCommand::SetAmplifier(amp))
            .map_err(buffer_is_full)
            .context("set amplifier")
    }

    pub fn seek_to(&mut self, position: f64) -> Result<()> {
        self.tx
            .send(MusicCommand::SeekTo(position))
            .map_err(buffer_is_full)
            .context("seek to")
    }

    pub fn set_low_pass(&mut self, low_pass: f32) -> Result<()> {
        self.tx
            .send(MusicCommand::SetLowPass(low_pass))
            .map_err(buffer_is_full)
            .context("set low pass")
    }

    pub fn fade_in(&mut self, time: f64) -> Result<()> {
        self.tx
            .send(MusicCommand::FadeIn(time))
            .map_err(buffer_is_full)
            .context("fade in")
    }

    pub fn fade_out(&mut self, time: f64) -> Result<()> {
        self.tx
            .send(MusicCommand::FadeOut(time))
            .map_err(buffer_is_full)
            .context("fade out")
    }

    pub fn position(&self) -> f64 {
        f64::from_bits(self.shared.position.load(Ordering::Relaxed))
    }
}

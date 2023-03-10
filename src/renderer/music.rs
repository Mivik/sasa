use crate::{buffer_is_full, AudioClip, Frame, Renderer};
use anyhow::{Context, Result};
use ringbuf::{HeapConsumer, HeapProducer, HeapRb};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Weak,
};

#[derive(Debug, Clone)]
pub struct MusicParams {
    pub loop_: bool,
    pub amplifier: f32,
    pub playback_rate: f32,
    pub command_buffer_size: usize,
}
impl Default for MusicParams {
    fn default() -> Self {
        Self {
            loop_: false,
            amplifier: 1.,
            playback_rate: 1.,
            command_buffer_size: 16,
        }
    }
}

struct SharedState {
    position: AtomicU32, // float in bits
    paused: AtomicBool,
}
impl Default for SharedState {
    fn default() -> Self {
        Self {
            position: AtomicU32::default(),
            paused: AtomicBool::new(true),
        }
    }
}

enum MusicCommand {
    Pause,
    Resume,
    SeekTo(f32),
    SetLowPass(f32),
}
pub(crate) struct MusicRenderer {
    clip: AudioClip,
    settings: MusicParams,
    state: Weak<SharedState>,
    cons: HeapConsumer<MusicCommand>,
    paused: bool,
    index: usize,
    last_sample_rate: u32,
    low_pass: f32,
    last_output: Frame,
}
impl MusicRenderer {
    fn prepare(&mut self, sample_rate: u32) {
        if self.last_sample_rate != sample_rate {
            self.index = (self.index as f32 * (sample_rate as f32 / self.last_sample_rate as f32))
                .round() as usize;
            self.last_sample_rate = sample_rate;
        }
        for cmd in self.cons.pop_iter() {
            match cmd {
                MusicCommand::Pause => {
                    self.paused = true;
                    if let Some(state) = self.state.upgrade() {
                        state.paused.store(true, Ordering::SeqCst);
                    }
                }
                MusicCommand::Resume => {
                    self.paused = false;
                    if let Some(state) = self.state.upgrade() {
                        state.paused.store(false, Ordering::SeqCst);
                    }
                }
                MusicCommand::SeekTo(position) => {
                    self.index = (position * sample_rate as f32 / self.settings.playback_rate)
                        .round() as usize;
                }
                MusicCommand::SetLowPass(low_pass) => {
                    self.low_pass = low_pass;
                }
            }
        }
    }

    #[inline]
    fn frame(&mut self, position: f32) -> Option<Frame> {
        let s = &self.settings;
        if let Some(frame) = self.clip.sample(position) {
            self.index += 1;
            Some(frame * s.amplifier)
        } else if s.loop_ {
            self.index = 1;
            Some(if let Some(frame) = self.clip.sample(0.) {
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
    fn position(&self, delta: f32) -> f32 {
        self.index as f32 * delta
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
            let delta = 1. / sample_rate as f64 * self.settings.playback_rate as f64;
            let mut position = self.index as f64 * delta;
            for sample in data.iter_mut() {
                if let Some(frame) = self.frame(position as f32) {
                    *sample += self.update_and_get(frame).avg();
                } else {
                    break;
                }
                position += delta;
            }
            if let Some(state) = self.state.upgrade() {
                state
                    .position
                    .store(self.position(delta as f32).to_bits(), Ordering::SeqCst);
            }
        }
    }

    fn render_stereo(&mut self, sample_rate: u32, data: &mut [f32]) {
        self.prepare(sample_rate);
        if !self.paused {
            let delta = 1. / sample_rate as f64 * self.settings.playback_rate as f64;
            let mut position = self.index as f64 * delta;
            for sample in data.chunks_exact_mut(2) {
                if let Some(frame) = self.frame(position as f32) {
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
                    .store(self.position(delta as f32).to_bits(), Ordering::SeqCst);
            }
        }
    }
}

pub struct Music {
    arc: Arc<SharedState>,
    prod: HeapProducer<MusicCommand>,
}
impl Music {
    pub(crate) fn new(clip: AudioClip, settings: MusicParams) -> (Music, MusicRenderer) {
        let (prod, cons) = HeapRb::new(settings.command_buffer_size).split();
        let arc = Arc::default();
        let renderer = MusicRenderer {
            clip,
            settings,
            state: Arc::downgrade(&arc),
            cons,
            paused: true,
            index: 0,
            last_sample_rate: 1,
            low_pass: 0.,
            last_output: Frame(0., 0.),
        };
        (Self { arc, prod }, renderer)
    }

    pub fn play(&mut self) -> Result<()> {
        self.prod
            .push(MusicCommand::Resume)
            .map_err(buffer_is_full)
            .context("play music")?;
        Ok(())
    }

    pub fn pause(&mut self) -> Result<()> {
        self.prod
            .push(MusicCommand::Pause)
            .map_err(buffer_is_full)
            .context("pause")?;
        Ok(())
    }

    pub fn paused(&mut self) -> bool {
        self.arc.paused.load(Ordering::SeqCst)
    }

    pub fn seek_to(&mut self, position: f32) -> Result<()> {
        self.prod
            .push(MusicCommand::SeekTo(position))
            .map_err(buffer_is_full)
            .context("seek to")?;
        Ok(())
    }

    pub fn set_low_pass(&mut self, low_pass: f32) -> Result<()> {
        self.prod
            .push(MusicCommand::SetLowPass(low_pass))
            .map_err(buffer_is_full)
            .context("set low pass")?;
        Ok(())
    }

    pub fn position(&self) -> f32 {
        f32::from_bits(self.arc.position.load(Ordering::SeqCst))
    }
}

use crate::{buffer_is_full, AudioClip, Renderer};
use anyhow::{Context, Result};
use ringbuf::{HeapConsumer, HeapProducer, HeapRb};
use std::sync::{Arc, Weak};

#[derive(Debug, Clone)]
pub struct PlaySfxParams {
    pub amplifier: f32,
}
impl Default for PlaySfxParams {
    fn default() -> Self {
        Self { amplifier: 1. }
    }
}

pub(crate) struct SfxRenderer {
    clip: AudioClip,
    arc: Weak<()>,
    cons: HeapConsumer<(f32, PlaySfxParams)>,
}

impl Renderer for SfxRenderer {
    fn alive(&self) -> bool {
        !self.cons.is_empty() || self.arc.strong_count() != 0
    }

    fn render_mono(&mut self, sample_rate: u32, data: &mut [f32]) {
        let delta = 1. / sample_rate as f32;
        let mut pop_count = 0;
        for (position, params) in self.cons.iter_mut() {
            for sample in data.iter_mut() {
                if let Some(frame) = self.clip.sample(*position) {
                    *sample += frame.avg() * params.amplifier;
                } else {
                    pop_count += 1;
                    break;
                }
                *position += delta;
            }
        }
        unsafe {
            self.cons.advance(pop_count);
        }
    }

    fn render_stereo(&mut self, sample_rate: u32, data: &mut [f32]) {
        let delta = 1. / sample_rate as f32;
        let mut pop_count = 0;
        for (position, params) in self.cons.iter_mut() {
            for sample in data.chunks_exact_mut(2) {
                if let Some(frame) = self.clip.sample(*position) {
                    sample[0] += frame.0 * params.amplifier;
                    sample[1] += frame.1 * params.amplifier;
                } else {
                    pop_count += 1;
                    break;
                }
                *position += delta;
            }
        }
        unsafe {
            self.cons.advance(pop_count);
        }
    }
}

pub struct Sfx {
    _arc: Arc<()>,
    prod: HeapProducer<(f32, PlaySfxParams)>,
}
impl Sfx {
    pub(crate) fn new(clip: AudioClip, buffer_size: Option<usize>) -> (Sfx, SfxRenderer) {
        let (prod, cons) = HeapRb::new(buffer_size.unwrap_or(64)).split();
        let arc = Arc::new(());
        let renderer = SfxRenderer {
            clip,
            arc: Arc::downgrade(&arc),
            cons,
        };
        (Self { _arc: arc, prod }, renderer)
    }

    pub fn play(&mut self, params: PlaySfxParams) -> Result<()> {
        self.prod
            .push((0., params))
            .map_err(buffer_is_full)
            .context("play sfx")
    }
}

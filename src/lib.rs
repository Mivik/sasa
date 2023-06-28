/// Simple And Stupid Audio for Rust, optimized for low latency.
pub mod backend;
pub use backend::Backend;

mod clip;
pub use clip::AudioClip;

mod mixer;

mod renderer;
pub use renderer::{Music, MusicParams, PlaySfxParams, Renderer, Sfx, Stretcher};

use crate::{backend::BackendSetup, mixer::MixerCommand};
use anyhow::{anyhow, Context, Result};
use ringbuf::{HeapProducer, HeapRb};
use std::{
    ops::{Add, Mul},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};

fn buffer_is_full<E>(_: E) -> anyhow::Error {
    anyhow!("buffer is full")
}

#[derive(Clone, Copy, Default)]
pub struct Frame(pub f32, pub f32);
impl Frame {
    pub fn avg(&self) -> f32 {
        (self.0 + self.1) / 2.
    }

    pub fn interpolate(&self, other: &Self, f: f32) -> Self {
        Self(
            self.0 + (other.0 - self.0) * f,
            self.1 + (other.1 - self.1) * f,
        )
    }
}
impl Add for Frame {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0, self.1 + rhs.1)
    }
}
impl Mul<f32> for Frame {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self(self.0 * rhs, self.1 * rhs)
    }
}

const LATENCY_RECORD_NUM: usize = 64;

pub struct LatencyRecorder {
    records: [f32; LATENCY_RECORD_NUM],
    head: usize,
    sum: f32,
    full: bool,
    result: Arc<AtomicU32>,
}

impl LatencyRecorder {
    pub fn new(result: Arc<AtomicU32>) -> Self {
        Self {
            records: [0.; LATENCY_RECORD_NUM],
            head: 0,
            sum: 0.,
            full: false,
            result,
        }
    }

    pub fn push(&mut self, record: f32) {
        let place = &mut self.records[self.head];
        self.sum += record - *place;
        *place = record;
        self.head += 1;
        if self.head == LATENCY_RECORD_NUM {
            self.full = true;
            self.head = 0;
        }
        self.result.store(
            (self.sum
                / (if self.full {
                    LATENCY_RECORD_NUM
                } else {
                    self.head.max(1)
                }) as f32)
                .to_bits(),
            Ordering::SeqCst,
        );
    }
}

pub struct AudioManager {
    backend: Box<dyn Backend>,
    latency: Arc<AtomicU32>,
    prod: HeapProducer<MixerCommand>,
}

impl AudioManager {
    pub fn new(backend: impl Backend + 'static) -> Result<Self> {
        Self::new_box(Box::new(backend))
    }

    pub fn new_box(mut backend: Box<dyn Backend>) -> Result<Self> {
        let (prod, cons) = HeapRb::new(16).split();
        let latency = Arc::default();
        let latency_rec = LatencyRecorder::new(Arc::clone(&latency));
        backend.setup(BackendSetup {
            mixer_cons: cons,
            latency_rec,
        })?;
        backend.start()?;
        Ok(Self {
            backend,
            latency,
            prod,
        })
    }

    pub fn create_sfx(&mut self, clip: AudioClip, buffer_size: Option<usize>) -> Result<Sfx> {
        let (sfx, sfx_renderer) = Sfx::new(clip, buffer_size);
        self.add_renderer(sfx_renderer)?;
        Ok(sfx)
    }

    pub fn create_music(&mut self, clip: AudioClip, settings: MusicParams) -> Result<Music> {
        let (music, music_renderer) = Music::new(clip, settings);
        self.add_renderer(music_renderer)?;
        Ok(music)
    }

    pub fn add_renderer(&mut self, renderer: impl Renderer + 'static) -> Result<()> {
        self.prod
            .push(MixerCommand::AddRenderer(Box::new(renderer)))
            .map_err(buffer_is_full)
            .context("add renderer")?;
        Ok(())
    }

    pub fn estimate_latency(&self) -> f32 {
        f32::from_bits(self.latency.load(Ordering::SeqCst))
    }

    #[inline(always)]
    pub fn consume_broken(&self) -> bool {
        self.backend.consume_broken()
    }

    #[inline(always)]
    pub fn start(&mut self) -> Result<()> {
        self.backend.start()
    }

    pub fn recover_if_needed(&mut self) -> Result<()> {
        if self.consume_broken() {
            self.start()
        } else {
            Ok(())
        }
    }
}

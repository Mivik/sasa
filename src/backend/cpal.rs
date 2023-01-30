use crate::Backend;
use anyhow::{anyhow, Context, Result};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    BufferSize, OutputCallbackInfo, Stream, StreamError,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use super::{BackendSetup, StateCell};

#[derive(Debug, Clone, Default)]
pub struct CpalSettings {
    pub buffer_size: Option<u32>,
}

pub struct CpalBackend {
    settings: CpalSettings,
    stream: Option<Stream>,
    broken: Arc<AtomicBool>,
    state: Option<Arc<StateCell>>,
}

impl CpalBackend {
    pub fn new(settings: CpalSettings) -> Self {
        Self {
            settings,
            stream: None,
            broken: Arc::default(),
            state: None,
        }
    }
}

impl Backend for CpalBackend {
    fn setup(&mut self, setup: BackendSetup) -> Result<()> {
        self.state = Some(Arc::new(setup.into()));
        Ok(())
    }

    fn start(&mut self) -> Result<()> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device is found"))?;
        let mut config = device
            .default_output_config()
            .context("cannot get output config")?
            .config();
        config.buffer_size = self
            .settings
            .buffer_size
            .map_or(BufferSize::Default, |it| BufferSize::Fixed(it));

        let broken = Arc::clone(&self.broken);
        let error_callback = move |err| {
            eprintln!("audio error: {err:?}");
            if matches!(err, StreamError::DeviceNotAvailable) {
                broken.store(true, Ordering::Relaxed);
            }
        };
        let state = Arc::clone(self.state.as_ref().unwrap());
        state.get().0.sample_rate = config.sample_rate.0;
        let stream = (if config.channels == 1 {
            device.build_output_stream(
                &config,
                move |data: &mut [f32], info: &OutputCallbackInfo| {
                    let (mixer, rec) = state.get();
                    mixer.render_mono(data);
                    let ts = info.timestamp();
                    if let Some(delay) = ts.playback.duration_since(&ts.callback) {
                        rec.push(delay.as_secs_f32());
                    }
                },
                error_callback,
            )
        } else {
            device.build_output_stream(
                &config,
                move |data: &mut [f32], info: &OutputCallbackInfo| {
                    let (mixer, rec) = state.get();
                    mixer.render_stereo(data);
                    let ts = info.timestamp();
                    if let Some(delay) = ts.playback.duration_since(&ts.callback) {
                        rec.push(delay.as_secs_f32());
                    }
                },
                error_callback,
            )
        })
        .context("failed to build stream")?;
        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    fn consume_broken(&self) -> bool {
        self.broken.fetch_and(false, Ordering::Relaxed)
    }
}

pub use oboe::{PerformanceMode, Usage};

use super::{BackendSetup, StateCell};
use crate::Backend;
use anyhow::Result;
use oboe::{
    AudioFormat, AudioOutputCallback, AudioOutputStreamSafe, AudioStream, AudioStreamAsync, AudioStreamBuilder, DataCallbackResult, Output, SharingMode, Stereo, Unspecified
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub struct OboeSettings {
    pub buffer_size: Option<u32>,
    pub performance_mode: PerformanceMode,
    pub usage: Usage,
}
impl Default for OboeSettings {
    fn default() -> Self {
        Self {
            buffer_size: None,
            performance_mode: PerformanceMode::None,
            usage: Usage::Media,
        }
    }
}

pub struct OboeBackend {
    settings: OboeSettings,
    stream: Option<AudioStreamAsync<Output, OboeCallback>>,
    state: Option<Arc<StateCell>>,
    broken: Arc<AtomicBool>,
}

impl OboeBackend {
    pub fn new(settings: OboeSettings) -> Self {
        Self {
            settings,
            stream: None,
            state: None,
            broken: Arc::default(),
        }
    }
}

impl Backend for OboeBackend {
    fn setup(&mut self, setup: BackendSetup) -> Result<()> {
        self.state = Some(Arc::new(setup.into()));
        Ok(())
    }

    fn start(&mut self) -> Result<()> {
        let mut stream = AudioStreamBuilder::default()
            .set_usage(self.settings.usage)
            .set_performance_mode(self.settings.performance_mode)
            .set_sharing_mode(SharingMode::Exclusive)
            //.set_format::<f32>()
            .set_channel_count::<Stereo>()
            .set_callback(OboeCallback::new(
                Arc::clone(self.state.as_ref().unwrap()),
                Arc::clone(&self.broken),
                self.settings.buffer_size,
            ))
            .open_stream()
            .unwrap();
        stream.start()?;
        self.stream = Some(stream);
        Ok(())
    }

    fn consume_broken(&self) -> bool {
        self.broken.fetch_and(false, Ordering::Relaxed)
    }
}

struct OboeCallback {
    state: Arc<StateCell>,
    broken: Arc<AtomicBool>,
    buffer_size: Option<u32>,
}

impl OboeCallback {
    pub fn new(state: Arc<StateCell>, broken: Arc<AtomicBool>, buffer_size: Option<u32>) -> Self {
        Self {
            state,
            broken,
            buffer_size,
        }
    }
}

impl AudioOutputCallback for OboeCallback {
    type FrameType = (Unspecified, Stereo);

    fn on_audio_ready(
        &mut self,
        stream: &mut dyn AudioOutputStreamSafe,
        frames: &mut [(oboe::Unspecified, oboe::Unspecified)],
    ) -> DataCallbackResult {
        if let Some(buffer_size) = &self.buffer_size {
            let _ = stream.set_buffer_size_in_frames(
                (*buffer_size as i32).min(stream.get_buffer_size_in_frames()),
            );
        }

        let (mixer, rec) = self.state.get();
        if let Ok(latency) = stream.calculate_latency_millis() {
            rec.push((latency / 1000.) as f32);
        }
        mixer.sample_rate = stream.get_sample_rate() as u32;
        let raw = frames.as_mut_ptr();
        mixer.render_stereo(unsafe {
            std::slice::from_raw_parts_mut(raw as *mut f32, frames.len() * 2)
        });

        DataCallbackResult::Continue
    }

    fn on_error_before_close(
        &mut self,
        _audio_stream: &mut dyn oboe::AudioOutputStreamSafe,
        error: oboe::Error,
    ) {
        eprintln!("audio error: {error:?}");
        self.broken.store(true, Ordering::Relaxed);
    }

    fn on_error_after_close(
        &mut self,
        _audio_stream: &mut dyn oboe::AudioOutputStreamSafe,
        error: oboe::Error,
    ) {
        eprintln!("audio error: {error:?}");
        self.broken.store(true, Ordering::Relaxed);
    }
}

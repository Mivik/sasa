use anyhow::Result;
use std::{
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use ohos_audio_sys::{
    OH_AudioInterrupt_ForceType, OH_AudioInterrupt_ForceType_AUDIOSTREAM_INTERRUPT_FORCE,
    OH_AudioInterrupt_Hint, OH_AudioInterrupt_Hint_AUDIOSTREAM_INTERRUPT_HINT_PAUSE,
    OH_AudioInterrupt_Hint_AUDIOSTREAM_INTERRUPT_HINT_STOP, OH_AudioRenderer,
    OH_AudioRenderer_Callbacks, OH_AudioRenderer_GetSamplingRate, OH_AudioRenderer_Release,
    OH_AudioRenderer_Start, OH_AudioStreamBuilder, OH_AudioStreamBuilder_Create,
    OH_AudioStreamBuilder_Destroy, OH_AudioStreamBuilder_GenerateRenderer,
    OH_AudioStreamBuilder_SetChannelCount, OH_AudioStreamBuilder_SetFrameSizeInCallback,
    OH_AudioStreamBuilder_SetLatencyMode, OH_AudioStreamBuilder_SetRendererCallback,
    OH_AudioStreamBuilder_SetRendererInfo, OH_AudioStreamBuilder_SetSampleFormat,
    OH_AudioStreamBuilder_SetSamplingRate,
    OH_AudioStream_LatencyMode_AUDIOSTREAM_LATENCY_MODE_FAST,
    OH_AudioStream_SampleFormat_AUDIOSTREAM_SAMPLE_F32LE,
    OH_AudioStream_Type_AUDIOSTREAM_TYPE_RENDERER, OH_AudioStream_Usage_AUDIOSTREAM_USAGE_GAME,
};

use super::{BackendSetup, StateCell};
use crate::Backend;

#[derive(Debug, Clone)]
pub struct OhosSettings {
    pub buffer_size: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: u16,
}

impl Default for OhosSettings {
    fn default() -> Self {
        Self {
            buffer_size: None,
            sample_rate: None,
            channels: 2,
        }
    }
}

pub struct OhosBackend {
    settings: OhosSettings,
    state: Option<Arc<StateCell>>,
    broken: Arc<AtomicBool>,
    stream: Option<*mut OH_AudioRenderer>,
}

impl OhosBackend {
    pub fn new(settings: OhosSettings) -> Self {
        Self {
            settings,
            state: None,
            broken: Arc::default(),
            stream: None,
        }
    }

    pub fn settings(&self) -> &OhosSettings {
        &self.settings
    }
}

impl Backend for OhosBackend {
    fn setup(&mut self, setup: BackendSetup) -> Result<()> {
        self.state = Some(Arc::new(setup.into()));
        Ok(())
    }

    fn start(&mut self) -> Result<()> {
        unsafe {
            let mut builder: *mut OH_AudioStreamBuilder = ptr::null_mut();
            let sample_rate = self.settings.sample_rate.unwrap_or(48000) as i32;
            let channels = self.settings.channels as i32;
            OH_AudioStreamBuilder_Create(
                &mut builder as *mut *mut OH_AudioStreamBuilder,
                OH_AudioStream_Type_AUDIOSTREAM_TYPE_RENDERER,
            );
            OH_AudioStreamBuilder_SetSamplingRate(builder, sample_rate);
            OH_AudioStreamBuilder_SetChannelCount(builder, channels);
            OH_AudioStreamBuilder_SetSampleFormat(
                builder,
                OH_AudioStream_SampleFormat_AUDIOSTREAM_SAMPLE_F32LE,
            );
            OH_AudioStreamBuilder_SetLatencyMode(
                builder,
                OH_AudioStream_LatencyMode_AUDIOSTREAM_LATENCY_MODE_FAST,
            );
            // 设置渲染器使用场景为游戏
            OH_AudioStreamBuilder_SetRendererInfo(
                builder,
                OH_AudioStream_Usage_AUDIOSTREAM_USAGE_GAME,
            );

            if let Some(buffer_size) = self.settings.buffer_size {
                OH_AudioStreamBuilder_SetFrameSizeInCallback(builder, buffer_size as i32);
            }

            let callback_data = Box::new(OhosCallbackData::new(
                Arc::clone(self.state.as_ref().unwrap()),
                Arc::clone(&self.broken),
                self.settings.channels,
            ));
            let user_data = Box::into_raw(callback_data) as *mut c_void;

            let callbacks = OH_AudioRenderer_Callbacks {
                OH_AudioRenderer_OnWriteData: Some(audio_renderer_on_write_data),
                OH_AudioRenderer_OnStreamEvent: None,
                OH_AudioRenderer_OnInterruptEvent: Some(audio_renderer_on_interrupt),
                OH_AudioRenderer_OnError: None,
            };

            OH_AudioStreamBuilder_SetRendererCallback(builder, callbacks, user_data);
            let mut renderer: *mut OH_AudioRenderer = ptr::null_mut();
            OH_AudioStreamBuilder_GenerateRenderer(builder, &mut renderer);
            OH_AudioStreamBuilder_Destroy(builder);
            let mut actual_sample_rate: i32 = 0;
            OH_AudioRenderer_GetSamplingRate(renderer, &mut actual_sample_rate);
            self.state.as_ref().unwrap().get().0.sample_rate = actual_sample_rate as u32;
            OH_AudioRenderer_Start(renderer);
            self.stream = Some(renderer);
            Ok(())
        }
    }

    fn consume_broken(&self) -> bool {
        self.broken.fetch_and(false, Ordering::Relaxed)
    }
}

struct OhosCallbackData {
    state: Arc<StateCell>,
    broken: Arc<AtomicBool>,
    channels: u16,
}

impl OhosCallbackData {
    fn new(state: Arc<StateCell>, broken: Arc<AtomicBool>, channels: u16) -> Self {
        Self {
            state,
            broken,
            channels,
        }
    }
}

extern "C" fn audio_renderer_on_write_data(
    _renderer: *mut OH_AudioRenderer,
    user_data: *mut c_void,
    buffer: *mut c_void,
    length: i32,
) -> i32 {
    if user_data.is_null() || buffer.is_null() || length <= 0 {
        return -1;
    }

    let callback_data = unsafe { &mut *(user_data as *mut OhosCallbackData) };
    let (mixer, _rec) = callback_data.state.get();

    let sample_count = length as usize / size_of::<f32>();

    let f32_buffer = unsafe { std::slice::from_raw_parts_mut(buffer as *mut f32, sample_count) };

    if callback_data.channels == 1 {
        mixer.render_mono(f32_buffer);
    } else {
        mixer.render_stereo(f32_buffer);
    }

    0
}

extern "C" fn audio_renderer_on_interrupt(
    _renderer: *mut OH_AudioRenderer,
    user_data: *mut c_void,
    force_type: OH_AudioInterrupt_ForceType,
    hint: OH_AudioInterrupt_Hint,
) -> i32 {
    if user_data.is_null() {
        return -1;
    }

    let callback_data = unsafe { &*(user_data as *mut OhosCallbackData) };

    if matches!(
        hint,
        OH_AudioInterrupt_Hint_AUDIOSTREAM_INTERRUPT_HINT_PAUSE
            | OH_AudioInterrupt_Hint_AUDIOSTREAM_INTERRUPT_HINT_STOP
    ) {
        callback_data.broken.store(true, Ordering::Relaxed);
    }

    0
}

impl Drop for OhosBackend {
    fn drop(&mut self) {
        if let Some(renderer) = self.stream {
            unsafe {
                OH_AudioRenderer_Release(renderer);
            }
        }
    }
}

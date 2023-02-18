use crate::Frame;
use anyhow::{anyhow, bail, Result};
use std::{io::Cursor, sync::Arc};
use symphonia::core::{
    audio::{AudioBufferRef, Signal},
    io::MediaSourceStream,
};

struct ClipInner {
    frames: Vec<Frame>,
    sample_rate: u32,
}
pub struct AudioClip(Arc<ClipInner>);
impl Clone for AudioClip {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl AudioClip {
    pub fn from_raw(frames: Vec<Frame>, sample_rate: u32) -> Self {
        Self(Arc::new(ClipInner {
            frames,
            sample_rate,
        }))
    }

    pub fn decode(data: Vec<u8>) -> Result<(Vec<Frame>, u32)> {
        fn load_frames_from_buffer(
            frames: &mut Vec<Frame>,
            buffer: &symphonia::core::audio::AudioBuffer<f32>,
        ) {
            match buffer.spec().channels.count() {
                1 => {
                    let chan = buffer.chan(0);
                    frames.reserve(chan.len());
                    frames.extend(chan.iter().map(|&it| Frame(it, it)));
                }
                _ => {
                    let iter = buffer.chan(0).iter().zip(buffer.chan(1));
                    frames.reserve(iter.len());
                    frames.extend(iter.map(|(left, right)| Frame(*left, *right)))
                }
            }
        }

        fn load_frames_from_buffer_ref(
            frames: &mut Vec<Frame>,
            buffer: &AudioBufferRef,
        ) -> Result<()> {
            macro_rules! conv {
                ($buffer:ident) => {{
                    let mut dest = symphonia::core::audio::AudioBuffer::new(
                        buffer.capacity() as u64,
                        buffer.spec().clone(),
                    );
                    $buffer.convert(&mut dest);
                    load_frames_from_buffer(frames, &dest);
                }};
            }
            use AudioBufferRef::*;
            match buffer {
                F32(buffer) => load_frames_from_buffer(frames, buffer),
                U8(buffer) => conv!(buffer),
                U16(buffer) => conv!(buffer),
                U24(buffer) => conv!(buffer),
                U32(buffer) => conv!(buffer),
                S8(buffer) => conv!(buffer),
                S16(buffer) => conv!(buffer),
                S24(buffer) => conv!(buffer),
                S32(buffer) => conv!(buffer),
                F64(buffer) => conv!(buffer),
            }
            Ok(())
        }
        let codecs = symphonia::default::get_codecs();
        let probe = symphonia::default::get_probe();
        let mss = MediaSourceStream::new(Box::new(Cursor::new(data)), Default::default());
        let mut format_reader = probe
            .format(
                &Default::default(),
                mss,
                &Default::default(),
                &Default::default(),
            )?
            .format;
        let codec_params = &format_reader
            .default_track()
            .ok_or_else(|| anyhow!("default track not found"))?
            .codec_params;
        let sample_rate = codec_params
            .sample_rate
            .ok_or_else(|| anyhow!("unknown sample rate"))?;
        let mut decoder = codecs.make(codec_params, &Default::default())?;
        let mut frames = Vec::new();
        loop {
            match format_reader.next_packet() {
                Ok(packet) => {
                    let buffer = match decoder.decode(&packet) {
                        Ok(buffer) => buffer,
                        Err(symphonia::core::errors::Error::DecodeError(s))
                            if s.contains("invalid main_data offset") =>
                        {
                            continue;
                        }
                        Err(err) => return Err(err.into()),
                    };
                    load_frames_from_buffer_ref(&mut frames, &buffer)?;
                }
                Err(error) => match error {
                    symphonia::core::errors::Error::IoError(error)
                        if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        break;
                    }
                    _ => bail!(error),
                },
            }
        }
        Ok((frames, sample_rate))
    }

    #[inline]
    pub fn new(data: Vec<u8>) -> Result<Self> {
        let (frames, sample_rate) = Self::decode(data)?;
        Ok(Self::from_raw(frames, sample_rate))
    }

    pub fn sample(&self, position: f32) -> Option<Frame> {
        let position = position * self.0.sample_rate as f32;
        let actual_index = position as usize;
        if let Some(frame) = self.0.frames.get(actual_index) {
            let next_frame = self.0.frames.get(actual_index + 1).unwrap_or(frame);
            Some(frame.interpolate(next_frame, position - actual_index as f32))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn frames(&self) -> &[Frame] {
        &self.0.frames
    }

    #[inline(always)]
    pub fn sample_rate(&self) -> u32 {
        self.0.sample_rate
    }

    #[inline(always)]
    pub fn frame_count(&self) -> usize {
        self.0.frames.len()
    }

    pub fn length(&self) -> f32 {
        self.frame_count() as f32 / self.sample_rate() as f32
    }
}

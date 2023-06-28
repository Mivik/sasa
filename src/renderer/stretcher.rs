/*
 * Implementation of Phase Vocoder-based Time Stretching Algorithm
 *
 * Copyright (c) 2023 Rong "Mantle" Bao <baorong2005@126.com>
 * Copyright (c) 2021 Andrew Yoon
 * Copyright (c) 2014 Nasca Octavian Paul
 *
 * Derived from [paulstretch_python](https://github.com/paulnasca/paulstretch_python)
 * - License: Public Domain
 * Derived from [rocoder](https://github.com/ajyoon/rocoder)
 * - License: CC0 1.0 Universal
 */

use rand::Rng;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use slice_deque::SliceDeque;
use std::sync::Arc;

const TWO_PI: f32 = std::f32::consts::PI * 2.0;

struct ReFFT {
    forward_fft: Arc<dyn Fft<f32>>,
    inverse_fft: Arc<dyn Fft<f32>>,
    window_len: usize,
    window: Vec<f32>,
}

impl ReFFT {
    pub fn new(window: Vec<f32>) -> ReFFT {
        let window_len = window.len();
        let mut planner = FftPlanner::new();
        let forward_fft = planner.plan_fft_forward(window_len);
        let inverse_fft = planner.plan_fft_inverse(window_len);
        ReFFT {
            forward_fft,
            inverse_fft,
            window_len,
            window,
        }
    }

    pub fn resynth(&mut self, samples: &[f32]) -> Vec<f32> {
        self.resynth_from_fft(self.forward_fft(samples))
    }

    fn forward_fft(&self, samples: &[f32]) -> Vec<Complex32> {
        let mut buf: Vec<Complex32> = samples
            .iter()
            .zip(&self.window)
            .map(|(s, w)| Complex32::new(s * w, 0.0))
            .collect();
        if buf.len() < self.window_len {
            buf.extend(vec![Complex32::new(0.0, 0.0); self.window_len - buf.len()]);
        }
        self.forward_fft.process(&mut buf);
        buf
    }

    fn resynth_from_fft(&self, fft_result: Vec<Complex32>) -> Vec<f32> {
        let mut rng = rand::thread_rng();
        let mut buf: Vec<Complex32> = fft_result
            .iter()
            .map(|c| Complex32::new(0.0, rng.gen_range(0.0..TWO_PI)).exp() * c.norm())
            .collect();
        self.inverse_fft.process(&mut buf);
        buf.iter()
            .zip(&self.window)
            .map(|(c, w)| (c.re / self.window_len as f32) * w)
            .collect()
    }
}

/// The 'upper part' of a cosine period
pub fn hanning(len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| 0.5 - (f32::cos((i as f32 * TWO_PI) / (len - 1) as f32) * 0.5))
        .collect()
}

/// Taken from paulstretch
fn hanning_crossfade_compensation(len: usize) -> Vec<f32> {
    let hinv_sqrt2 = (1.0 + f32::sqrt(0.5).sqrt()) * 0.5;
    (0..len)
        .map(|i| 0.5 - ((1.0 - hinv_sqrt2) * f32::cos((i as f32 * TWO_PI) / (len - 1) as f32)))
        .collect()
}

/// Concurrent Vocoder for one channel of audio.
pub struct Stretcher {
    pub sample_rate: u32,
    input_buf: SliceDeque<f32>,
    output_buf: SliceDeque<f32>,
    corrected_amp_factor: f32,
    amp_correction_envelope: Vec<f32>,
    re_fft: ReFFT,
    window_len: usize,
    half_window_len: usize,
    samples_needed_per_window: usize,
    sample_step_len: usize,
    done: bool,
}

impl Stretcher {
    pub fn new(sample_rate: u32, input: &[f32], factor: f32) -> Stretcher {
        let window = hanning(8192);
        let window_len = window.len();
        let half_window_len = window_len / 2;
        Stretcher {
            sample_rate,
            corrected_amp_factor: (4.0f32).max(factor / 4.0),
            amp_correction_envelope: hanning_crossfade_compensation(window.len() / 2),
            re_fft: ReFFT::new(window),
            window_len,
            half_window_len,
            samples_needed_per_window: window_len,
            sample_step_len: (window_len as f32 / (factor * 2.0)) as _,
            output_buf: SliceDeque::from(vec![0.0; half_window_len].as_slice()),
            input_buf: SliceDeque::from(input),
            done: false,
        }
    }

    pub const fn is_done(&self) -> bool {
        self.done
    }

    pub fn next_window(&mut self) -> Vec<f32> {
        let mut iter_output_buf_pos = 0;
        while self.output_buf.len() < self.samples_needed_per_window + self.half_window_len {
            // Generate output one half-window at a time, with each step leaving a half window
            // from the fade-out half of the window function for the next iteration to pick up.
            self.ensure_input_samples_available(self.window_len);
            let fft_result = self.re_fft.resynth(&self.input_buf[..self.window_len]);
            for i in 0..self.half_window_len {
                self.output_buf[iter_output_buf_pos + i] = (fft_result[i]
                    + self.output_buf[iter_output_buf_pos + i])
                    * self.amp_correction_envelope[i]
                    * self.corrected_amp_factor;
            }
            self.output_buf
                .extend_from_slice(&fft_result[self.half_window_len..]);
            iter_output_buf_pos += self.half_window_len;
            self.input_buf
                .truncate_front(self.input_buf.len() - self.sample_step_len);
        }
        let result = self.output_buf[..self.samples_needed_per_window].to_vec();
        self.output_buf.truncate_front(self.half_window_len);
        result
    }

    pub fn ensure_input_samples_available(&mut self, n: usize) {
        if self.input_buf.len() < n {
            self.input_buf.resize(n, 0.0);
            self.done = true;
        }
    }
}

impl Iterator for Stretcher {
    type Item = Vec<f32>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_done() {
            None
        } else {
            Some(self.next_window())
        }
    }
}

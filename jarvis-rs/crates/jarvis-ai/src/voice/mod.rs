//! Microphone capture for push-to-talk voice input.
//!
//! [`VoiceRecorder`] opens the default input device via `cpal`, buffers samples
//! while held, and on stop returns a complete 16-bit PCM WAV byte vector ready
//! to hand straight to the Whisper client (`WhisperClient::transcribe`).
//!
//! The capture stream is kept alive inside the recorder for the duration of the
//! recording and dropped on [`VoiceRecorder::stop`] (or when the recorder is
//! dropped). Samples are mixed down to a single mono channel and encoded with
//! the device's native sample rate written into the WAV header, so the file is
//! always valid regardless of the hardware rate.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use tracing::{debug, warn};

use crate::AiError;

/// Shared buffer of captured mono samples in `[-1.0, 1.0]`.
type SampleBuffer = Arc<Mutex<Vec<f32>>>;

/// A live microphone recorder.
///
/// Construct + start with [`VoiceRecorder::start`]; the device stream begins
/// capturing immediately and runs until [`VoiceRecorder::stop`] is called (or
/// the recorder is dropped). `stop` returns the recorded audio as a complete
/// WAV byte vector (16-bit PCM, mono, at the capture device's sample rate).
pub struct VoiceRecorder {
    /// The active capture stream. Held so the callback keeps firing; dropped on
    /// `stop`. `cpal::Stream` is `!Send`, so the recorder must live on the
    /// thread that started it (the app's main/UI thread).
    stream: Stream,
    /// Accumulated mono samples, filled by the stream callback.
    samples: SampleBuffer,
    /// The device's capture sample rate (Hz), written into the WAV header.
    sample_rate: u32,
}

impl VoiceRecorder {
    /// Open the default input device and begin capturing.
    ///
    /// Returns an error if there is no default input device, no supported input
    /// config, or the stream cannot be built/started. On success the stream is
    /// already running.
    pub fn start() -> Result<Self, AiError> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| AiError::ApiError("no default input device".into()))?;

        let supported = device
            .default_input_config()
            .map_err(|e| AiError::ApiError(format!("no default input config: {e}")))?;

        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels().max(1) as usize;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        debug!(
            sample_rate,
            channels,
            ?sample_format,
            "Voice recorder opening input stream"
        );

        let samples: SampleBuffer = Arc::new(Mutex::new(Vec::new()));
        let err_fn = |e| warn!(error = %e, "voice input stream error");

        // Mix every frame's channels down to a single mono sample (average).
        let build_mono = |buf: SampleBuffer, channels: usize| {
            move |data: &[f32]| {
                if let Ok(mut out) = buf.lock() {
                    if channels <= 1 {
                        out.extend_from_slice(data);
                    } else {
                        for frame in data.chunks(channels) {
                            let sum: f32 = frame.iter().copied().sum();
                            out.push(sum / frame.len() as f32);
                        }
                    }
                }
            }
        };

        let stream = match sample_format {
            SampleFormat::F32 => {
                let cb = build_mono(samples.clone(), channels);
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _| cb(data),
                    err_fn,
                    None,
                )
            }
            SampleFormat::I16 => {
                let buf = samples.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        if let Ok(mut out) = buf.lock() {
                            if channels <= 1 {
                                out.extend(data.iter().map(|&s| s as f32 / i16::MAX as f32));
                            } else {
                                for frame in data.chunks(channels) {
                                    let sum: f32 =
                                        frame.iter().map(|&s| s as f32 / i16::MAX as f32).sum();
                                    out.push(sum / frame.len() as f32);
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let buf = samples.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        if let Ok(mut out) = buf.lock() {
                            let to_f32 = |s: u16| (s as f32 - 32768.0) / 32768.0;
                            if channels <= 1 {
                                out.extend(data.iter().map(|&s| to_f32(s)));
                            } else {
                                for frame in data.chunks(channels) {
                                    let sum: f32 = frame.iter().map(|&s| to_f32(s)).sum();
                                    out.push(sum / frame.len() as f32);
                                }
                            }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            other => {
                return Err(AiError::ApiError(format!(
                    "unsupported input sample format: {other:?}"
                )));
            }
        }
        .map_err(|e| AiError::ApiError(format!("failed to build input stream: {e}")))?;

        stream
            .play()
            .map_err(|e| AiError::ApiError(format!("failed to start input stream: {e}")))?;

        Ok(Self {
            stream,
            samples,
            sample_rate,
        })
    }

    /// Stop capturing and return the recorded audio as a complete WAV byte
    /// vector (16-bit PCM, mono, at the capture device's sample rate).
    ///
    /// Dropping the stream halts the callback; any samples buffered so far are
    /// encoded. An empty recording yields a valid, zero-data WAV.
    pub fn stop(self) -> Vec<u8> {
        // Drop the stream first so no further callbacks mutate the buffer.
        drop(self.stream);
        let samples = self
            .samples
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default();
        debug!(
            samples = samples.len(),
            sample_rate = self.sample_rate,
            "Voice recorder stopped"
        );
        encode_wav_mono(&samples, self.sample_rate)
    }
}

/// Encode mono `f32` samples (`[-1.0, 1.0]`) into a 16-bit PCM WAV byte vector.
///
/// Writes a canonical 44-byte RIFF/`WAVE` header (`fmt ` + `data` chunks) with
/// the given `sample_rate` and a single channel, followed by little-endian
/// 16-bit samples. Out-of-range inputs are clamped.
pub fn encode_wav_mono(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;
    let bytes_per_sample = (BITS_PER_SAMPLE / 8) as u32;
    let block_align = CHANNELS as u32 * bytes_per_sample;
    let byte_rate = sample_rate * block_align;
    let data_len = samples.len() as u32 * bytes_per_sample;
    // RIFF chunk size = 4 ("WAVE") + (8 + 16 fmt) + (8 + data_len).
    let riff_len = 36 + data_len;

    let mut out = Vec::with_capacity(44 + data_len as usize);

    // RIFF header
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_len.to_le_bytes());
    out.extend_from_slice(b"WAVE");

    // fmt chunk (PCM)
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    out.extend_from_slice(&CHANNELS.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&(block_align as u16).to_le_bytes());
    out.extend_from_slice(&BITS_PER_SAMPLE.to_le_bytes());

    // data chunk
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let v = (clamped * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read a little-endian u32 at `offset`.
    fn u32_at(b: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([b[offset], b[offset + 1], b[offset + 2], b[offset + 3]])
    }

    /// Read a little-endian u16 at `offset`.
    fn u16_at(b: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([b[offset], b[offset + 1]])
    }

    #[test]
    fn wav_header_is_valid_for_synthetic_buffer() {
        // 8 synthetic samples spanning the full range.
        let samples: Vec<f32> = vec![0.0, 0.5, -0.5, 1.0, -1.0, 0.25, -0.25, 0.0];
        let sample_rate = 16_000u32;
        let wav = encode_wav_mono(&samples, sample_rate);

        // 44-byte header + 2 bytes per sample.
        assert_eq!(wav.len(), 44 + samples.len() * 2);

        // RIFF magic + WAVE form type.
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");

        // RIFF chunk size = 36 + data_len.
        let data_len = (samples.len() * 2) as u32;
        assert_eq!(u32_at(&wav, 4), 36 + data_len);

        // fmt chunk.
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32_at(&wav, 16), 16); // PCM fmt chunk size
        assert_eq!(u16_at(&wav, 20), 1); // audio format = PCM
        assert_eq!(u16_at(&wav, 22), 1); // mono
        assert_eq!(u32_at(&wav, 24), sample_rate);
        assert_eq!(u32_at(&wav, 28), sample_rate * 2); // byte rate = rate * block_align
        assert_eq!(u16_at(&wav, 32), 2); // block align = channels * bytes/sample
        assert_eq!(u16_at(&wav, 34), 16); // bits per sample

        // data chunk header + length.
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32_at(&wav, 40), data_len);
    }

    #[test]
    fn wav_samples_are_encoded_and_clamped() {
        let samples = vec![0.0f32, 1.0, -1.0, 2.0, -2.0];
        let wav = encode_wav_mono(&samples, 44_100);
        // First sample (0.0) -> 0.
        assert_eq!(u16_at(&wav, 44), 0);
        // 1.0 -> i16::MAX.
        assert_eq!(u16_at(&wav, 46) as i16, i16::MAX);
        // -1.0 -> -i16::MAX (32767), not i16::MIN.
        assert_eq!(u16_at(&wav, 48) as i16, -i16::MAX);
        // 2.0 clamps to 1.0 -> i16::MAX.
        assert_eq!(u16_at(&wav, 50) as i16, i16::MAX);
        // -2.0 clamps to -1.0 -> -i16::MAX.
        assert_eq!(u16_at(&wav, 52) as i16, -i16::MAX);
    }

    #[test]
    fn empty_recording_is_a_valid_zero_data_wav() {
        let wav = encode_wav_mono(&[], 48_000);
        assert_eq!(wav.len(), 44);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32_at(&wav, 4), 36); // 36 + 0 data
        assert_eq!(u32_at(&wav, 40), 0); // data length = 0
        assert_eq!(u32_at(&wav, 24), 48_000); // sample rate preserved
    }

    #[test]
    fn sample_rate_is_written_into_header() {
        for rate in [8_000u32, 16_000, 22_050, 44_100, 48_000] {
            let wav = encode_wav_mono(&[0.1, -0.1], rate);
            assert_eq!(u32_at(&wav, 24), rate, "sample rate {rate} mismatch");
            assert_eq!(u32_at(&wav, 28), rate * 2, "byte rate for {rate} mismatch");
        }
    }
}

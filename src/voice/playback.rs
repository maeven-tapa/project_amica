use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use cpal::{
    ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, StreamTrait},
};
use tokio::sync::mpsc;

use crate::microphone::pcm::{Resampler, SpeechFrame, SpeechFrames};

pub const REPLY_RATE: u32 = 24_000;

#[derive(Default)]
struct Output {
    samples: VecDeque<f32>,
    audible_until: Option<Instant>,
}

pub struct Playback {
    _stream: Stream,
    output: Arc<Mutex<Output>>,
    failed: Arc<AtomicBool>,
    resampler: Resampler,
    rate: u32,
}

impl Playback {
    pub fn open(reference: mpsc::Sender<SpeechFrame>, output: &str) -> Result<Self, &'static str> {
        let device = crate::settings::device(crate::settings::AudioKind::Speaker, output)
            .ok_or("Connect speakers or headphones and try voice again.")?;
        let supported = device
            .default_output_config()
            .map_err(|_| "Amica could not access the default speakers.")?;
        let config = supported.config();
        let output = Arc::new(Mutex::new(Output::default()));
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => output_stream::<f32>(&device, config, &output, &failed, reference),
            SampleFormat::F64 => output_stream::<f64>(&device, config, &output, &failed, reference),
            SampleFormat::I16 => output_stream::<i16>(&device, config, &output, &failed, reference),
            SampleFormat::I24 => {
                output_stream::<cpal::I24>(&device, config, &output, &failed, reference)
            }
            SampleFormat::I32 => output_stream::<i32>(&device, config, &output, &failed, reference),
            SampleFormat::I64 => output_stream::<i64>(&device, config, &output, &failed, reference),
            SampleFormat::U8 => output_stream::<u8>(&device, config, &output, &failed, reference),
            _ => {
                return Err(
                    "This speaker format is unsupported. Choose a different output device.",
                );
            }
        }
        .map_err(|_| "Amica could not open the default speakers.")?;
        stream
            .play()
            .map_err(|_| "Amica could not start speaker playback.")?;
        Ok(Self {
            _stream: stream,
            output,
            failed,
            resampler: Resampler::new(REPLY_RATE, config.sample_rate),
            rate: config.sample_rate,
        })
    }

    pub fn enqueue(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if !bytes.len().is_multiple_of(2) {
            return Err("Gemini Live returned incomplete audio samples.");
        }
        let mut output = self.output.lock().unwrap();
        let added = bytes.len() as u64 / 2 * self.rate as u64 / REPLY_RATE as u64;
        if output.samples.len() as u64 + added > u64::from(self.rate) * 60 {
            return Err("Speaker playback fell too far behind. Unmute to reconnect.");
        }
        for pair in bytes.chunks_exact(2) {
            let sample = i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0;
            self.resampler
                .push(sample, |sample| output.samples.push_back(sample));
        }
        Ok(())
    }

    pub fn clear(&mut self) {
        *self.output.lock().unwrap() = Output::default();
        self.resampler = Resampler::new(REPLY_RATE, self.rate);
    }

    pub fn playing(&self) -> bool {
        let output = self.output.lock().unwrap();
        !output.samples.is_empty()
            || output
                .audible_until
                .is_some_and(|instant| Instant::now() < instant)
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

fn output_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    output: &Arc<Mutex<Output>>,
    failed: &Arc<AtomicBool>,
    reference: mpsc::Sender<SpeechFrame>,
) -> Result<Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let output = Arc::clone(output);
    let failed = Arc::clone(failed);
    let mut frames = SpeechFrames::new(config.sample_rate);
    device.build_output_stream(
        config,
        move |samples: &mut [T], info| {
            let now = Instant::now();
            let timestamp = info.timestamp();
            let mut output = output.try_lock().ok();
            let mut played = false;
            for frame in samples.chunks_mut(config.channels as usize) {
                let sample = output
                    .as_mut()
                    .and_then(|output| output.samples.pop_front())
                    .inspect(|_| played = true)
                    .unwrap_or(0.0);
                frame.fill(T::from_sample(sample));
                frames.push(sample, |frame| {
                    let _ = reference.try_send(frame);
                });
            }
            if played && let Some(output) = output.as_mut() {
                let lead = timestamp.playback.duration_since(timestamp.callback);
                let duration = Duration::from_secs_f64(
                    (samples.len() / config.channels as usize) as f64 / config.sample_rate as f64,
                );
                let until = now + (lead + duration).max(Duration::from_millis(85));
                output.audible_until =
                    Some(output.audible_until.map_or(until, |old| old.max(until)));
            }
        },
        move |error| {
            if !matches!(
                error.kind(),
                ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged
            ) {
                failed.store(true, Ordering::Relaxed);
            }
        },
        None,
    )
}

pub mod pcm;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use tokio::sync::mpsc;

use pcm::{SpeechFrame, SpeechFrames};

#[derive(Default)]
pub struct AudioTap {
    active: AtomicBool,
    capture: Mutex<Option<Capture>>,
}

struct Capture {
    sender: mpsc::Sender<SpeechFrame>,
    ready: Arc<AtomicBool>,
    frames: Option<(u32, SpeechFrames)>,
}

impl AudioTap {
    pub fn subscribe(&self, sender: mpsc::Sender<SpeechFrame>, ready: Arc<AtomicBool>) {
        *self.capture.lock().unwrap() = Some(Capture {
            sender,
            ready,
            frames: None,
        });
    }

    pub fn unsubscribe(&self) {
        self.capture.lock().unwrap().take();
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn forward<T: Sample>(&self, samples: &[T], rate: u32, channels: usize)
    where
        f32: FromSample<T>,
    {
        let Ok(mut slot) = self.capture.try_lock() else {
            return;
        };
        let Some(capture) = slot.as_mut() else { return };
        if !capture.ready.load(Ordering::Acquire) {
            return;
        }

        if capture
            .frames
            .as_ref()
            .is_none_or(|(saved_rate, _)| *saved_rate != rate)
        {
            capture.frames = Some((rate, SpeechFrames::new(rate)));
        }
        let (_, frames) = capture.frames.as_mut().unwrap();
        for frame in samples.chunks_exact(channels) {
            let mono = frame
                .iter()
                .map(|sample| sample.to_sample::<f32>())
                .sum::<f32>()
                / channels as f32;
            frames.push(mono, |speech| {
                let _ = capture.sender.try_send(speech);
            });
        }
    }
}

use cpal::{
    ErrorKind, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, StreamTrait},
};

#[derive(Default)]
struct Meter {
    peak: AtomicU32,
    failed: AtomicBool,
}

impl Meter {
    fn measure<T: Sample>(&self, samples: &[T])
    where
        f32: FromSample<T>,
    {
        if samples.is_empty() {
            return;
        }

        let energy = samples
            .iter()
            .map(|sample| {
                let amplitude = sample.to_sample::<f32>();
                amplitude * amplitude
            })
            .sum::<f32>();
        let rms = (energy / samples.len() as f32).sqrt();
        if rms.is_finite() && rms > 0.0 {
            let dbfs = 20.0 * rms.log10();
            let level = ((dbfs + 54.0) / 48.0).clamp(0.0, 1.0);
            self.peak.fetch_max(level.to_bits(), Ordering::Relaxed);
        }
    }

    fn take_level(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }
}

pub struct Microphone {
    _stream: Stream,
    meter: Arc<Meter>,
    tap: Arc<AudioTap>,
    pub name: String,
}

impl Microphone {
    pub fn open(tap: Arc<AudioTap>, input: &str) -> Result<Self, &'static str> {
        let device = crate::settings::device(crate::settings::AudioKind::Microphone, input)
            .ok_or("Connect a microphone or choose a default input in Windows Sound settings.")?;
        let config = device
            .default_input_config()
            .map_err(|_| "Check your default input in Windows Sound settings, then retry.")?;
        let meter = Arc::new(Meter::default());
        let stream_config = config.config();

        let stream = match config.sample_format() {
            SampleFormat::F32 => input_stream::<f32>(&device, stream_config, &meter, &tap),
            SampleFormat::F64 => input_stream::<f64>(&device, stream_config, &meter, &tap),
            SampleFormat::I8 => input_stream::<i8>(&device, stream_config, &meter, &tap),
            SampleFormat::I16 => input_stream::<i16>(&device, stream_config, &meter, &tap),
            SampleFormat::I24 => input_stream::<cpal::I24>(&device, stream_config, &meter, &tap),
            SampleFormat::I32 => input_stream::<i32>(&device, stream_config, &meter, &tap),
            SampleFormat::I64 => input_stream::<i64>(&device, stream_config, &meter, &tap),
            SampleFormat::U8 => input_stream::<u8>(&device, stream_config, &meter, &tap),
            _ => {
                return Err("This input format is unsupported. Choose another default microphone.");
            }
        }
        .map_err(|error| {
            eprintln!("Could not open microphone: {error}");
            "Check microphone access in Windows settings, then retry."
        })?;

        stream.play().map_err(|error| {
            eprintln!("Could not start microphone: {error}");
            "The microphone could not start. Check Windows Sound settings, then retry."
        })?;

        let name = device
            .description()
            .map(|description| description.name().to_owned())
            .unwrap_or_else(|_| "System default input".into());

        tap.active.store(true, Ordering::Release);
        Ok(Self {
            _stream: stream,
            meter,
            tap,
            name,
        })
    }

    pub fn take_level(&self) -> Option<f32> {
        if self.meter.failed.load(Ordering::Relaxed) {
            return None;
        }
        Some(self.meter.take_level())
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        self.tap.active.store(false, Ordering::Release);
        if let Some(capture) = self.tap.capture.lock().unwrap().as_mut() {
            capture.frames = None;
        }
    }
}

fn input_stream<T: SizedSample>(
    device: &cpal::Device,
    config: StreamConfig,
    meter: &Arc<Meter>,
    tap: &Arc<AudioTap>,
) -> Result<Stream, cpal::Error>
where
    f32: FromSample<T>,
{
    let levels = Arc::clone(meter);
    let errors = Arc::clone(meter);
    let tap = Arc::clone(tap);
    device.build_input_stream(
        config,
        move |samples: &[T], _| {
            levels.measure(samples);
            if tap.is_active() {
                tap.forward(samples, config.sample_rate, config.channels as usize);
            }
        },
        move |error| {
            if matches!(
                error.kind(),
                ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged
            ) {
                return;
            }
            eprintln!("Microphone disconnected: {error}");
            errors.failed.store(true, Ordering::Relaxed);
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_reports_audio_energy_and_returns_to_silence() {
        let meter = Meter::default();
        meter.measure(&[0.0_f32; 128]);
        assert_eq!(meter.take_level(), 0.0);

        meter.measure(&[0.1_f32, -0.1, 0.1, -0.1]);
        meter.measure(&[0.002_f32; 128]);
        assert!((meter.take_level() - 34.0 / 48.0).abs() < 0.001);
        assert_eq!(meter.take_level(), 0.0);
    }
}

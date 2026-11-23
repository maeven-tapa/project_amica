pub const SPEECH_RATE: u32 = 16_000;
pub type SpeechFrame = [f32; (SPEECH_RATE / 100) as usize];

pub struct Resampler {
    source_rate: u32,
    target_rate: u32,
    remaining: u32,
    weighted: f32,
}

impl Resampler {
    pub fn new(source_rate: u32, target_rate: u32) -> Self {
        Self {
            source_rate,
            target_rate,
            remaining: source_rate,
            weighted: 0.0,
        }
    }

    pub fn push(&mut self, sample: f32, mut emit: impl FnMut(f32)) {
        let mut ticks = self.target_rate;
        while ticks >= self.remaining {
            self.weighted += sample * self.remaining as f32;
            emit(self.weighted / self.source_rate as f32);
            ticks -= self.remaining;
            self.remaining = self.source_rate;
            self.weighted = 0.0;
        }
        self.remaining -= ticks;
        self.weighted += sample * ticks as f32;
    }
}

pub struct SpeechFrames {
    resampler: Resampler,
    frame: SpeechFrame,
    filled: usize,
}

impl SpeechFrames {
    pub fn new(source_rate: u32) -> Self {
        Self {
            resampler: Resampler::new(source_rate, SPEECH_RATE),
            frame: [0.0; 160],
            filled: 0,
        }
    }

    pub fn push(&mut self, sample: f32, mut emit: impl FnMut(SpeechFrame)) {
        self.resampler.push(sample, |sample| {
            self.frame[self.filled] = sample;
            self.filled += 1;
            if self.filled == self.frame.len() {
                emit(self.frame);
                self.filled = 0;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packetizes_fractional_and_integer_input_rates_without_drift() {
        for rate in [8_000, 16_000, 44_100, 48_000] {
            let mut frames = SpeechFrames::new(rate);
            let mut count = 0;
            for _ in 0..rate {
                frames.push(0.25, |frame| {
                    assert!(frame.iter().all(|sample| (*sample - 0.25).abs() < 0.0001));
                    count += 1;
                });
            }
            assert_eq!(count, 100);
        }
    }
}

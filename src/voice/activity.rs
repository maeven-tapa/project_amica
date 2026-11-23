use aec3::audio_processing::agc2::{
    cpu_features::get_available_cpu_features, vad_wrapper::VoiceActivityDetectorWrapper,
};

use crate::microphone::pcm::{SPEECH_RATE, SpeechFrame};

pub(super) struct SpeechActivity {
    detector: VoiceActivityDetectorWrapper,
    voiced_frames: u8,
    peak: f32,
}

impl SpeechActivity {
    pub(super) fn new() -> Self {
        Self {
            detector: VoiceActivityDetectorWrapper::new(
                get_available_cpu_features(),
                SPEECH_RATE as usize,
            ),
            voiced_frames: 0,
            peak: 0.0,
        }
    }

    pub(super) fn capture(&mut self, frame: &SpeechFrame) {
        let analysis = frame.map(|sample| sample * 32768.0);
        let probability = self.detector.analyze(&[&analysis]);
        let level = envelope(frame);
        if probability >= 0.62 && level > 0.0 {
            self.voiced_frames = (self.voiced_frames + 1).min(3);
            if self.voiced_frames >= 2 {
                self.peak = self.peak.max(level);
            }
        } else {
            self.voiced_frames = self.voiced_frames.saturating_sub(1);
        }
    }

    pub(super) fn render(&mut self, frame: &SpeechFrame) {
        self.peak = self.peak.max(envelope(frame));
    }

    pub(super) fn take_level(&mut self) -> f32 {
        std::mem::take(&mut self.peak)
    }
}

fn envelope(frame: &SpeechFrame) -> f32 {
    let mean_square = frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32;
    ((mean_square.sqrt() - 0.0035) * 7.0).clamp(0.0, 1.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_and_quiet_background_do_not_drive_the_speech_meter() {
        let mut activity = SpeechActivity::new();
        for frame in [[0.0; 160], [0.001; 160]] {
            for _ in 0..20 {
                activity.capture(&frame);
                activity.render(&frame);
            }
            assert_eq!(activity.take_level(), 0.0);
        }
        let reply = std::array::from_fn(|index| (index as f32 * 0.09).sin() * 0.12);
        activity.render(&reply);
        assert!(activity.take_level() > 0.5);
        assert_eq!(activity.take_level(), 0.0);
    }
}

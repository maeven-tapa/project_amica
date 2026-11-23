use std::time::{Duration, Instant};

use slint::{ComponentHandle, Timer, TimerMode};

use crate::AmicaWindow;

use super::{graph::GraphScene, ripple_border::RippleBorder};

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const SPEECH_HOLD: Duration = Duration::from_millis(48);

pub fn connect(window: &AmicaWindow) {
    let timer = Timer::default();
    let weak = window.as_weak();
    window.on_memory_visibility_changed(move |visible| {
        let Some(window) = weak.upgrade() else { return };
        if !visible {
            timer.stop();
            window.set_speech_level(0.0);
            window.set_memory_pulse(0.0);
            window.set_memory_ripple("".into());
            return;
        }

        let mut graph = GraphScene::default();
        let mut border = RippleBorder::default();
        let mut last_tick = Instant::now();
        let mut speech = None;
        graph.update(&window, 0.0);

        let weak = window.as_weak();
        timer.start(TimerMode::Repeated, FRAME_INTERVAL, move || {
            let Some(window) = weak.upgrade() else { return };
            let now = Instant::now();
            let elapsed = (now - last_tick).as_secs_f32().min(0.08);
            last_tick = now;

            let peak = window.get_speech_level();
            if peak > 0.0 {
                window.set_speech_level(0.0);
                speech = Some((peak, now));
            }
            if window.window().is_minimized() {
                return;
            }
            if window.get_reduced_motion() {
                graph.update(&window, 0.0);
                window.set_memory_pulse(0.0);
                window.set_memory_ripple("".into());
                return;
            }

            let level = speech
                .filter(|&(_, time)| now - time < SPEECH_HOLD)
                .map_or(0.0, |(level, _)| level);
            graph.update(&window, elapsed);
            if let Some((ripple, pulse)) = border.update(
                window.get_memory_width(),
                window.get_memory_height(),
                level,
                elapsed,
            ) {
                window.set_memory_ripple(ripple.into());
                window.set_memory_pulse(pulse);
            }
        });
    });
}

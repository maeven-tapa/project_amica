#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod actions;
mod app;
mod brand;
mod chat;
mod daily;
mod desktop;
mod hardware;
mod local_time;
mod media;
mod memory;
mod microphone;
mod orb;
mod routines;
mod screensaver;
mod settings;
mod storage;
mod timers;
mod usage;
mod voice;

use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use microphone::{AudioTap, Microphone};
use orb::{Orb, PointerMotion};
use slint::{ComponentHandle, Timer, TimerMode};

slint::include_modules!();

fn open_microphone(window: &AmicaWindow, tap: &Arc<AudioTap>) -> Option<Microphone> {
    match Microphone::open(Arc::clone(tap), &window.get_input_device_id()) {
        Ok(microphone) => {
            window.set_device_name(microphone.name.as_str().into());
            window.set_audio_message("Speak to see the orb respond.".into());
            window.set_audio_state(AudioState::Listening);
            Some(microphone)
        }
        Err(message) => {
            show_unavailable(window, message);
            None
        }
    }
}

fn show_unavailable(window: &AmicaWindow, message: &str) {
    window.set_input_level(0.0);
    window.set_device_name("Unavailable".into());
    window.set_audio_message(message.into());
    window.set_audio_state(AudioState::Unavailable);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--set-gemini-key") {
        chat::store_from_stdin()?;
        println!("Gemini API key saved in the system credential store.");
        return Ok(());
    }

    let window = AmicaWindow::new()?;
    window.set_linux_desktop(cfg!(target_os = "linux"));
    usage::start();
    app::connect(&window);
    desktop::connect(&window);
    hardware::connect(&window);
    let _media = media::connect(&window);
    settings::connect(&window);
    memory::connect(&window);
    chat::connect(&window);
    actions::connect(&window);
    let _dispatcher = actions::dispatcher::connect(&window);
    routines::connect(&window);
    let _clock = local_time::connect(&window);
    let _saver = screensaver::connect(&window);
    let tap = Arc::new(AudioTap::default());
    voice::connect(&window, Arc::clone(&tap));
    let orb = Orb::new();
    let mut pointer = PointerMotion::default();
    window.set_orb(orb.frame(0.0, 0.0, false, pointer));
    window.set_dashboard_orb(orb.dashboard_frame());

    window.set_device_name("Microphone off".into());
    window.set_audio_message("Talk to start voice conversation.".into());
    let microphone = Rc::new(RefCell::new(None::<Microphone>));

    window.on_apply_audio_device({
        let weak = window.as_weak();
        let microphone = Rc::clone(&microphone);
        let tap = Arc::clone(&tap);
        move |input| {
            let Some(window) = weak.upgrade() else { return };
            let mut microphone = microphone.borrow_mut();
            if microphone.is_none() {
                return;
            }
            window.set_audio_state(AudioState::Muted);
            window.invoke_sync_voice();
            if input {
                microphone.take();
                *microphone = open_microphone(&window, &tap);
            } else {
                window.set_audio_state(AudioState::Listening);
            }
            drop(microphone);
            window.invoke_sync_voice();
        }
    });

    window.on_toggle_microphone({
        let weak = window.as_weak();
        let microphone = Rc::clone(&microphone);
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut microphone = microphone.borrow_mut();
            if microphone.take().is_some() {
                window.set_input_level(0.0);
                window.set_audio_message("Unmute to start listening.".into());
                window.set_audio_state(AudioState::Muted);
            } else {
                *microphone = open_microphone(&window, &tap);
            }
            drop(microphone);
            window.invoke_sync_voice();
        }
    });
    window.invoke_sync_voice();

    let start = Instant::now();
    let mut last_frame = start;
    let mut last_thinking = false;
    let mut was_charging = false;
    let mut level = 0.0;
    let mut maps_were_open = false;
    let animation = Timer::default();
    animation.start(TimerMode::Repeated, Duration::from_millis(40), {
        let weak = window.as_weak();
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut microphone = microphone.borrow_mut();
            if let Some(input) = microphone.as_ref() {
                if let Some(target) = input.take_level() {
                    let response = if target > level { 0.58 } else { 0.19 };
                    level += (target - level) * response;
                    window.set_input_level(level);
                } else {
                    *microphone = None;
                    show_unavailable(
                        &window,
                        "The audio input stopped. Select a microphone and retry.",
                    );
                    window.invoke_sync_voice();
                    level = 0.0;
                }
            } else {
                level = 0.0;
            }

            if window.window().is_minimized() || window.get_saver_active() {
                return;
            }
            if window.get_memory_open() {
                maps_were_open = true;
                pointer = PointerMotion::default();
                return;
            }

            let now = Instant::now();
            let time = (now - start).as_secs_f32();
            let reopened_orb = std::mem::take(&mut maps_were_open);
            let pointer_active = pointer.update(
                window
                    .get_orb_hovered()
                    .then(|| (window.get_orb_pointer_x(), window.get_orb_pointer_y())),
            );
            let voice = window.get_voice_state();
            let thinking = window.get_thinking() || voice == VoiceState::Thinking;
            let speaking = voice == VoiceState::Speaking;
            let charging = window.get_memory_charging() || window.get_memory_flash();
            let reacting = microphone.is_some()
                && (level > 0.035 || now - last_frame >= Duration::from_millis(75));
            if window.get_reduced_motion() && !reopened_orb {
                return;
            }
            if thinking
                || thinking != last_thinking
                || speaking
                || reacting
                || pointer_active
                || reopened_orb
                || charging
                || was_charging
            {
                let motion = if charging {
                    level.max(0.19 + 0.12 * (time * 7.0).sin().abs())
                } else if speaking {
                    level.max(0.21 + 0.12 * (time * 9.0).sin().abs())
                } else if thinking {
                    level.max(0.13 + 0.07 * (time * 4.0).sin())
                } else {
                    level
                };
                window.set_orb(orb.frame(time, motion, thinking, pointer));
                last_frame = now;
                last_thinking = thinking;
            }
            was_charging = charging;
        }
    });

    let result = window.run();
    desktop::restore_external_windows();
    Ok(result?)
}

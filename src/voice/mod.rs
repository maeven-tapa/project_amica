mod activity;
mod live;
mod playback;

use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
};

use slint::ComponentHandle;
use tokio::sync::{mpsc, oneshot};

use crate::{AmicaWindow, AudioState, VoiceState, microphone::AudioTap};

pub fn connect(window: &AmicaWindow, tap: Arc<AudioTap>) {
    window.set_voice_model(live::MODEL_NAME.into());
    let weak = window.as_weak();
    let generation = Arc::new(AtomicU64::new(0));
    let stop = RefCell::new(None::<oneshot::Sender<()>>);
    window.on_sync_voice(move || {
        let Some(window) = weak.upgrade() else { return };
        if window.get_audio_state() != AudioState::Listening {
            if window.get_voice_active() {
                generation.fetch_add(1, Ordering::AcqRel);
                tap.unsubscribe();
                if let Some(stop) = stop.borrow_mut().take() { let _ = stop.send(()); }
                window.set_voice_state(VoiceState::Idle);
                window.set_voice_caption("".into());
                window.set_speech_level(0.0);
            }
            return;
        }
        if window.get_voice_active() || window.get_thinking() || !window.get_chat_ready() {
            return;
        }

        let id = generation.fetch_add(1, Ordering::AcqRel) + 1;
        let worker_generation = Arc::clone(&generation);
        let worker_tap = Arc::clone(&tap);
        let ready = Arc::new(AtomicBool::new(false));
        let (audio_tx, input) = mpsc::channel(24);
        let (stop_tx, stop_rx) = oneshot::channel();
        let worker_window = weak.clone();
        let speaker = window.get_output_device_id().to_string();
        let context = crate::chat::context(&window);
        let mode = if context.private { "private".into() } else { context.id.to_string() };
        let session_key = format!("{}-{}-{id}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());

        tap.subscribe(audio_tx, Arc::clone(&ready));
        window.set_voice_error("".into());
        window.set_voice_caption("".into());
        window.set_speech_level(0.0);
        window.set_voice_state(VoiceState::Connecting);

        let worker = thread::Builder::new().name("amica-live-voice".into()).spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread().enable_all().build()
                .map_err(|_| "Amica could not start its voice connection.")
                .and_then(|runtime| runtime.block_on(async {
                    let mut turn_number = 0;
                    let events = |event| {
                        let generation = Arc::clone(&worker_generation);
                        let turn_key = if matches!(&event, live::Event::Turn(_)) {
                            turn_number += 1;
                            format!("{session_key}-{turn_number}")
                        } else { String::new() };
                        let mode = mode.clone();
                        let _ = worker_window.upgrade_in_event_loop(move |window| {
                            if generation.load(Ordering::Acquire) == id {
                                match event {
                                    live::Event::Phase(phase) => window.set_voice_state(phase),
                                    live::Event::Caption(caption) => window.set_voice_caption(caption.into()),
                                    live::Event::Recovery(text) => window.set_voice_recovery(text.into()),
                                    live::Event::SpeechLevel(level) => {
                                        if window.get_memory_open() {
                                            window.set_speech_level(window.get_speech_level().max(level));
                                        }
                                    }
                                    live::Event::Navigate(action) => action.apply(&window),
                                    live::Event::Turn(turn) => window.invoke_save_voice_turn(turn.user.into(), turn.assistant.into(), turn_key.into(), mode.into()),
                                }
                            }
                        });
                    };
                    tokio::select! {
                        biased;
                        _ = stop_rx => Ok(()),
                        result = live::session(Arc::clone(&worker_tap), Arc::clone(&ready), input, &speaker, context, events) => result,
                    }
                }));
            ready.store(false, Ordering::Release);
            let _ = worker_window.upgrade_in_event_loop(move |window| {
                if worker_generation.load(Ordering::Acquire) != id { return; }
                worker_tap.unsubscribe();
                window.set_voice_caption("".into());
                window.set_speech_level(0.0);
                match result {
                    Ok(()) => window.set_voice_state(VoiceState::Idle),
                    Err(error) => show_failure(&window, error),
                }
            });
        });

        if worker.is_ok() {
            *stop.borrow_mut() = Some(stop_tx);
        } else {
            tap.unsubscribe();
            let generation = Arc::clone(&generation);
            let _ = weak.upgrade_in_event_loop(move |window| {
                if generation.load(Ordering::Acquire) == id {
                    show_failure(&window, "Amica could not start its voice thread. Unmute to retry.");
                }
            });
        }
    });
}

fn show_failure(window: &AmicaWindow, message: &str) {
    window.set_voice_error(message.into());
    window.set_voice_state(VoiceState::Failed);
    if window.get_audio_state() == AudioState::Listening {
        window.invoke_toggle_microphone();
    }
}

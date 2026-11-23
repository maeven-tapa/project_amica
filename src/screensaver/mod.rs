use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use slint::{Color, ComponentHandle, Timer, TimerMode};

use crate::{AmicaWindow, AudioState, memory::RippleBorder, settings, storage};

#[cfg(windows)]
mod windows;

#[derive(Clone, Copy)]
struct Active {
    fullscreen: bool,
    was_minimized: bool,
    was_maximized: bool,
    last_input: Option<u32>,
    entered: Instant,
}

const PALETTES: &[(&str, (u8, u8, u8))] = &[
    ("Indigo", (57, 43, 108)),
    ("Lavender", (95, 68, 127)),
    ("Midnight", (24, 46, 84)),
    ("Dawn", (99, 59, 87)),
    ("Dusk", (75, 47, 87)),
    ("Ocean", (26, 71, 107)),
    ("Sampaguita", (64, 79, 104)),
    ("Forest", (35, 80, 71)),
    ("Ember", (99, 48, 50)),
    ("Monochrome", (55, 57, 66)),
];

pub fn connect(window: &AmicaWindow) -> Timer {
    let db = storage::open()
        .ok()
        .filter(|db| settings::initialize(db).is_ok())
        .map(Rc::new);
    for key in [
        "saver_idle",
        "saver_preset",
        "saver_palette",
        "saver_dim",
        "show_clock",
        "show_date",
        "show_orb",
        "show_timer",
        "show_media",
    ] {
        if let Some(value) = db
            .as_ref()
            .and_then(|db| settings::read(db, key).ok())
            .filter(|value| !value.is_empty())
        {
            set(window, key, &value);
        }
    }
    let active = Rc::new(RefCell::new(None::<Active>));
    let idle_after = Rc::new(Cell::new(Instant::now()));
    let weak = window.as_weak();
    window.on_save_saver_setting(move |key, value| {
        let Some(window) = weak.upgrade() else { return };
        if db
            .as_ref()
            .is_some_and(|db| settings::save(db, &key, &value).is_ok())
        {
            set(&window, &key, &value);
            window.set_settings_error("".into());
        } else {
            window.set_settings_error("Amica could not save the screensaver setting.".into());
        }
    });

    let weak = window.as_weak();
    let entering = Rc::clone(&active);
    window.on_enter_saver(move |preset| {
        let Some(window) = weak.upgrade() else { return };
        if entering.borrow().is_some() || !window.get_power_request().is_empty() {
            return;
        }
        let root = window.window();
        let input = last_input().map(|(stamp, _)| stamp);
        *entering.borrow_mut() = Some(Active {
            fullscreen: root.is_fullscreen(),
            was_minimized: root.is_minimized(),
            was_maximized: root.is_maximized(),
            last_input: input,
            entered: Instant::now(),
        });
        if window.get_audio_state() == AudioState::Listening {
            window.invoke_toggle_microphone();
        }
        window.invoke_stop_desktop();
        window.invoke_dismiss_memory();
        window.set_active_saver_preset(if preset.is_empty() {
            window.get_saver_preset()
        } else {
            preset
        });
        window.set_saver_dimmed(window.get_saver_dim_on_battery() && on_battery());
        window.set_saver_active(true);
        root.set_minimized(false);
        root.set_fullscreen(true);
        #[cfg(windows)]
        windows::raise(&window);
        window.invoke_focus_saver();
    });

    let weak = window.as_weak();
    let exiting = Rc::clone(&active);
    let cooldown = Rc::clone(&idle_after);
    window.on_exit_saver(move || {
        let Some(window) = weak.upgrade() else { return };
        let Some(previous) = exiting.borrow_mut().take() else {
            return;
        };
        window.set_saver_active(false);
        window.set_saver_drift(0.0);
        window.set_saver_ripple("".into());
        window.set_saver_edge_glow(0.0);
        let root = window.window();
        root.set_fullscreen(previous.fullscreen);
        if !previous.fullscreen {
            root.set_maximized(previous.was_maximized);
        }
        root.set_minimized(previous.was_minimized);
        cooldown.set(Instant::now() + Duration::from_secs(5));
        window.invoke_focus_home();
    });

    let timer = Timer::default();
    let weak = window.as_weak();
    let mut ripple = RippleBorder::default();
    let mut static_size = None;
    timer.start(TimerMode::Repeated, Duration::from_millis(160), move || {
        let Some(window) = weak.upgrade() else { return };
        let current = last_input();
        let previous = *active.borrow();
        if let Some(previous) = previous {
            if previous.entered.elapsed() > Duration::from_millis(210)
                && current.is_some_and(|(stamp, _)| Some(stamp) != previous.last_input)
            {
                window.invoke_exit_saver();
                static_size = None;
                return;
            }
            let reduced = window.get_reduced_motion();
            let elapsed = if reduced {
                0.0
            } else {
                previous.entered.elapsed().as_secs_f32()
            };
            window.set_saver_drift((elapsed * 0.15).sin());
            if !window.get_saver_show_ripple() {
                return;
            }
            let size = (
                window.get_saver_ripple_width(),
                window.get_saver_ripple_height(),
            );
            if reduced && static_size == Some(size) && window.get_saver_edge_glow() > 0.0 {
                return;
            }
            static_size = reduced.then_some(size);
            let phase = elapsed * 0.65 % std::f32::consts::TAU;
            window.set_saver_ripple(ripple.path(size.0, size.1, phase, 1.16).into());
            window.set_saver_edge_glow(0.68 + 0.15 * (elapsed * 0.58).sin());
            return;
        }
        static_size = None;
        let timeout = window.get_saver_idle_seconds();
        if timeout > 0
            && Instant::now() >= idle_after.get()
            && current.is_some_and(|(_, idle)| idle >= (timeout as u32).saturating_mul(1000))
            && window.get_audio_state() != AudioState::Listening
            && !window.get_media_fullscreen()
            && !window.get_thinking()
            && !blocked()
        {
            window.invoke_enter_saver("".into());
        }
    });
    timer
}

fn set(window: &AmicaWindow, key: &str, value: &str) {
    match key {
        "saver_idle" => {
            let seconds = value.parse::<i32>().unwrap_or(0).clamp(0, 86_400);
            window.set_saver_idle_seconds(seconds);
            window.set_saver_idle_label(match seconds {
                0 => "Off".into(),
                30 => "30 seconds".into(),
                60 => "1 minute".into(),
                seconds => format!("{} minutes", seconds / 60).into(),
            });
        }
        "saver_preset" if matches!(value, "Ambient" | "Dashboard" | "Black Clock") => {
            window.set_saver_preset(value.into())
        }
        "saver_dim" => window.set_saver_dim_on_battery(value == "on"),
        "saver_palette" => {
            if let Some((name, (r, g, b))) = PALETTES.iter().find(|(name, _)| *name == value) {
                window.set_saver_palette((*name).into());
                window.set_saver_tint(Color::from_rgb_u8(*r, *g, *b));
            }
        }
        "show_clock" => window.set_saver_show_clock(value == "on"),
        "show_date" => window.set_saver_show_date(value == "on"),
        "show_orb" => window.set_saver_show_orb(value == "on"),
        "show_timer" => window.set_saver_show_timer(value == "on"),
        "show_media" => window.set_saver_show_media(value == "on"),
        _ => (),
    }
}

#[cfg(windows)]
use windows::{blocked, last_input, on_battery};

#[cfg(not(windows))]
fn last_input() -> Option<(u32, u32)> {
    None
}
#[cfg(not(windows))]
fn blocked() -> bool {
    true
}
#[cfg(not(windows))]
fn on_battery() -> bool {
    false
}

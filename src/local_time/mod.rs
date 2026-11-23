mod calendar;

use std::{cell::RefCell, rc::Rc, time::Duration};

use chrono::{DateTime, Local, Utc};
use serde_json::{Value, json};
use slint::{ComponentHandle, Timer, TimerMode};

use crate::AmicaWindow;

use calendar::Calendar;

pub fn connect(window: &AmicaWindow) -> Timer {
    let now = Local::now();
    let state = Rc::new(RefCell::new(Calendar::new(now.date_naive())));
    state.borrow().publish(window);
    update_clock(window, &now);

    let weak = window.as_weak();
    let calendar = Rc::clone(&state);
    window.on_change_month(move |offset| {
        let Some(window) = weak.upgrade() else { return };
        let mut calendar = calendar.borrow_mut();
        if calendar.change_month(offset) {
            calendar.publish(&window);
        }
    });

    let weak = window.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        if let Some(window) = weak.upgrade() {
            let now = Local::now();
            update_clock(&window, &now);
            let mut calendar = state.borrow_mut();
            if calendar.update_today(now.date_naive()) {
                calendar.publish(&window);
            }
        }
    });
    timer
}

fn update_clock(window: &AmicaWindow, now: &DateTime<Local>) {
    window.set_local_clock(now.format("%-I:%M:%S %p").to_string().into());
    window.set_local_date(now.format("%A, %B %-d · UTC%:z").to_string().into());
    window.set_saver_clock(now.format("%-I:%M").to_string().into());
    window.set_saver_period(now.format("%p").to_string().into());
    window.set_saver_date(now.format("%A, %B %-d").to_string().into());
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn local_at(timestamp_ms: i64) -> Option<DateTime<Local>> {
    DateTime::from_timestamp_millis(timestamp_ms).map(|time| time.with_timezone(&Local))
}

pub fn read_clock() -> Value {
    let time = Local::now();
    json!({
        "local_datetime": time.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "weekday": time.format("%A").to_string(),
        "local_time": time.format("%-I:%M %p").to_string(),
        "utc_offset": time.format("%:z").to_string(),
    })
}

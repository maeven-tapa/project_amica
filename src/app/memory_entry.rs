use std::{rc::Rc, time::Duration};

use slint::{ComponentHandle, Timer, TimerMode};

use crate::AmicaWindow;

const HOLD_TIME: Duration = Duration::from_millis(1500);
const FLASH_TIME: Duration = Duration::from_millis(150);

pub(super) fn connect(window: &AmicaWindow) {
    let hold = Rc::new(Timer::default());
    let flash = Rc::new(Timer::default());
    let weak = window.as_weak();
    let hold_timer = Rc::clone(&hold);
    let flash_timer = Rc::clone(&flash);
    window.on_hold_memory(move |holding, from_orb| {
        let Some(window) = weak.upgrade() else { return };
        if window.get_memory_open() || window.get_memory_flash() {
            return;
        }
        window.set_memory_charging(holding);
        if !holding {
            hold_timer.stop();
            return;
        }
        let target = window.as_weak();
        let flash = Rc::clone(&flash_timer);
        hold_timer.start(TimerMode::SingleShot, HOLD_TIME, move || {
            if let Some(window) = target.upgrade()
                && window.get_memory_charging()
            {
                begin_flash(&window, &flash, from_orb);
            }
        });
    });

    let weak = window.as_weak();
    window.on_activate_memory(move |from_orb| {
        let Some(window) = weak.upgrade() else { return };
        hold.stop();
        begin_flash(&window, &flash, from_orb);
    });
}

fn begin_flash(window: &AmicaWindow, timer: &Timer, from_orb: bool) {
    if window.get_memory_open() || window.get_memory_flash() {
        return;
    }
    window.set_memory_flash(true);
    let weak = window.as_weak();
    timer.start(TimerMode::SingleShot, FLASH_TIME, move || {
        if let Some(window) = weak.upgrade()
            && window.get_memory_flash()
        {
            window.invoke_present_memory(from_orb);
        }
    });
}

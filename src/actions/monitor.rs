use std::{sync::mpsc, thread, time::Duration};

use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::{AmicaWindow, TimerRow, local_time, memory, timers};

pub fn connect(window: &AmicaWindow) {
    let (commands, receiver) = mpsc::channel();
    let weak = window.as_weak();
    window.set_timer_rows(ModelRc::new(VecModel::<TimerRow>::default()));
    if thread::Builder::new()
        .name("amica-local-context".into())
        .spawn(move || run(weak, receiver))
        .is_err()
    {
        window.set_local_error("Amica could not start local context.".into());
        return;
    }
    window.on_cancel_timer(move |id| {
        if let Ok(id) = id.parse() {
            let _ = commands.send(id);
        }
    });
}

fn run(window: Weak<AmicaWindow>, commands: mpsc::Receiver<i64>) {
    let db = super::open().ok();
    let mut memory_version = None;
    let mut cancel_error = false;
    loop {
        let now = local_time::now_ms();
        let mut error = if cancel_error {
            "Amica could not cancel the timer."
        } else {
            ""
        };
        let mut running = None;
        let mut saved = None;
        let mut finished = Vec::new();
        let mut due = Vec::new();
        let mut daily = None;
        let mut media_status = None;

        if let Some(db) = &db {
            match timers::take_finished(db, now) {
                Ok(ended) => finished = ended,
                Err(_) => error = "Amica could not check for finished timers.",
            }
            if let Ok(reminders) = crate::daily::take_due(db, now) {
                due = reminders;
            }
            if let Ok(sources) = crate::media::take_due(db, now) {
                for source in sources {
                    media_status = Some(crate::media::pause(source).unwrap_or_else(|error| error));
                }
            }
            let snapshot = || -> rusqlite::Result<_> {
                let running = timers::running(db, now)?;
                let version: i64 = db.query_row("PRAGMA data_version", [], |row| row.get(0))?;
                let saved = if memory_version != Some(version) {
                    Some(memory::find(db, None, "", -1)?)
                } else {
                    None
                };
                Ok((running, saved, version))
            };
            match snapshot() {
                Ok((active, entries, version)) => {
                    if memory_version != Some(version) {
                        daily = crate::daily::list(db).ok();
                    }
                    running = Some(active);
                    saved = entries;
                    memory_version = Some(version);
                }
                Err(_) => error = "Amica could not refresh local data.",
            }
        } else {
            error = "Amica could not open local storage.";
        }

        let finished = finished.join(", ");
        let due = due.join(", ");
        if !finished.is_empty() || !due.is_empty() {
            timers::notify_finished(&finished, &due);
        }
        if window
            .upgrade_in_event_loop(move |window| {
                if window.get_local_error() != error {
                    window.set_local_error(error.into());
                }

                if let Some(running) = running {
                    refresh_timers(&window, running);
                }
                if let Some(saved) = saved {
                    memory::publish(&window, saved);
                }
                if let Some(items) = daily {
                    crate::daily::publish(&window, items);
                }
                if !due.is_empty() {
                    window.set_due_notice(due.into());
                }
                if let Some(status) = media_status {
                    window.set_command_result(status.into());
                }
                if !finished.is_empty() {
                    window.set_finished_timer(finished.into());
                }
            })
            .is_err()
        {
            break;
        }

        match commands.recv_timeout(Duration::from_secs(1)) {
            Ok(id) => {
                cancel_error = db
                    .as_ref()
                    .is_none_or(|db| timers::cancel(db, id, local_time::now_ms()).is_err())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => cancel_error = false,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn refresh_timers(window: &AmicaWindow, running: Vec<timers::RunningTimer>) {
    let rows = running
        .into_iter()
        .map(|timer| TimerRow {
            id: timer.id.to_string().into(),
            label: timer.label.into(),
            countdown: countdown(timer.remaining_seconds).into(),
        })
        .collect::<Vec<_>>();
    let current = window.get_timer_rows();
    let Some(model) = current.as_any().downcast_ref::<VecModel<TimerRow>>() else {
        return;
    };
    if model.row_count() == rows.len() && model.iter().zip(&rows).all(|(old, new)| old.id == new.id)
    {
        for (index, new) in rows.into_iter().enumerate() {
            if model
                .row_data(index)
                .is_some_and(|old| old.countdown != new.countdown || old.label != new.label)
            {
                model.set_row_data(index, new);
            }
        }
    } else {
        model.set_vec(rows);
    }
}

fn countdown(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    if hours == 0 {
        format!("{minutes:02}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
    }
}

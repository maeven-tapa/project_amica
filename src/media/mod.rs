#[cfg(windows)]
mod playback;
mod request;
#[cfg(windows)]
mod source;
#[cfg(windows)]
mod system;
#[cfg(windows)]
mod webview;

use crate::{AmicaWindow, Panel, actions::command::MediaAction};

pub use request::{PlayRequest, Service, spoken_request};

pub fn panel_for(id: &str) -> Option<Panel> {
    Some(Service::parse(id)?.panel())
}

pub fn initialize(db: &rusqlite::Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS media_pauses (
        id INTEGER PRIMARY KEY, source TEXT NOT NULL, deadline INTEGER NOT NULL
    )",
    )
}

pub fn take_due(db: &rusqlite::Connection, now: i64) -> rusqlite::Result<Vec<String>> {
    db.prepare("DELETE FROM media_pauses WHERE deadline <= ?1 RETURNING source")?
        .query_map([now], |row| row.get(0))?
        .collect()
}

pub fn pause(source: String) -> Result<String, String> {
    run(MediaAction::Play(source, false))
}

pub fn local_command(_: &AmicaWindow, action: MediaAction) -> Result<String, String> {
    if let MediaAction::PauseLater(seconds) = action {
        return schedule_pause(seconds);
    }
    run(action)
}

#[cfg(windows)]
fn schedule_pause(seconds: u32) -> Result<String, String> {
    if !(1..=86_400).contains(&seconds) {
        return Err("Choose a pause time within the next day.".into());
    }
    let source = std::thread::spawn(system::source)
        .join()
        .map_err(|_| "The media provider stopped.")??;
    let db = crate::actions::open().map_err(|_| "Amica could not open media schedules.")?;
    let deadline = crate::local_time::now_ms() + i64::from(seconds) * 1000;
    db.execute(
        "INSERT INTO media_pauses(source, deadline) VALUES (?1, ?2)",
        rusqlite::params![source, deadline],
    )
    .map_err(|_| "Amica could not schedule the pause.")?;
    Ok(format!(
        "Scheduled a pause in {} seconds for the current player.",
        seconds
    ))
}

#[cfg(not(windows))]
fn schedule_pause(_: u32) -> Result<String, String> {
    Err("Scheduled media controls are available on Windows.".into())
}

#[cfg(windows)]
fn run(action: MediaAction) -> Result<String, String> {
    std::thread::spawn(move || system::run(action))
        .join()
        .map_err(|_| "The Windows media provider stopped.")?
        .map_err(str::to_owned)
}

#[cfg(not(windows))]
fn run(_: MediaAction) -> Result<String, String> {
    Err("System media controls are available on Windows.".into())
}

pub fn connect(window: &AmicaWindow) -> slint::Timer {
    #[cfg(windows)]
    {
        webview::connect(window)
    }
    #[cfg(not(windows))]
    {
        use slint::ComponentHandle;

        window.set_media_error("Embedded media is available in Amica on Windows.".into());
        let weak = window.as_weak();
        window.on_play_media(move |_, _| {
            if let Some(window) = weak.upgrade() {
                crate::desktop::report_window_error(
                    &window,
                    Err("Embedded media playback is available on Windows."),
                );
            }
        });
        slint::Timer::default()
    }
}

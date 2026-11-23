use rusqlite::{Connection, params};
use serde::Serialize;

use crate::local_time;

pub fn initialize(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS local_timers (
            id INTEGER PRIMARY KEY,
            label TEXT NOT NULL,
            deadline_ms INTEGER NOT NULL,
            state INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS local_timers_pending ON local_timers (state, deadline_ms);",
    )
}

#[derive(Serialize)]
pub struct RunningTimer {
    pub id: i64,
    pub label: String,
    pub remaining_seconds: i64,
    finishes_at: String,
}

impl RunningTimer {
    fn new(id: i64, label: String, deadline_ms: i64, now: i64) -> Self {
        let finishes_at = local_time::local_at(deadline_ms)
            .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
            .unwrap_or_default();
        Self {
            id,
            label,
            remaining_seconds: (deadline_ms.saturating_sub(now).max(0) + 999) / 1000,
            finishes_at,
        }
    }
}

pub fn start(
    connection: &Connection,
    duration_seconds: i64,
    label: &str,
    now: i64,
) -> Result<RunningTimer, String> {
    let Some(deadline) = duration_seconds
        .checked_mul(1000)
        .and_then(|duration| now.checked_add(duration))
        .filter(|time| *time > now && local_time::local_at(*time).is_some())
    else {
        return Err("Use a positive timer duration in seconds.".into());
    };
    let id = connection
        .query_row(
            "INSERT INTO local_timers (label, deadline_ms) VALUES (?1, ?2) RETURNING id",
            params![label, deadline],
            |row| row.get(0),
        )
        .map_err(|_| "Amica could not save the timer.")?;
    Ok(RunningTimer::new(id, label.into(), deadline, now))
}

pub fn running(connection: &Connection, now: i64) -> rusqlite::Result<Vec<RunningTimer>> {
    let mut select = connection.prepare(
        "SELECT id, label, deadline_ms FROM local_timers WHERE state = 0 AND deadline_ms > ?1 ORDER BY deadline_ms, id",
    )?;
    select
        .query_map([now], |row| {
            Ok(RunningTimer::new(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                now,
            ))
        })?
        .collect()
}

pub fn cancel(connection: &Connection, id: i64, now: i64) -> rusqlite::Result<bool> {
    let count = connection.execute(
        "UPDATE local_timers SET state = 2 WHERE id = ?1 AND state = 0 AND deadline_ms > ?2",
        params![id, now],
    )?;
    Ok(count == 1)
}

pub fn take_finished(connection: &Connection, now: i64) -> rusqlite::Result<Vec<String>> {
    let mut update = connection.prepare(
        "UPDATE local_timers SET state = 1 WHERE state = 0 AND deadline_ms <= ?1 RETURNING label",
    )?;
    update.query_map([now], |row| row.get(0))?.collect()
}

pub fn notify_finished() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_count_down_cancel_and_finish_once() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let now = 1_800_000_000_000;
        let tea = start(&db, 3, "Tea", now).unwrap();
        let break_timer = start(&db, 120, "Break", now).unwrap();
        assert_eq!(tea.remaining_seconds, 3);
        assert_eq!(running(&db, now + 2101).unwrap()[0].remaining_seconds, 1);
        assert!(cancel(&db, break_timer.id, now + 3000).unwrap());
        assert!(!cancel(&db, tea.id, now + 3000).unwrap());
        assert_eq!(take_finished(&db, now + 3000).unwrap(), ["Tea"]);
        assert!(take_finished(&db, now + 130_000).unwrap().is_empty());
        assert!(running(&db, now + 130_000).unwrap().is_empty());
    }
}

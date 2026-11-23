use rusqlite::{Connection, params};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Task,
    Reminder,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Reminder => "reminder",
        }
    }
}

pub struct Item {
    pub id: i64,
    pub label: String,
    pub due: Option<i64>,
    pub completed: bool,
    pub kind: Kind,
}

pub fn initialize(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS daily_items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL CHECK(kind IN ('task', 'reminder')),
            label TEXT NOT NULL,
            due INTEGER,
            completed INTEGER NOT NULL DEFAULT 0,
            notified INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS daily_due ON daily_items(due)
            WHERE kind = 'reminder' AND notified = 0 AND completed = 0;",
    )
}

pub fn add(db: &Connection, kind: Kind, label: &str, due: Option<i64>) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO daily_items(kind, label, due) VALUES (?1, ?2, ?3)",
        params![kind.name(), label, due],
    )?;
    Ok(())
}

pub fn finish(db: &Connection, id: i64, completed: bool) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE daily_items SET completed = ?1 WHERE id = ?2",
        params![completed, id],
    )?;
    Ok(())
}

pub fn list(db: &Connection) -> rusqlite::Result<Vec<Item>> {
    let mut statement = db.prepare(
        "SELECT id, label, due, completed, kind FROM daily_items
         ORDER BY completed, coalesce(due, 9223372036854775807), id DESC",
    )?;
    statement
        .query_map([], |row| {
            Ok(Item {
                id: row.get(0)?,
                label: row.get(1)?,
                due: row.get(2)?,
                completed: row.get(3)?,
                kind: if row.get::<_, String>(4)? == "task" {
                    Kind::Task
                } else {
                    Kind::Reminder
                },
            })
        })?
        .collect()
}

pub fn take_due(db: &Connection, now: i64) -> rusqlite::Result<Vec<String>> {
    db.prepare(
        "UPDATE daily_items SET notified = 1
        WHERE kind = 'reminder' AND notified = 0 AND completed = 0 AND due <= ?1 RETURNING label",
    )?
    .query_map([now], |row| row.get(0))?
    .collect()
}

pub fn publish(window: &crate::AmicaWindow, daily_items: Vec<Item>) {
    let rows = daily_items
        .into_iter()
        .map(|item| crate::DailyRow {
            id: item.id.to_string().into(),
            label: item.label.into(),
            completed: item.completed,
            due: item
                .due
                .map(crate::memory::learned::time_label)
                .unwrap_or_default()
                .into(),
            reminder: item.kind == Kind::Reminder,
        })
        .collect::<Vec<_>>();
    window.set_daily_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

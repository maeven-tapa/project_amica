mod animation;
mod extraction;
mod graph;
pub mod learned;
mod recent;
mod ripple_border;

use rusqlite::{Connection, params};
use serde::Serialize;
use slint::{Model, ModelRc, VecModel};

use crate::{AmicaWindow, SavedRow};

pub(crate) use ripple_border::RippleBorder;

pub fn connect(window: &AmicaWindow) {
    animation::connect(window);
    recent::connect(window);
}

pub fn initialize(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS saved_items (
            kind TEXT NOT NULL,
            title TEXT NOT NULL COLLATE NOCASE,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(kind, title)
        );",
    )
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct SavedItem {
    pub kind: String,
    pub title: String,
    pub value: String,
}

pub fn publish(window: &AmicaWindow, items: Vec<SavedItem>) {
    let mut saved = Vec::with_capacity(items.len());
    for item in items {
        let is_note = item.kind == "note";
        let row = SavedRow {
            title: item.title.into(),
            value: item.value.into(),
            category: if is_note { "Note" } else { "Preference" }.into(),
        };
        saved.push(row);
    }
    window.set_saved_rows(ModelRc::new(VecModel::from(saved)));
    refresh_graph(window);
}

fn refresh_graph(window: &AmicaWindow) {
    let saved: Vec<_> = window.get_saved_rows().iter().collect();
    let learned: Vec<_> = window.get_learned_graph_rows().iter().collect();
    window.set_memory_graph(graph::layout(&saved, &learned));
}

pub fn save(
    connection: &Connection,
    kind: &str,
    title: &str,
    value: &str,
    now: i64,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO saved_items (kind, title, value, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(kind, title) DO UPDATE SET title = excluded.title, value = excluded.value, updated_at = excluded.updated_at",
        params![kind, title, value, now],
    )?;
    Ok(())
}

pub fn find(
    connection: &Connection,
    kind: Option<&str>,
    query: &str,
    limit: i64,
) -> rusqlite::Result<Vec<SavedItem>> {
    let mut selection = connection.prepare(
        "SELECT kind, title, value FROM saved_items
         WHERE (?1 IS NULL OR kind = ?1) AND (?2 = '' OR instr(lower(title || char(10) || value), lower(?2)) > 0)
         ORDER BY updated_at DESC, kind, title LIMIT ?3",
    )?;
    selection
        .query_map(params![kind, query, limit], |row| {
            Ok(SavedItem {
                kind: row.get(0)?,
                title: row.get(1)?,
                value: row.get(2)?,
            })
        })?
        .collect()
}

pub fn remove(connection: &Connection, kind: &str, title: &str) -> rusqlite::Result<bool> {
    Ok(connection.execute(
        "DELETE FROM saved_items WHERE kind = ?1 AND title = ?2",
        params![kind, title],
    )? == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_and_preferences_are_independent_and_updatable() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        save(&db, "preference", "Language", "Filipino", 1).unwrap();
        save(&db, "note", "Language", "Practice Filipino every Sunday", 2).unwrap();
        save(&db, "preference", "language", "Taglish", 3).unwrap();
        assert_eq!(
            find(&db, Some("preference"), "", 10).unwrap()[0].value,
            "Taglish"
        );
        assert_eq!(
            find(&db, Some("note"), "filipino", 10).unwrap()[0].title,
            "Language"
        );
        assert!(remove(&db, "note", "Language").unwrap());
        assert_eq!(find(&db, None, "", -1).unwrap().len(), 1);
    }
}

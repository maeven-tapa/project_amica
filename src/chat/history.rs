use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

use crate::local_time::now_ms;

#[derive(Clone, Serialize)]
pub struct Turn {
    pub user: String,
    pub assistant: String,
}

pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub updated_at: i64,
}

pub struct History(Connection);

impl History {
    pub fn open() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self::initialize(crate::storage::open()?)?)
    }

    pub fn initialize(mut connection: Connection) -> rusqlite::Result<Self> {
        connection.pragma_update(None, "foreign_keys", true)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO conversations VALUES (1, 'Conversation history', 0);
            CREATE TABLE IF NOT EXISTS chat_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_text TEXT NOT NULL,
                assistant_text TEXT NOT NULL,
                conversation_id INTEGER NOT NULL DEFAULT 1 REFERENCES conversations(id),
                channel TEXT NOT NULL DEFAULT 'text',
                created_at INTEGER NOT NULL DEFAULT 0,
                request_key TEXT UNIQUE
            );",
        )?;
        let modern: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('chat_turns') WHERE name = 'conversation_id')",
            [], |row| row.get(0),
        )?;
        if !modern {
            tx.execute_batch(
                "ALTER TABLE chat_turns RENAME TO legacy_chat_turns;
                 CREATE TABLE chat_turns (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_text TEXT NOT NULL,
                    assistant_text TEXT NOT NULL,
                    conversation_id INTEGER NOT NULL DEFAULT 1 REFERENCES conversations(id),
                    channel TEXT NOT NULL DEFAULT 'text',
                    created_at INTEGER NOT NULL DEFAULT 0,
                    request_key TEXT UNIQUE
                 );
                 INSERT INTO chat_turns (id, user_text, assistant_text)
                 SELECT id, user_text, assistant_text FROM legacy_chat_turns;
                 DROP TABLE legacy_chat_turns;",
            )?;
        }
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS chat_turns_conversation ON chat_turns(conversation_id, id)",
        )?;
        crate::memory::learned::initialize(&tx)?;
        tx.commit()?;
        Ok(Self(connection))
    }

    pub fn db(&self) -> &Connection {
        &self.0
    }

    pub fn create(&self, title: &str) -> rusqlite::Result<i64> {
        self.0.execute(
            "INSERT INTO conversations (title, updated_at) VALUES (?1, ?2)",
            params![title, now_ms()],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    pub fn rename(&self, id: i64, title: &str) -> rusqlite::Result<()> {
        self.0.execute(
            "UPDATE conversations SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    pub fn conversations(&self, query: &str) -> rusqlite::Result<Vec<Conversation>> {
        let mut statement = self.0.prepare(
            "SELECT id, title, updated_at FROM conversations c
             WHERE ?1 = '' OR instr(lower(title), lower(?1)) > 0
             OR EXISTS(SELECT 1 FROM chat_turns t WHERE t.conversation_id = c.id
                 AND instr(lower(t.user_text || char(10) || t.assistant_text), lower(?1)) > 0)
             ORDER BY updated_at DESC, id DESC",
        )?;
        statement
            .query_map([query.trim()], |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    updated_at: row.get(2)?,
                })
            })?
            .collect()
    }

    pub fn turns(&self, conversation: i64, limit: i64) -> rusqlite::Result<Vec<Turn>> {
        let mut statement = self.0.prepare(
            "SELECT user_text, assistant_text FROM
             (SELECT id, user_text, assistant_text FROM chat_turns
              WHERE conversation_id = ?1 ORDER BY id DESC LIMIT ?2) ORDER BY id",
        )?;
        statement
            .query_map(params![conversation, limit], read_turn)?
            .collect()
    }

    pub fn source(&self, id: i64) -> rusqlite::Result<Option<(i64, String)>> {
        self.0
            .query_row(
                "SELECT conversation_id, user_text FROM chat_turns WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    pub fn find(&self, query: &str) -> rusqlite::Result<Vec<Turn>> {
        let terms: Vec<_> = query
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| word.chars().count() > 2)
            .take(5)
            .collect();
        let term = |index| terms.get(index).copied().unwrap_or("");
        let mut statement = self.0.prepare(
            "WITH matches AS (
                SELECT id, user_text, assistant_text,
                    5 * (instr(lower(user_text || ' ' || assistant_text), lower(?1)) > 0)
                    + (?2 <> '' AND instr(lower(user_text || ' ' || assistant_text), lower(?2)) > 0)
                    + (?3 <> '' AND instr(lower(user_text || ' ' || assistant_text), lower(?3)) > 0)
                    + (?4 <> '' AND instr(lower(user_text || ' ' || assistant_text), lower(?4)) > 0)
                    + (?5 <> '' AND instr(lower(user_text || ' ' || assistant_text), lower(?5)) > 0)
                    + (?6 <> '' AND instr(lower(user_text || ' ' || assistant_text), lower(?6)) > 0) AS score
                FROM chat_turns
             )
             SELECT substr(user_text, 1, 1800), substr(assistant_text, 1, 1800)
             FROM matches WHERE score > 0 ORDER BY score DESC, id DESC LIMIT 6",
        )?;
        statement
            .query_map(
                params![query.trim(), term(0), term(1), term(2), term(3), term(4)],
                read_turn,
            )?
            .collect()
    }

    pub fn append(
        &self,
        conversation: i64,
        turn: &Turn,
        channel: &str,
        key: Option<&str>,
    ) -> rusqlite::Result<Option<i64>> {
        let now = now_ms();
        let tx = self.0.unchecked_transaction()?;
        let inserted = tx.execute(
            "INSERT INTO chat_turns (user_text, assistant_text, conversation_id, channel, created_at, request_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(request_key) DO NOTHING",
            params![turn.user, turn.assistant, conversation, channel, now, key],
        )?;
        if inserted == 0 {
            return Ok(None);
        }
        let id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![now, conversation],
        )?;
        tx.commit()?;
        Ok(Some(id))
    }
}

fn read_turn(row: &rusqlite::Row<'_>) -> rusqlite::Result<Turn> {
    Ok(Turn {
        user: row.get(0)?,
        assistant: row.get(1)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_turns_survive_migration_and_conversations_stay_separate() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE chat_turns (id INTEGER PRIMARY KEY, user_text TEXT NOT NULL, assistant_text TEXT NOT NULL);
             INSERT INTO chat_turns VALUES (39, 'I called the dog Miso', 'I remember Miso.');",
        ).unwrap();
        let history = History::initialize(db).unwrap();
        assert_eq!(history.source(39).unwrap().unwrap().0, 1);
        let chat = history.create("Study").unwrap();
        let turn = Turn {
            user: "Quiet study".into(),
            assistant: "Sure.".into(),
        };
        assert_eq!(
            history
                .append(chat, &turn, "voice", Some("voice-1"))
                .unwrap(),
            Some(40)
        );
        assert_eq!(
            history
                .append(chat, &turn, "voice", Some("voice-1"))
                .unwrap(),
            None
        );
        assert_eq!(history.turns(chat, 24).unwrap().len(), 1);
        assert_eq!(history.turns(1, 24).unwrap().len(), 1);
        assert_eq!(
            history.find("dog Miso").unwrap()[0].user,
            "I called the dog Miso"
        );
        assert_eq!(history.conversations("quiet study").unwrap()[0].id, chat);
    }
}

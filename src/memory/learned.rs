use chrono::Local;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{local_time::now_ms, settings};

use super::extraction::{extract, study_scope, tokens};

pub struct Node {
    pub id: i64,
    pub category: String,
    pub scope: String,
    pub claim: String,
    pub pinned: bool,
    pub state: String,
    pub until: Option<String>,
    pub updated: i64,
    pub sources: Vec<Source>,
    pub related: Vec<String>,
    pub related_ids: Vec<i64>,
}

pub struct Source {
    pub turn: i64,
    pub excerpt: String,
    pub channel: String,
    pub claim: String,
}

pub fn initialize(db: &Connection) -> rusqlite::Result<()> {
    settings::initialize(db)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS learned_nodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            concept TEXT NOT NULL UNIQUE,
            category TEXT NOT NULL,
            scope TEXT NOT NULL,
            edited_claim TEXT,
            pinned INTEGER NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS learned_turns (turn_id INTEGER PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS learned_evidence (
            node_id INTEGER NOT NULL REFERENCES learned_nodes(id) ON DELETE CASCADE,
            turn_id INTEGER NOT NULL REFERENCES chat_turns(id) ON DELETE CASCADE,
            excerpt TEXT NOT NULL,
            claim TEXT NOT NULL,
            state TEXT NOT NULL,
            meaning TEXT NOT NULL DEFAULT '',
            due TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY(node_id, turn_id)
         );
         CREATE TABLE IF NOT EXISTS learned_links (
            from_id INTEGER NOT NULL REFERENCES learned_nodes(id) ON DELETE CASCADE,
            to_id INTEGER NOT NULL REFERENCES learned_nodes(id) ON DELETE CASCADE,
            turn_id INTEGER NOT NULL REFERENCES chat_turns(id) ON DELETE CASCADE,
            relation TEXT NOT NULL,
            PRIMARY KEY(from_id, to_id, turn_id, relation)
         );
         CREATE INDEX IF NOT EXISTS learned_evidence_turn ON learned_evidence(turn_id);
         CREATE INDEX IF NOT EXISTS learned_links_turn ON learned_links(turn_id);
         CREATE TABLE IF NOT EXISTS learned_user_links (
            from_id INTEGER NOT NULL REFERENCES learned_nodes(id) ON DELETE CASCADE,
            to_id INTEGER NOT NULL REFERENCES learned_nodes(id) ON DELETE CASCADE,
            relation TEXT NOT NULL,
            PRIMARY KEY(from_id, to_id, relation)
         );
         CREATE TRIGGER IF NOT EXISTS prune_learned_nodes AFTER DELETE ON chat_turns BEGIN
             DELETE FROM learned_nodes WHERE NOT EXISTS
                 (SELECT 1 FROM learned_evidence WHERE node_id = learned_nodes.id);
         END;",
    )?;
    let has_meaning: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('learned_evidence') WHERE name = 'meaning')",
        [],
        |row| row.get(0),
    )?;
    if !has_meaning {
        db.execute_batch(
            "ALTER TABLE learned_evidence ADD COLUMN meaning TEXT NOT NULL DEFAULT ''",
        )?;
    }
    Ok(())
}

pub fn capture_epoch(db: &Connection) -> Option<i64> {
    let value = settings::read(db, "learned_capture").ok()?;
    let (epoch, state) = value.split_once(':').unwrap_or(("1", "on"));
    (state == "on").then(|| epoch.parse().ok()).flatten()
}

pub fn configure(db: &Connection, enabled: bool, clear: bool) -> rusqlite::Result<()> {
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    let value = settings::read(&tx, "learned_capture")?;
    let epoch = value
        .split_once(':')
        .and_then(|(epoch, _)| epoch.parse::<i64>().ok())
        .unwrap_or(1);
    let state = if enabled { "on" } else { "off" };
    settings::save(&tx, "learned_capture", &format!("{}:{state}", epoch + 1))?;
    if clear {
        tx.execute("DELETE FROM learned_nodes", [])?;
    }
    tx.commit()
}

pub fn process(db: &Connection, turn_id: i64, expected_epoch: i64) -> rusqlite::Result<bool> {
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    let Some((user, now)) = tx
        .query_row(
            "SELECT user_text, created_at FROM chat_turns WHERE id = ?1 AND created_at > 0",
            [turn_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
    else {
        return Ok(false);
    };
    if capture_epoch(&tx) != Some(expected_epoch)
        || tx.execute("INSERT OR IGNORE INTO learned_turns VALUES (?1)", [turn_id])? == 0
    {
        return Ok(false);
    }

    let mut stored = Vec::new();
    for mut candidate in extract(&user, now) {
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM learned_nodes WHERE concept = ?1",
                [&candidate.key],
                |row| row.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => id,
            None if candidate.state == "completed" => continue,
            None => {
                tx.execute(
                    "INSERT INTO learned_nodes (concept, category, scope) VALUES (?1, ?2, ?3)",
                    params![candidate.key, candidate.category, candidate.scope],
                )?;
                tx.last_insert_rowid()
            }
        };
        let previous: Option<(String, String, Option<String>)> = tx.query_row(
            "SELECT meaning, state, due FROM learned_evidence WHERE node_id = ?1 AND turn_id <> ?2 ORDER BY turn_id DESC LIMIT 1",
            params![id, turn_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        if let Some((meaning, state, due)) = previous {
            if candidate.category == "Goal" && state == "active" && candidate.until.is_none() {
                candidate.until = due;
            }
            if !candidate.meaning.is_empty() && !meaning.is_empty() {
                if state == "tentative" && meaning == candidate.meaning {
                    candidate.state = "active";
                } else if meaning != candidate.meaning && !candidate.correction {
                    candidate.state = "tentative";
                }
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO learned_evidence (node_id, turn_id, excerpt, claim, state, due, updated_at, meaning)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, turn_id, candidate.excerpt, candidate.claim, candidate.state, candidate.until, now, candidate.meaning],
        )?;
        stored.push((id, candidate));
    }
    for (from, preference) in &stored {
        for (to, goal) in &stored {
            if preference.category == "Preference"
                && goal.category == "Goal"
                && preference.scope == goal.scope
                && preference.state == "active"
                && goal.state == "active"
            {
                tx.execute(
                    "INSERT OR IGNORE INTO learned_links VALUES (?1, ?2, ?3, 'supports')",
                    params![from, to, turn_id],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(!stored.is_empty())
}

pub fn list(db: &Connection, query: &str) -> rusqlite::Result<Vec<Node>> {
    let mut statement = db.prepare(
        "SELECT n.id, n.category, n.scope, coalesce(n.edited_claim, e.claim), n.pinned,
                e.state, e.due, e.updated_at
         FROM learned_nodes n JOIN learned_evidence e ON e.node_id = n.id
         AND e.turn_id = (SELECT turn_id FROM learned_evidence WHERE node_id = n.id
            ORDER BY (state <> 'tentative') DESC, turn_id DESC LIMIT 1)
         WHERE ?1 = '' OR instr(lower(n.category || ' ' || n.scope || ' ' || coalesce(n.edited_claim, e.claim)), lower(?1)) > 0
         ORDER BY n.pinned DESC, e.updated_at DESC, n.id DESC",
    )?;
    let mut nodes = statement
        .query_map([query.trim()], |row| {
            Ok(Node {
                id: row.get(0)?,
                category: row.get(1)?,
                scope: row.get(2)?,
                claim: row.get(3)?,
                pinned: row.get(4)?,
                state: row.get(5)?,
                until: row.get(6)?,
                updated: row.get(7)?,
                sources: Vec::new(),
                related: Vec::new(),
                related_ids: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut source = db.prepare(
        "SELECT e.turn_id, e.excerpt, t.channel, e.claim
         FROM learned_evidence e JOIN chat_turns t ON t.id = e.turn_id
         WHERE e.node_id = ?1 ORDER BY e.turn_id DESC",
    )?;
    let mut links = db.prepare(
        "WITH edges AS (
            SELECT from_id, to_id, relation FROM learned_links
            UNION SELECT from_id, to_id, relation FROM learned_user_links
         )
         SELECT DISTINCT l.relation, coalesce(n.edited_claim, e.claim), l.from_id = ?1, n.id
         FROM edges l JOIN learned_nodes n ON n.id = CASE WHEN l.from_id = ?1 THEN l.to_id ELSE l.from_id END
         JOIN learned_evidence e ON e.node_id = n.id AND e.turn_id =
             (SELECT turn_id FROM learned_evidence WHERE node_id = n.id
                ORDER BY (state <> 'tentative') DESC, turn_id DESC LIMIT 1)
         WHERE l.from_id = ?1 OR l.to_id = ?1",
    )?;
    for node in &mut nodes {
        node.sources = source
            .query_map([node.id], |row| {
                Ok(Source {
                    turn: row.get(0)?,
                    excerpt: row.get(1)?,
                    channel: row.get(2)?,
                    claim: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let related = links
            .query_map([node.id], |row| {
                let relation: String = row.get(0)?;
                let claim: String = row.get(1)?;
                let outgoing: bool = row.get(2)?;
                let id: i64 = row.get(3)?;
                Ok((
                    id,
                    if outgoing {
                        format!("{relation} · {claim}")
                    } else {
                        format!("{claim} · {relation} this memory")
                    },
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        (node.related_ids, node.related) = related.into_iter().unzip();
    }
    Ok(nodes)
}

pub fn relationship(
    db: &Connection,
    from: i64,
    to: i64,
    relation: &str,
    remove: bool,
) -> rusqlite::Result<()> {
    if from == to || !matches!(relation, "supports" | "belongs to" | "supersedes") {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
    if remove {
        for table in ["learned_links", "learned_user_links"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE from_id = ?1 AND to_id = ?2 AND relation = ?3"),
                params![from, to, relation],
            )?;
        }
    } else {
        tx.execute(
            "INSERT OR IGNORE INTO learned_user_links VALUES (?1, ?2, ?3)",
            params![from, to, relation],
        )?;
    }
    tx.commit()
}

pub fn recall(db: &Connection, prompt: &str) -> rusqlite::Result<Vec<String>> {
    let lower = prompt.to_lowercase();
    let terms = tokens(&lower);
    let now = Local::now().date_naive();
    let today = now.to_string();
    let nodes = list(db, "")?;
    let later = now
        .checked_add_days(chrono::Days::new(7))
        .unwrap_or(now)
        .to_string();
    let upcoming = nodes.iter().any(|node| {
        node.category == "Goal"
            && node.scope == "studying"
            && node.state == "active"
            && node
                .until
                .as_ref()
                .is_some_and(|date| date >= &today && date <= &later)
    });
    let scope = study_scope(&lower).or_else(|| {
        (upcoming
            && [
                "after dinner",
                "pagkatapos ng hapunan",
                "pagkatapos ng dinner",
            ]
            .iter()
            .any(|phrase| lower.contains(phrase)))
        .then_some("studying")
    });
    let mut relevant = Vec::new();
    for node in nodes {
        if node.state != "active" || node.until.as_ref().is_some_and(|date| date < &today) {
            continue;
        }
        let claim = node.claim.to_lowercase();
        let words = tokens(&claim);
        let overlap = terms
            .iter()
            .filter(|term| {
                term.len() >= 4
                    && ![
                        "please", "help", "like", "prefer", "want", "gusto", "with", "that",
                        "this", "what", "when",
                    ]
                    .contains(term)
                    && words.contains(term)
            })
            .count();
        let scope_score = match scope {
            Some(scope) if scope == node.scope => 5,
            Some("reading" | "memorizing") if node.scope == "studying" => 2,
            _ => 0,
        };
        if overlap + scope_score > 0 {
            let score = (
                overlap + scope_score,
                node.pinned,
                node.sources.len(),
                node.updated,
            );
            let description = match node.until {
                Some(date) => format!("{} ({}; due {})", node.claim, node.scope, date),
                None => format!("{} ({})", node.claim, node.scope),
            };
            relevant.push((score, description));
        }
    }
    relevant.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(relevant
        .into_iter()
        .take(4)
        .map(|(_, description)| description)
        .collect())
}

pub fn edit(db: &Connection, id: i64, claim: &str) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE learned_nodes SET edited_claim = ?1 WHERE id = ?2",
        params![claim, id],
    )?;
    Ok(())
}

pub fn pin(db: &Connection, id: i64) -> rusqlite::Result<()> {
    db.execute(
        "UPDATE learned_nodes SET pinned = NOT pinned WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

pub fn forget(db: &Connection, id: i64, delete_sources: bool) -> rusqlite::Result<()> {
    let tx = db.unchecked_transaction()?;
    if delete_sources {
        tx.execute("DELETE FROM chat_turns WHERE id IN (SELECT turn_id FROM learned_evidence WHERE node_id = ?1)", [id])?;
    }
    tx.execute("DELETE FROM learned_nodes WHERE id = ?1", [id])?;
    tx.commit()
}

pub fn time_label(time: i64) -> String {
    chrono::DateTime::from_timestamp_millis(time)
        .filter(|_| time > 0)
        .map(|date| {
            date.with_timezone(&Local)
                .format("%d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

pub fn is_recent(node: &Node) -> bool {
    now_ms().saturating_sub(node.updated) < 5_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{History, Turn};

    fn add(history: &History, chat: i64, user: &str, channel: &str) -> i64 {
        history
            .append(
                chat,
                &Turn {
                    user: user.into(),
                    assistant: "Got it.".into(),
                },
                channel,
                None,
            )
            .unwrap()
            .unwrap()
    }

    #[test]
    fn spoken_and_typed_memories_reinforce_recall_and_forget() {
        let profile = std::env::temp_dir().join(format!(
            "amica-memory-{}-{}.sqlite3",
            std::process::id(),
            now_ms()
        ));
        let history = History::initialize(Connection::open(&profile).unwrap()).unwrap();
        let db = history.db();
        let epoch = capture_epoch(db).unwrap();
        let turn = add(
            &history,
            1,
            "May Rust quiz ako sa Friday. Mas gusto kong tahimik kapag nag-aaral ako.",
            "voice",
        );
        assert!(process(db, turn, epoch).unwrap());
        assert!(!process(db, turn, epoch).unwrap());
        let chat = history.create("Study").unwrap();
        let typed = add(&history, chat, "I prefer quiet when studying.", "text");
        assert!(process(db, typed, epoch).unwrap());
        let nodes = list(db, "").unwrap();
        let preference = nodes
            .iter()
            .find(|node| node.category == "Preference")
            .unwrap();
        assert_eq!(preference.sources.len(), 2);
        assert_eq!(preference.sources[1].channel, "voice");
        assert!(preference.related[0].contains("Rust quiz"));
        assert!(
            recall(db, "Help me make a study plan")
                .unwrap()
                .iter()
                .any(|claim| claim.contains("quiet"))
        );
        assert!(recall(db, "What is the weather?").unwrap().is_empty());
        edit(
            db,
            preference.id,
            "I prefer instrumental music for studying.",
        )
        .unwrap();
        pin(db, preference.id).unwrap();
        assert!(recall(db, "Study plan").unwrap()[0].contains("instrumental"));
        let completed = add(&history, chat, "I finished the Rust quiz.", "text");
        process(db, completed, epoch).unwrap();
        assert_eq!(recall(db, "Study plan").unwrap().len(), 1);
        forget(db, preference.id, false).unwrap();
        drop(history);
        let reopened = History::initialize(Connection::open(&profile).unwrap()).unwrap();
        assert!(!process(reopened.db(), turn, epoch).unwrap());
        assert!(recall(reopened.db(), "Study plan").unwrap().is_empty());
        drop(reopened);
        std::fs::remove_file(profile).unwrap();
    }

    #[test]
    fn capture_respects_epoch_source_deletion_and_uncertain_text() {
        let history = History::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let db = history.db();
        let epoch = capture_epoch(db).unwrap();
        let unsafe_texts = [
            "She said “I prefer quiet when studying.”",
            "I prefer quiet when studying, just kidding.",
            "I prefer using password 343434 when studying.",
            "I prefer quiet when studying?",
        ];
        for line in unsafe_texts {
            let id = add(&history, 1, line, "voice");
            assert!(!process(db, id, epoch).unwrap());
        }
        let pending = add(&history, 1, "I prefer quiet when studying.", "voice");
        configure(db, false, false).unwrap();
        configure(db, true, false).unwrap();
        assert!(!process(db, pending, epoch).unwrap());
        let valid = add(&history, 1, "I prefer quiet when studying.", "text");
        assert!(process(db, valid, capture_epoch(db).unwrap()).unwrap());
        let id = list(db, "").unwrap()[0].id;
        forget(db, id, true).unwrap();
        assert!(history.source(valid).unwrap().is_none());
        assert!(list(db, "").unwrap().is_empty());
    }

    #[test]
    fn scopes_weak_claims_corrections_and_expired_goals() {
        let history = History::initialize(Connection::open_in_memory().unwrap()).unwrap();
        let db = history.db();
        let epoch = capture_epoch(db).unwrap();
        let learn = |text| process(db, add(&history, 1, text, "text"), epoch).unwrap();
        assert!(learn("I think I prefer quiet when studying."));
        assert!(recall(db, "Make a study plan.").unwrap().is_empty());
        assert!(learn("Mas gusto kong tahimik kapag nag-aaral ako."));
        assert!(learn("I like music when studying."));
        assert!(recall(db, "Make a study plan.").unwrap()[0].contains("tahimik"));
        assert!(learn("Actually, I prefer music when studying."));
        assert!(learn("I prefer quiet when reading."));
        assert!(recall(db, "Help with studying.").unwrap()[0].contains("music"));
        assert!(recall(db, "Help with reading.").unwrap()[0].contains("quiet"));
        assert!(learn("I have a Rust quiz on 2000-01-01."));
        assert!(
            recall(db, "Rust quiz")
                .unwrap()
                .iter()
                .all(|claim| !claim.contains("Prepare for"))
        );
    }
}

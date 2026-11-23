use std::{cell::RefCell, rc::Rc, time::Duration};

use slint::{ComponentHandle, Model, ModelRc, Timer, VecModel};

use crate::{AmicaWindow, LearnedRow, LearnedSource, chat::History};

use super::{learned, refresh_graph};

pub fn connect(window: &AmicaWindow) {
    let Ok(history) = History::open() else {
        window.set_memory_error("Amica could not open learned memories.".into());
        return;
    };
    let history = Rc::new(history);
    let query = Rc::new(RefCell::new(String::new()));
    window.set_memory_capture(learned::capture_epoch(history.db()).is_some());
    window.set_memory_intro(
        crate::settings::read(history.db(), "learned_explained")
            .is_ok_and(|value| value.is_empty()),
    );
    let weak = window.as_weak();
    let db = Rc::clone(&history);
    window.on_dismiss_memory_intro(move || {
        if let Some(window) = weak.upgrade()
            && crate::settings::save(db.db(), "learned_explained", "yes").is_ok()
        {
            window.set_memory_intro(false);
        }
    });

    let weak = window.as_weak();
    let db = Rc::clone(&history);
    let current_query = Rc::clone(&query);
    let highlight = Timer::default();
    window.on_refresh_memories(move || {
        let Some(window) = weak.upgrade() else { return };
        if refresh(&window, &db, &current_query.borrow()) {
            let weak = window.as_weak();
            let db = Rc::clone(&db);
            let current_query = Rc::clone(&current_query);
            highlight.start(
                slint::TimerMode::SingleShot,
                Duration::from_secs(5),
                move || {
                    if let Some(window) = weak.upgrade() {
                        refresh(&window, &db, &current_query.borrow());
                    }
                },
            );
        }
    });

    let weak = window.as_weak();
    window.on_search_memories(move |value| {
        if let Some(window) = weak.upgrade() {
            *query.borrow_mut() = value.into();
            window.invoke_refresh_memories();
        }
    });

    let weak = window.as_weak();
    window.on_select_memory(move |id| {
        if let Some(window) = weak.upgrade()
            && let Some(row) = window.get_learned_rows().iter().find(|row| row.id == id)
        {
            window.set_learned_detail(row);
        }
    });

    let weak = window.as_weak();
    let db = Rc::clone(&history);
    window.on_change_memory(move |action, id, value| {
        let Some(window) = weak.upgrade() else { return };
        let Ok(id) = id.parse() else { return };
        let result = match action.as_str() {
            "edit" if !value.trim().is_empty() && value.chars().count() <= 500 => {
                learned::edit(db.db(), id, value.trim())
            }
            "pin" => learned::pin(db.db(), id),
            "link" | "unlink" => {
                let Some((target, relation)) = value.split_once(':') else {
                    return;
                };
                let Ok(target) = target.parse() else { return };
                learned::relationship(db.db(), id, target, relation, action == "unlink")
            }
            "forget" | "delete" => {
                if action == "delete" && (window.get_thinking() || window.get_voice_active()) {
                    window.set_memory_error(
                        "Finish the conversation turn before deleting source messages.".into(),
                    );
                    return;
                }
                learned::forget(db.db(), id, action == "delete")
            }
            _ => {
                window.set_memory_error("Enter a memory under 500 characters.".into());
                return;
            }
        };
        if result.is_ok() {
            window.invoke_refresh_memories();
            if action == "delete" {
                window.invoke_reload_conversation();
            }
        } else {
            window.set_memory_error("Amica could not update this memory.".into());
        }
    });

    let weak = window.as_weak();
    window.on_configure_memory(move |enabled, clear| {
        let Some(window) = weak.upgrade() else { return };
        if learned::configure(history.db(), enabled, clear).is_ok() {
            window.set_memory_capture(enabled);
            window.invoke_refresh_memories();
        } else {
            window.set_memory_error("Amica could not update learned memory settings.".into());
        }
    });
    window.invoke_refresh_memories();
}

fn refresh(window: &AmicaWindow, history: &History, query: &str) -> bool {
    let Ok(nodes) = learned::list(history.db(), "") else {
        window.set_memory_error("Amica could not refresh learned memories.".into());
        return false;
    };
    let rows = nodes.into_iter().map(row).collect::<Vec<_>>();
    let search = query.trim().to_lowercase();
    let visible = rows
        .iter()
        .filter(|row| {
            search.is_empty()
                || format!("{} {} {}", row.category, row.scope, row.claim)
                    .to_lowercase()
                    .contains(&search)
        })
        .cloned()
        .collect::<Vec<_>>();
    let current = window.get_learned_detail().id;
    window.set_learned_detail(
        rows.iter()
            .find(|row| row.id == current)
            .cloned()
            .unwrap_or_default(),
    );
    let fresh = rows.iter().any(|row| row.fresh);
    window.set_learned_graph_rows(ModelRc::new(VecModel::from(rows)));
    window.set_learned_rows(ModelRc::new(VecModel::from(visible)));
    window.set_memory_error("".into());
    refresh_graph(window);
    fresh
}

fn row(node: learned::Node) -> LearnedRow {
    let fresh = learned::is_recent(&node);
    let mut status = match node.state.as_str() {
        "completed" => " · Completed".to_owned(),
        "tentative" => " · Tentative".to_owned(),
        _ => String::new(),
    };
    if let Some(date) = node.until {
        status.push_str(&format!(" · Due {date}"));
    }
    let sources = node
        .sources
        .into_iter()
        .map(|source| LearnedSource {
            id: source.turn.to_string().into(),
            label: format!(
                "{} · {}",
                if source.channel == "voice" {
                    "Spoken"
                } else {
                    "Typed"
                },
                source.claim
            )
            .into(),
            excerpt: source.excerpt.into(),
        })
        .collect::<Vec<_>>();
    LearnedRow {
        id: node.id.to_string().into(),
        category: node.category.into(),
        scope: node.scope.into(),
        claim: node.claim.into(),
        pinned: node.pinned,
        fresh,
        updated: learned::time_label(node.updated).into(),
        status: status.into(),
        related: node.related.join("\n").into(),
        sources: ModelRc::new(VecModel::from(sources)),
        related_ids: ModelRc::new(VecModel::from(
            node.related_ids
                .into_iter()
                .map(|id| id.to_string().into())
                .collect::<Vec<_>>(),
        )),
    }
}

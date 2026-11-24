use std::{cell::RefCell, rc::Rc, time::Duration};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, ModelRc, Timer, VecModel};

use crate::{
    AmicaWindow, Panel, RoutineRow,
    actions::{catalog, command::RoutineAction},
    desktop,
    local_time::now_ms,
};

const MAX_STEPS: usize = 16;
const DB_ERROR: &str = "Amica could not access saved routines.";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Step {
    App(String),
    Folder(String),
    Timer(u32, String),
    Wait(u32),
    Pause,
    SaveLayout(String),
    ShowTasks,
    Layout(Window),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Window {
    pub app: String,
    pub bounds: [i32; 4],
}

pub fn initialize(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS routines (
            name TEXT PRIMARY KEY COLLATE NOCASE,
            steps TEXT NOT NULL,
            shortcut TEXT UNIQUE,
            last_run TEXT
         );",
    )?;
    for (name, steps) in [
        (
            "Focus",
            vec![
                Step::Timer(1500, "Focus".into()),
                Step::App("notepad".into()),
            ],
        ),
        (
            "Study",
            vec![
                Step::Folder("Documents".into()),
                Step::App("notepad".into()),
                Step::App("calculator".into()),
                Step::Timer(1800, "Reading".into()),
            ],
        ),
        (
            "End of Day",
            vec![Step::SaveLayout("Last workspace".into()), Step::ShowTasks],
        ),
    ] {
        let _ = db.execute(
            "INSERT OR IGNORE INTO routines(name, steps) VALUES (?1, ?2)",
            params![name, encode(&steps)],
        );
    }
    Ok(())
}

fn db() -> Result<Connection, &'static str> {
    crate::storage::open().map_err(|_| DB_ERROR)
}

fn encode(steps: &[Step]) -> String {
    serde_json::to_string(steps).unwrap_or_default()
}

fn read(db: &Connection, name: &str) -> Result<Vec<Step>, String> {
    let json: Option<String> = db
        .query_row(
            "SELECT steps FROM routines WHERE name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| DB_ERROR)?;
    serde_json::from_str(&json.ok_or_else(|| format!("No routine or layout named “{name}”."))?)
        .map_err(|_| "Amica could not read the routine steps.".into())
}

fn save(db: &Connection, name: &str, steps: &[Step]) -> Result<(), String> {
    if name.trim().is_empty()
        || name.chars().count() > 80
        || steps.is_empty()
        || steps.len() > MAX_STEPS
    {
        return Err("Use a name of up to 80 characters and one to 16 steps.".into());
    }
    db.execute(
        "INSERT INTO routines(name, steps) VALUES (?1, ?2)
        ON CONFLICT(name) DO UPDATE SET steps = excluded.steps",
        params![name, encode(steps)],
    )
    .map_err(|_| DB_ERROR)?;
    Ok(())
}

fn script(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|step| match step {
            Step::App(name) => format!("Open {}", name.replace('_', " ")),
            Step::Folder(name) => format!("Folder {name}"),
            Step::Timer(seconds, label) => format!("Timer {seconds} {label}"),
            Step::Wait(seconds) => format!("Wait {seconds}"),
            Step::Pause => "Pause media".into(),
            Step::SaveLayout(name) => format!("Save layout {name}"),
            Step::ShowTasks => "Show tasks".into(),
            Step::Layout(window) => format!(
                "Place {} {} {} {} {}",
                window.app.replace('_', " "),
                window.bounds[0],
                window.bounds[1],
                window.bounds[2],
                window.bounds[3]
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_script(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let invalid = "Use Open, Folder, Timer, Wait, Pause media, Show tasks, or Save layout.";
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (verb, args) = line.split_once(' ').ok_or(invalid)?;
        let step = match verb.to_lowercase().as_str() {
            "open" => Step::App(
                catalog::app(args)
                    .ok_or("Name a supported app, such as Notepad or Calculator.")?
                    .into(),
            ),
            "folder" => Step::Folder(args.into()),
            "timer" => {
                let (seconds, label) = args
                    .split_once(' ')
                    .ok_or("Use Timer followed by seconds and a label.")?;
                Step::Timer(bounded(seconds, 86_400)?, label.into())
            }
            "wait" => Step::Wait(bounded(args, 120)?),
            "pause" if args.eq_ignore_ascii_case("media") => Step::Pause,
            "show" if args.eq_ignore_ascii_case("tasks") => Step::ShowTasks,
            "save" if args.to_lowercase().starts_with("layout ") => {
                Step::SaveLayout(args[7..].into())
            }
            _ => return Err(invalid.into()),
        };
        steps.push(step);
    }
    Ok(steps)
}

fn bounded(text: &str, max: u32) -> Result<u32, String> {
    text.parse()
        .ok()
        .filter(|value| (1..=max).contains(value))
        .ok_or_else(|| format!("Use a duration from 1 to {max} seconds."))
}

pub fn connect(window: &AmicaWindow) {
    let weak = window.as_weak();
    window.on_refresh_routines(move || {
        if let Some(window) = weak.upgrade() {
            refresh(&window);
        }
    });

    let weak = window.as_weak();
    window.on_routine_edit(move |action, name, value| {
        let Some(window) = weak.upgrade() else { return };
        let result = match action.as_str() {
            "select" => command(&window, RoutineAction::Preview(name.into())),
            "run" => command(&window, RoutineAction::Run(name.into())),
            "last" => command(&window, RoutineAction::LastRun(name.into())),
            "save" => consider_save(&window, name.into(), value.as_str()),
            "shortcut" => {
                let outcome = db().and_then(|db| {
                    if !matches!(
                        value.as_str(),
                        "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "None"
                    ) {
                        return Err("Choose an available function key.");
                    }
                    let shortcut = (value != "None").then_some(value.as_str());
                    db.execute(
                        "UPDATE routines SET shortcut = ?1 WHERE name = ?2",
                        params![shortcut, name.as_str()],
                    )
                    .map_err(|_| "That shortcut is already assigned to a routine.")?;
                    Ok("Saved the routine shortcut.".into())
                });
                outcome.map_err(str::to_owned)
            }
            _ => Err("Choose a saved routine.".into()),
        };
        set_result(&window, result);
        refresh(&window);
    });

    let weak = window.as_weak();
    window.on_routine_shortcut(move |key| {
        let Some(window) = weak.upgrade() else { return };
        if window.get_saver_active() || !window.get_power_request().is_empty() {
            return;
        }
        if let Ok(db) = db()
            && let Ok(name) = db.query_row(
                "SELECT name FROM routines WHERE shortcut = ?1",
                [key.as_str()],
                |row| row.get::<_, String>(0),
            )
        {
            set_result(&window, command(&window, RoutineAction::Run(name)));
        }
    });

    let weak = window.as_weak();
    window.on_stop_routine(move || {
        if let Some(window) = weak.upgrade()
            && window.get_routine_running()
        {
            window.set_routine_running(false);
            window.set_routine_run_id("".into());
            window.set_routine_result("Stopped. Completed steps remain in the run history.".into());
        }
    });
}

fn consider_save(window: &AmicaWindow, name: String, text: &str) -> Result<String, String> {
    let steps = parse_script(text)?;
    let db = db()?;
    if read(&db, &name).is_ok() {
        desktop::confirm(
            window,
            format!("Replace the steps of {name}?\n{}", script(&steps)),
            move |window| {
                let result = crate::routines::db()
                    .map_err(str::to_owned)
                    .and_then(|db| save(&db, &name, &steps))
                    .map(|()| format!("Saved {name}."));
                set_result(window, result);
                refresh(window);
            },
        );
        Ok("Review the routine changes.".into())
    } else {
        save(&db, &name, &steps)?;
        Ok(format!("Saved {name}."))
    }
}

pub fn command(window: &AmicaWindow, action: RoutineAction) -> Result<String, String> {
    let db = db()?;
    window.invoke_show_panel(Panel::Routines);
    refresh(window);
    match action {
        RoutineAction::Preview(name) | RoutineAction::Shortcut(name) => {
            let steps = read(&db, &name)?;
            window.set_routine_name(name.into());
            window.set_routine_script(script(&steps).into());
            Ok("Showing the routine steps and shortcut controls.".into())
        }
        RoutineAction::Run(name) | RoutineAction::RestoreLayout(name) => {
            if window.get_routine_running() {
                return Err("Stop the current routine first.".into());
            }
            let steps = read(&db, &name)?;
            let preview = script(&steps);
            window.set_routine_name(name.as_str().into());
            window.set_routine_script(preview.as_str().into());
            desktop::confirm(window, format!("Run {name}?\n{preview}"), move |window| {
                if !window.get_routine_running() {
                    run(window, name, steps);
                }
            });
            Ok("Review the ordered routine steps in Amica.".into())
        }
        RoutineAction::SaveLayout(name) => {
            #[cfg(windows)]
            let steps: Vec<_> = desktop::layout::snapshot()
                .into_iter()
                .map(Step::Layout)
                .collect();
            #[cfg(not(windows))]
            let steps: Vec<Step> = Vec::new();
            if steps.is_empty() {
                return Err(if cfg!(windows) {
                    "No supported app windows are open to save."
                } else {
                    "Saving window layouts is unavailable on this platform."
                }
                .into());
            }
            desktop::confirm(
                window,
                format!("Save layout {name}?\n{}", script(&steps)),
                move |window| {
                    set_result(
                        window,
                        crate::routines::db()
                            .map_err(str::to_owned)
                            .and_then(|db| save(&db, &name, &steps))
                            .map(|()| format!("Saved {name}.")),
                    );
                    refresh(window);
                },
            );
            Ok("Review the windows and positions in Amica.".into())
        }
        RoutineAction::AddApp(name, app) => {
            let mut steps = read(&db, &name)?;
            steps.push(Step::App(
                catalog::app(&app)
                    .ok_or("That app has no local launcher.")?
                    .into(),
            ));
            consider_save(window, name, &script(&steps))
        }
        RoutineAction::Wait(name, seconds) => {
            let mut steps = read(&db, &name)?;
            steps.push(Step::Wait(bounded(&seconds.to_string(), 120)?));
            consider_save(window, name, &script(&steps))
        }
        RoutineAction::Remove(name, index) => {
            let mut steps = read(&db, &name)?;
            let index = usize::from(index.checked_sub(1).ok_or("Step numbers start at 1.")?);
            if index >= steps.len() {
                return Err("No step has that number.".into());
            }
            steps.remove(index);
            consider_save(window, name, &script(&steps))
        }
        RoutineAction::Duplicate(name) => {
            let steps = read(&db, &name)?;
            window.set_routine_name(format!("{name} copy").into());
            window.set_routine_script(script(&steps).into());
            Ok("Review and save the editable copy.".into())
        }
        RoutineAction::LastRun(name) => db
            .query_row(
                "SELECT last_run FROM routines WHERE name = ?1",
                [&name],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|_| DB_ERROR)?
            .flatten()
            .ok_or("This routine has no recorded runs.".into()),
    }
}

fn refresh(window: &AmicaWindow) {
    let result = (|| -> Result<_, Box<dyn std::error::Error>> {
        let db = db()?;
        let mut statement =
            db.prepare("SELECT name, coalesce(shortcut, '') FROM routines ORDER BY name")?;
        Ok(statement
            .query_map([], |row| {
                Ok(RoutineRow {
                    name: row.get::<_, String>(0)?.into(),
                    shortcut: row.get::<_, String>(1)?.into(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    })();
    if let Ok(rows) = result {
        window.set_routine_rows(ModelRc::new(VecModel::from(rows)));
    }
}

fn set_result(window: &AmicaWindow, result: Result<String, String>) {
    window.set_routine_result(result.unwrap_or_else(|error| error).into());
}

struct Run {
    name: String,
    token: String,
    steps: Vec<Step>,
    entries: Vec<String>,
    next: usize,
}

fn run(window: &AmicaWindow, name: String, steps: Vec<Step>) {
    let token = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_default()
        .to_string();
    let entries = vec![format!(
        "{} · {name}",
        crate::memory::learned::time_label(now_ms())
    )];
    window.set_routine_run_id(token.as_str().into());
    window.set_routine_running(true);
    advance(
        window,
        Rc::new(RefCell::new(Run {
            name,
            token,
            steps,
            entries,
            next: 0,
        })),
    );
}

fn advance(window: &AmicaWindow, running: Rc<RefCell<Run>>) {
    let (step, number) = {
        let mut run = running.borrow_mut();
        if !window.get_routine_running()
            || window.get_routine_run_id() != run.token
            || window.get_saver_active()
        {
            return;
        }
        let number = run.next + 1;
        let Some(step) = run.steps.get(run.next).cloned() else {
            window.set_routine_running(false);
            run.entries.push("Completed.".into());
            publish_run(window, &run);
            return;
        };
        run.next = number;
        (step, number)
    };
    if let Step::Wait(seconds) = step {
        {
            let mut run = running.borrow_mut();
            run.entries
                .push(format!("Step {number}: waiting {seconds}s"));
            publish_run(window, &run);
        }
        schedule(
            window,
            running,
            Duration::from_secs(u64::from(seconds)),
            number,
        );
        return;
    }
    let result = execute(window, &step);
    {
        let mut run = running.borrow_mut();
        run.entries.push(format!(
            "Step {number}: {}",
            result.as_ref().map_or("failed", |_| "completed")
        ));
        if result.is_err() {
            window.set_routine_running(false);
            publish_run(window, &run);
            let error = result.err().unwrap_or_default();
            window.set_routine_result(format!("{}\n{error}", run.entries.join("\n")).into());
            return;
        }
        publish_run(window, &run);
    }
    schedule(window, running, Duration::from_millis(50), 0);
}

fn schedule(window: &AmicaWindow, running: Rc<RefCell<Run>>, delay: Duration, wait_step: usize) {
    let weak = window.as_weak();
    Timer::single_shot(delay, move || {
        if let Some(window) = weak.upgrade() {
            {
                let mut run = running.borrow_mut();
                if !window.get_routine_running() || window.get_routine_run_id() != run.token {
                    return;
                }
                if wait_step > 0 {
                    run.entries.push(format!("Step {wait_step}: completed"));
                    publish_run(&window, &run);
                }
            }
            advance(&window, running);
        }
    });
}

fn execute(window: &AmicaWindow, step: &Step) -> Result<(), String> {
    match step {
        Step::App(app) => {
            if let Some(panel) = crate::media::panel_for(app) {
                window.invoke_show_panel(panel);
            } else {
                desktop::open_app(app)?;
            }
        }
        Step::Folder(folder) => desktop::open_folder(folder)?,
        Step::Timer(seconds, label) => {
            let db = crate::actions::open().map_err(|_| DB_ERROR)?;
            crate::timers::start(&db, i64::from(*seconds), label, now_ms())?;
        }
        Step::Pause => {
            crate::media::local_command(
                window,
                crate::actions::command::MediaAction::Play("current".into(), false),
            )?;
        }
        Step::SaveLayout(name) => {
            #[cfg(windows)]
            {
                let steps: Vec<_> = desktop::layout::snapshot()
                    .into_iter()
                    .map(Step::Layout)
                    .collect();
                save(&db()?, name, &steps)?;
            }
            #[cfg(not(windows))]
            return Err(format!("Saving window layouts is unavailable: {name}."));
        }
        Step::ShowTasks => window.invoke_show_panel(Panel::Daily),
        Step::Layout(saved) => {
            #[cfg(windows)]
            desktop::layout::restore_saved(saved)?;
            #[cfg(not(windows))]
            return Err(format!("Window placement is unavailable: {}.", saved.app));
        }
        Step::Wait(_) => (),
    }
    Ok(())
}

fn publish_run(window: &AmicaWindow, run: &Run) {
    let result = run.entries.join("\n");
    window.set_routine_result(result.as_str().into());
    if !window.get_private_chat()
        && let Ok(db) = db()
    {
        let _ = db.execute(
            "UPDATE routines SET last_run = ?1 WHERE name = ?2",
            params![result, run.name],
        );
    }
}

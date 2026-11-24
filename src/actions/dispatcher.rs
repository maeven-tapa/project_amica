#[cfg(windows)]
use std::cell::RefCell;
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};
use slint::{ComponentHandle, ModelRc, Timer, VecModel};

use crate::{AmicaWindow, Panel, desktop, local_time::now_ms};
use crate::{daily, memory, timers};

#[cfg(any(windows, target_os = "linux"))]
use super::command::FileAction;
#[cfg(windows)]
use super::command::{InputAction, WindowControl};
use super::{
    catalog,
    command::{BrowserAction, Command, MediaAction, SystemAction},
};

#[cfg(windows)]
#[derive(Default)]
struct State {
    recent: Option<desktop::TargetWindow>,
    target: Option<desktop::TargetWindow>,
    grid: Option<(desktop::TargetWindow, u8, u8, crate::GridOverlay)>,
    marker: Option<crate::PointerMarker>,
}

#[cfg(windows)]
thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

pub fn initialize(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS action_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            occurred_at INTEGER NOT NULL,
            family TEXT NOT NULL,
            outcome TEXT NOT NULL
         );",
    )
}

pub fn connect(window: &AmicaWindow) -> Timer {
    let weak = window.as_weak();
    window.on_run_local_command(move |input| {
        let Some(window) = weak.upgrade() else { return };
        let result = catalog::parse(&input)
            .filter(|command| *command != Command::Companion)
            .map_or_else(
                || "No local command matches. Try an example from the palette.".into(),
                |command| dispatch(&window, command, Instant::now()),
            );
        window.set_command_result(result.into());
    });
    let weak = window.as_weak();
    window.on_add_daily(move |label, reminder| {
        if let Some(window) = weak.upgrade() {
            let command = if reminder {
                Command::Reminder(1800, label.into())
            } else {
                Command::Task(label.into())
            };
            window.set_command_result(dispatch(&window, command, Instant::now()).into());
        }
    });
    let weak = window.as_weak();
    window.on_finish_daily(move |id, checked| {
        if let Some(window) = weak.upgrade() {
            finish_daily(&window, id.as_str(), checked);
        }
    });

    let weak = window.as_weak();
    window.on_stop_desktop(move || {
        if let Some(window) = weak.upgrade() {
            stop(&window);
        }
    });
    let weak = window.as_weak();
    window.on_search_palette(move |query| {
        if let Some(window) = weak.upgrade() {
            let needle = query.to_lowercase();
            let rows = super::catalog::examples()
                .into_iter()
                .filter(|entry| {
                    needle.is_empty()
                        || entry.phrase.to_lowercase().contains(&needle)
                        || entry.outcome.to_lowercase().contains(&needle)
                        || entry.language.to_lowercase() == needle
                })
                .collect::<Vec<_>>();
            window.set_palette_rows(ModelRc::new(VecModel::from(rows)));
        }
    });
    let timer = Timer::default();
    #[cfg(windows)]
    {
        let weak = window.as_weak();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(300),
            move || {
                let Some(window) = weak.upgrade() else { return };
                if !window.get_saver_active()
                    && let Ok(front) = desktop::TargetWindow::find("current")
                {
                    STATE.with_borrow_mut(|state| state.recent = Some(front));
                }
            },
        );
    }
    timer
}

fn stop(window: &AmicaWindow) {
    window.set_desktop_control(false);
    window.set_desktop_target("".into());
    window.set_power_request("".into());
    window.invoke_stop_routine();
    #[cfg(windows)]
    STATE.with_borrow_mut(|state| {
        if let Some((_, _, _, overlay)) = state.grid.take() {
            let _ = overlay.hide();
        }
        if let Some(marker) = state.marker.take() {
            let _ = marker.hide();
        }
        state.target = None;
    });
}

pub fn available(command: &Command) -> bool {
    #[cfg(target_os = "linux")]
    return match command {
        Command::Input(..)
        | Command::Window(..)
        | Command::Clipboard(..)
        | Command::CopyTitle(..)
        | Command::Capture(..) => false,
        Command::Files(action) => matches!(action, FileAction::OpenFolder(_)),
        Command::Browser(action) => {
            matches!(action, BrowserAction::Open(_) | BrowserAction::Search(_))
        }
        Command::System(action) => {
            matches!(action, SystemAction::Volume(..) | SystemAction::Metric(_))
        }
        Command::Media(MediaAction::Rate(_)) => false,
        Command::Routines(
            super::command::RoutineAction::SaveLayout(_)
            | super::command::RoutineAction::RestoreLayout(_),
        ) => false,
        Command::Launch(id) => desktop::linux::supports(id),
        _ => true,
    };
    #[cfg(not(target_os = "linux"))]
    !matches!(command, Command::System(SystemAction::Output(..)))
}

pub fn keeps_transcript(command: &Command) -> bool {
    matches!(
        command,
        Command::Companion
            | Command::Palette
            | Command::View(_)
            | Command::Timer(..)
            | Command::Note(_)
            | Command::FindNote(_)
            | Command::Task(_)
            | Command::Reminder(..)
            | Command::Saver(..)
    )
}

pub fn dispatch(window: &AmicaWindow, command: Command, created: Instant) -> String {
    if created.elapsed() >= Duration::from_secs(12)
        || (window.get_saver_active() && command != Command::Stop)
    {
        return "This request expired. Enter it again when Amica is visible.".into();
    }
    let family = command.family();
    let needs_mode = !matches!(&command, Command::System(SystemAction::Metric(_)))
        && matches!(
            &command,
            Command::Input(..)
                | Command::Window(..)
                | Command::Clipboard(..)
                | Command::CopyTitle(..)
                | Command::Files(..)
                | Command::Browser(..)
                | Command::System(..)
                | Command::Media(..)
                | Command::Capture(..)
        );
    let result = if needs_mode && !window.get_desktop_control() {
        Err("Start Desktop Control first, then repeat the command.".into())
    } else {
        execute(window, command)
    };
    if !window.get_private_chat()
        && let Ok(db) = crate::storage::open()
    {
        let outcome = if result.is_ok() {
            "handled"
        } else {
            "blocked_or_failed"
        };
        let _ = db.execute(
            "INSERT INTO action_history(occurred_at, family, outcome) VALUES (?1, ?2, ?3)",
            params![now_ms(), family, outcome],
        );
    }
    result.unwrap_or_else(|error| error)
}

fn db() -> Result<Connection, String> {
    super::open().map_err(|_| "Amica could not open local storage.".into())
}

fn execute(window: &AmicaWindow, command: Command) -> Result<String, String> {
    match command {
        Command::Stop => {
            stop(window);
            Ok("Stopped Desktop Control and pending steps.".into())
        }
        Command::DesktopMode(true) => {
            window.set_desktop_control(true);
            #[cfg(target_os = "linux")]
            return Ok(
                "Desktop Control is on. Use the palette to find available Linux commands.".into(),
            );
            #[cfg(not(target_os = "linux"))]
            Ok("Desktop Control is on. Choose a target window or bring it forward.".into())
        }
        Command::DesktopMode(false) => {
            stop(window);
            Ok("Desktop Control is off.".into())
        }
        Command::Palette => {
            window.invoke_search_palette("".into());
            window.invoke_show_panel(Panel::CommandPalette);
            window.invoke_focus_palette();
            Ok("Opened the command palette.".into())
        }
        Command::Timer(seconds, label) => {
            let timer = timers::start(&db()?, i64::from(seconds), limited(&label, 80)?, now_ms())?;
            if label != "Focus" {
                window.invoke_show_panel(Panel::TimeDate);
            }
            Ok(format!(
                "Started {} for {} seconds.",
                timer.label, timer.remaining_seconds
            ))
        }
        Command::Task(label) => {
            let db = db()?;
            daily::add(&db, daily::Kind::Task, limited(&label, 300)?, None)
                .map_err(|_| "Amica could not save the task.")?;
            refresh_daily(window, &db);
            window.invoke_show_panel(Panel::Daily);
            Ok("Added the local task.".into())
        }
        Command::Reminder(seconds, label) => {
            if !(1..=2_592_000).contains(&seconds) {
                return Err("Choose a reminder delay within the next 30 days.".into());
            }
            let db = db()?;
            let due = now_ms() + i64::from(seconds) * 1000;
            daily::add(&db, daily::Kind::Reminder, limited(&label, 300)?, Some(due))
                .map_err(|_| "Amica could not schedule the reminder.")?;
            refresh_daily(window, &db);
            window.invoke_show_panel(Panel::Daily);
            Ok(format!(
                "Set a reminder for {}.",
                memory::learned::time_label(due)
            ))
        }
        Command::Note(note) => {
            let note = limited(&note, 2000)?;
            let title: String = note.chars().take(55).collect();
            memory::save(&db()?, "note", &title, note, now_ms())
                .map_err(|_| "Amica could not save the note.")?;
            window.invoke_show_panel(Panel::Saved);
            Ok(format!("Saved “{title}”."))
        }
        Command::FindNote(name) => {
            let notes = memory::find(&db()?, Some("note"), &name, 15)
                .map_err(|_| "Amica could not search notes.")?;
            if notes.is_empty() {
                return Err(format!("No saved note matches “{name}”."));
            }
            window.invoke_show_panel(Panel::Saved);
            let mut matches = notes;
            matches.sort_by_key(|item| !item.title.eq_ignore_ascii_case(&name));
            Ok(matches
                .iter()
                .take(4)
                .map(|item| format!("{}\n{}", item.title, item.value))
                .collect::<Vec<_>>()
                .join("\n\n"))
        }
        Command::View(name) => {
            let panel = match name.as_str() {
                "home" => Panel::None,
                "conversation" => Panel::Conversation,
                "time" => Panel::TimeDate,
                "saved" => Panel::Saved,
                "memory map" | "recent memories" => Panel::MemoryMap,
                "apps" => Panel::Apps,
                "settings" => Panel::Settings,
                "system specs" => Panel::SystemSpecs,
                "tasks" => Panel::Daily,
                "routines" => Panel::Routines,
                _ => return Err("No Amica view matches that name.".into()),
            };
            if panel == Panel::Routines {
                window.invoke_refresh_routines();
            }
            window.invoke_show_panel(panel);
            if panel == Panel::Conversation {
                window.invoke_focus_conversation();
            }
            if name == "recent memories" {
                window.invoke_focus_recent_memories();
            }
            Ok(format!("Opened {name}."))
        }
        Command::FindChat(query) => {
            if window.get_private_chat() {
                return Err("End the private session to search saved conversations.".into());
            }
            window.invoke_show_panel(Panel::Conversation);
            window.invoke_show_chat_search(query.into());
            Ok("Showing matching local conversations.".into())
        }
        Command::Launch(name) => {
            if let Some(panel) = crate::media::panel_for(&name) {
                window.invoke_show_panel(panel);
            } else {
                desktop::open_app(&name)?;
            }
            Ok(format!("Opened {}.", name.replace('_', " ")))
        }
        Command::Saver(preset) => {
            window.invoke_enter_saver(preset.into());
            Ok("Started the screensaver.".into())
        }
        Command::SaverSetting(key, value) => {
            window.invoke_save_saver_setting(key.clone().into(), value.into());
            if !window.get_settings_error().is_empty() {
                return Err(window.get_settings_error().into());
            }
            if key == "saver_palette" {
                window.invoke_enter_saver("Ambient".into());
            } else {
                window.invoke_show_panel(Panel::Settings);
            }
            Ok("Updated the screensaver setting.".into())
        }
        Command::Power(name) => {
            let action = match name.as_str() {
                "shutdown" => desktop::PowerAction::Shutdown,
                "restart" => desktop::PowerAction::Restart,
                "sleep" => desktop::PowerAction::Sleep,
                _ => return Err("Choose restart, shutdown, or sleep.".into()),
            };
            desktop::request_power(window, action);
            Ok("Review the PC power request in Amica.".into())
        }
        Command::Routines(action) => crate::routines::command(window, action),
        Command::Media(action) => media(window, action),
        Command::Companion => Err("Type this conversation request in Chat.".into()),
        other => platform(window, other),
    }
}

pub fn finish_daily(window: &AmicaWindow, id: &str, checked: bool) {
    let result = (|| -> Result<(), String> {
        let db = db()?;
        let id = id
            .parse::<i64>()
            .map_err(|_| "The task selection is unavailable.")?;
        daily::finish(&db, id, checked).map_err(|_| "Amica could not update the task.")?;
        refresh_daily(window, &db);
        Ok(())
    })();
    if let Err(error) = result {
        window.set_command_result(error.into());
    }
}

fn refresh_daily(window: &AmicaWindow, db: &Connection) {
    if let Ok(items) = daily::list(db) {
        daily::publish(window, items);
    }
}

fn limited(value: &str, max: usize) -> Result<&str, &'static str> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max {
        Err("Enter a shorter, nonempty value.")
    } else {
        Ok(value)
    }
}

fn media(window: &AmicaWindow, action: MediaAction) -> Result<String, String> {
    match action {
        MediaAction::Search(service, query) => {
            let mut url = match service.to_lowercase().as_str() {
                "youtube" => reqwest::Url::parse("https://www.youtube.com/results").unwrap(),
                "spotify" => reqwest::Url::parse("https://open.spotify.com/search/").unwrap(),
                _ => return Err("Choose YouTube or Spotify.".into()),
            };
            if service.eq_ignore_ascii_case("youtube") {
                url.query_pairs_mut().append_pair("search_query", &query);
            } else if let Ok(mut path) = url.path_segments_mut() {
                path.pop_if_empty().push(&query);
            }
            platform(window, Command::Browser(BrowserAction::Open(url.into())))
        }
        action => crate::media::local_command(window, action),
    }
}

#[cfg(target_os = "linux")]
fn platform(_: &AmicaWindow, command: Command) -> Result<String, String> {
    match command {
        Command::Files(FileAction::OpenFolder(folder)) => {
            desktop::open_folder(&folder)?;
            Ok("Opened the folder in the file manager.".into())
        }
        Command::Browser(BrowserAction::Open(url)) => {
            desktop::open_browser_url(&url)?;
            Ok("Opened the address in the default browser.".into())
        }
        Command::Browser(BrowserAction::Search(query)) => {
            let mut url = reqwest::Url::parse("https://www.google.com/search").unwrap();
            url.query_pairs_mut()
                .append_pair("q", limited(&query, 500)?);
            desktop::open_browser_url(url.as_str())?;
            Ok("Opened a browser search for the requested query.".into())
        }
        Command::System(SystemAction::Metric(name)) => desktop::linux::metric(&name),
        Command::System(SystemAction::Volume(amount, relative)) => {
            desktop::linux::set_volume(amount, relative).map_err(str::to_owned)
        }
        _ => Err("This desktop action is unavailable on Linux.".into()),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn platform(_: &AmicaWindow, _: Command) -> Result<String, String> {
    Err("This desktop action is unavailable on this platform.".into())
}

#[cfg(windows)]
fn target(window: &AmicaWindow, name: &str) -> Result<desktop::TargetWindow, String> {
    let chosen = if name.trim().is_empty() || name.eq_ignore_ascii_case("current") {
        STATE
            .with_borrow(|state| state.target.or(state.recent))
            .or_else(|| desktop::TargetWindow::find("current").ok())
            .ok_or("Bring the intended app forward, or use Switch to followed by its name.")?
    } else {
        desktop::TargetWindow::find(name.trim())?
    };
    window.set_desktop_target(desktop::input::title(chosen)?.into());
    STATE.with_borrow_mut(|state| state.target = Some(chosen));
    Ok(chosen)
}

#[cfg(windows)]
fn platform(window: &AmicaWindow, command: Command) -> Result<String, String> {
    use crate::desktop::{files, input, system, windows};
    match command {
        Command::Input(_, InputAction::Cell(number)) => grid_cell(window, number),
        Command::Input(_, InputAction::Mark(color)) => mark(&color),
        Command::Input(name, InputAction::Grid(columns, rows)) => {
            grid(window, target(window, &name)?, columns, rows)
        }
        Command::Input(name, InputAction::Paste) => {
            let selector = match name.to_lowercase().as_str() {
                name if name.starts_with("notepad ") => "Notepad",
                name if name.starts_with("explorer ") => "File Explorer",
                name if name.starts_with("browser ") => "",
                _ => name.as_str(),
            };
            let target = target(window, selector)?;
            let title = input::title(target)?;
            desktop::confirm(
                window,
                format!("Paste into the focused field of {title}?"),
                move |window| {
                    if window.get_desktop_control() {
                        display(window, input::apply(target, &InputAction::Paste));
                    }
                },
            );
            Ok("Review the paste target in Amica.".into())
        }
        Command::Input(name, action) => {
            hide_grid();
            input::apply(target(window, &name)?, &action)
        }
        Command::Window(name, action) => {
            hide_grid();
            let selected = target(window, &name)?;
            match action {
                WindowControl::Switch => {
                    input::activate(selected)?;
                    Ok("Switched to the selected window.".into())
                }
                WindowControl::Find => Ok(format!("Selected window: {}", input::title(selected)?)),
                WindowControl::Mode(mode) => {
                    mode.apply(selected)?;
                    Ok("Sent the window request.".into())
                }
                other => desktop::layout::apply(selected, other),
            }
        }
        Command::Clipboard(text) => {
            let owner = desktop::window_handle(window)
                .ok_or("Amica could not access the Windows clipboard.")?;
            desktop::clipboard::set(owner, limited(&text, 2000)?)?;
            Ok("Copied the literal text to the Windows clipboard.".into())
        }
        Command::CopyTitle(name) => {
            let title = input::title(target(window, &name)?)?;
            let owner = desktop::window_handle(window)
                .ok_or("Amica could not access the Windows clipboard.")?;
            desktop::clipboard::set(owner, &title)?;
            Ok("Copied the selected window title.".into())
        }
        Command::Capture(name) => {
            let target = target(window, &name)?;
            let title = input::title(target)?;
            let folder = files::known_folder("Pictures")?.join("Amica");
            let destination = folder.join(format!(
                "Capture-{}.bmp",
                chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")
            ));
            desktop::confirm(
                window,
                format!("Capture {title}\nSave to {}", destination.display()),
                move |window| {
                    if window.get_desktop_control() {
                        display(window, desktop::capture::save(target, &destination));
                    }
                },
            );
            Ok("Review the window and screenshot destination in Amica.".into())
        }
        Command::Files(action) => match action {
            FileAction::OpenFolder(_) => files::run(None, &action),
            FileAction::SaveCopy(name) => {
                let editor = target(window, "Notepad")?;
                let title = input::title(editor)?;
                let folder = files::known_folder("Documents")?;
                let destination = files::child(&folder, &name)?;
                if destination.exists() {
                    return Err("That path already exists. Choose a different name or review overwrite in the editor.".into());
                }
                desktop::confirm(
                    window,
                    format!("Save a copy of {title}?\n{}", destination.display()),
                    move |window| {
                        if window.get_desktop_control() {
                            let result = if input::title(editor).as_deref() == Ok(title.as_str()) {
                                files::save_copy(editor, &destination)
                            } else {
                                Err("The Notepad document changed. Request a new preview.".into())
                            };
                            display(window, result);
                        }
                    },
                );
                Ok("Review the Notepad source and exact destination in Amica.".into())
            }
            FileAction::Move(_)
            | FileAction::Copy(_)
            | FileAction::Rename(_)
            | FileAction::Recycle(_) => {
                let selected = target(window, "File Explorer")?;
                let change = files::preview(selected, &action)?;
                desktop::confirm(window, change.description(), move |window| {
                    if window.get_desktop_control() {
                        display(window, change.run());
                    }
                });
                Ok("Review the exact file operation in Amica.".into())
            }
            _ => files::run(Some(target(window, "File Explorer")?), &action),
        },
        Command::Browser(action) => match action {
            BrowserAction::Open(url) => {
                let url = reqwest::Url::parse(&url).map_err(|_| "Enter a valid HTTPS address.")?;
                if url.scheme() != "https"
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err("Use an HTTPS address without credentials.".into());
                }
                windows::open_protocol(url.as_str())?;
                Ok("Opened the address in the default browser.".into())
            }
            BrowserAction::Search(query) => {
                let mut url = reqwest::Url::parse("https://www.google.com/search").unwrap();
                url.query_pairs_mut()
                    .append_pair("q", limited(&query, 500)?);
                windows::open_protocol(url.as_str())?;
                Ok("Opened a browser search for the requested query.".into())
            }
            BrowserAction::Tab(name) => {
                let selected = target(window, "")?;
                if !desktop::layout::is_browser(selected) {
                    return Err("Choose a supported browser as the Desktop Control target.".into());
                }
                input::apply(selected, &InputAction::Invoke(format!("{name} tab")))
            }
            BrowserAction::History(count) => {
                let selected = target(window, "")?;
                if !desktop::layout::is_browser(selected) {
                    return Err("Choose a supported browser as the Desktop Control target.".into());
                }
                input::apply(
                    selected,
                    &InputAction::Keys(
                        if count < 0 { "Alt+Left" } else { "Alt+Right" }.into(),
                        count.unsigned_abs(),
                    ),
                )
            }
            BrowserAction::Zoom(percent) => {
                let selected = target(window, "")?;
                if !desktop::layout::is_chromium(selected) {
                    return Err("Exact page zoom is available for Chromium browsers exposed to Windows accessibility.".into());
                }
                let levels = [
                    25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500,
                ];
                let index = levels
                    .iter()
                    .position(|&level| level == percent)
                    .ok_or("Choose a zoom preset available in the browser.")?
                    as i32;
                input::activate(selected)?;
                input::keys(selected, "Ctrl+0", 1)?;
                let steps = index - 7;
                if steps != 0 {
                    input::keys(
                        selected,
                        if steps < 0 {
                            "Ctrl+Minus"
                        } else {
                            "Ctrl+Equals"
                        },
                        steps.unsigned_abs() as u8,
                    )?;
                }
                match desktop::accessibility::browser_zoom(selected) {
                    Some(actual) => Ok(format!("Browser zoom is {actual}%.")),
                    None if steps == 0 => Ok("Sent the browser’s reset zoom shortcut.".into()),
                    None => Ok(format!(
                        "Requested {percent}% zoom. The browser did not expose the result."
                    )),
                }
            }
            BrowserAction::Bookmark(name) => {
                limited(&name, 100)?;
                let selected = target(window, "")?;
                if !desktop::layout::is_browser(selected) {
                    return Err("Choose a supported browser first.".into());
                }
                input::activate(selected)?;
                let address = desktop::accessibility::page_address(selected)?;
                desktop::confirm(
                    window,
                    format!("Bookmark this page as {name}?\n{address}"),
                    move |window| {
                        if window.get_desktop_control() {
                            let result = (|| -> Result<String, String> {
                                input::activate(selected)?;
                                if desktop::accessibility::page_address(selected)? != address {
                                    return Err(
                                        "The page changed. Request a new bookmark preview.".into(),
                                    );
                                }
                                input::keys(selected, "Ctrl+D", 1)?;
                                for _ in 0..6 {
                                    if desktop::accessibility::name_bookmark(selected, &name)
                                        .is_ok()
                                    {
                                        return Ok(format!("Saved the bookmark as {name}."));
                                    }
                                    std::thread::sleep(Duration::from_millis(25));
                                }
                                Ok("Opened the browser bookmark editor. Confirm the bookmark name there.".into())
                            })();
                            display(window, result);
                        }
                    },
                );
                Ok("Review the URL and bookmark name in Amica.".into())
            }
            BrowserAction::Reopen(count) => {
                let selected = target(window, "")?;
                let batch = desktop::browser::recent(selected, count)?;
                desktop::confirm(window, batch.preview(), move |window| {
                    if window.get_desktop_control() {
                        display(window, batch.open());
                    }
                });
                Ok("Review the recent browser pages in Amica.".into())
            }
        },
        Command::System(action) => {
            let target_window = if matches!(action, SystemAction::Brightness(_)) {
                Some(target(window, "")?)
            } else {
                None
            };
            system::execute(&action, target_window)
        }
        _ => Err("This action has no local adapter.".into()),
    }
}

#[cfg(windows)]
fn display(window: &AmicaWindow, result: Result<String, String>) {
    window.set_command_result(result.unwrap_or_else(|error| error).into());
}

#[cfg(windows)]
fn hide_grid() {
    STATE.with_borrow_mut(|state| {
        if let Some((_, _, _, overlay)) = state.grid.take() {
            let _ = overlay.hide();
        }
    });
}

#[cfg(windows)]
fn grid(
    window: &AmicaWindow,
    target: desktop::TargetWindow,
    columns: u8,
    rows: u8,
) -> Result<String, String> {
    if !(2..=6).contains(&columns) || !(2..=6).contains(&rows) {
        return Err("Use a mouse grid from 2 × 2 through 6 × 6.".into());
    }
    hide_grid();
    desktop::input::activate(target)?;
    let rect = desktop::input::client(target)?;
    let overlay =
        crate::GridOverlay::new().map_err(|_| "Amica could not create the mouse grid.")?;
    overlay.set_columns(i32::from(columns));
    overlay.set_rows(i32::from(rows));
    overlay
        .window()
        .set_position(slint::PhysicalPosition::new(rect.left, rect.top));
    overlay.window().set_size(slint::PhysicalSize::new(
        (rect.right - rect.left) as u32,
        (rect.bottom - rect.top) as u32,
    ));
    let weak = window.as_weak();
    overlay.on_cancel(move || {
        if let Some(window) = weak.upgrade() {
            hide_grid();
            window.set_command_result("Closed the mouse grid.".into());
        }
    });
    let weak = window.as_weak();
    overlay.on_select_cell(move |number| {
        if let Some(window) = weak.upgrade()
            && window.get_desktop_control()
        {
            display(&window, grid_cell(&window, number as u8));
        }
    });
    overlay
        .show()
        .map_err(|_| "Amica could not show the mouse grid.")?;
    STATE.with_borrow_mut(|state| state.grid = Some((target, columns, rows, overlay)));
    Ok(format!(
        "Mouse grid shown. Choose a cell from 1 to {}.",
        columns * rows
    ))
}

#[cfg(windows)]
fn grid_cell(window: &AmicaWindow, number: u8) -> Result<String, String> {
    let (target, columns, rows, overlay) = STATE
        .with_borrow_mut(|state| state.grid.take())
        .ok_or("Show the mouse grid first.")?;
    if number == 0 || number > columns * rows {
        STATE.with_borrow_mut(|state| state.grid = Some((target, columns, rows, overlay)));
        return Err("Choose a number in the visible mouse grid.".into());
    }
    overlay
        .hide()
        .map_err(|_| "Amica could not dismiss the mouse grid.")?;
    window.set_desktop_target(desktop::input::title(target)?.into());
    desktop::input::click_cell(target, columns, rows, number).map_err(str::to_owned)
}

#[cfg(windows)]
fn mark(color: &str) -> Result<String, String> {
    let style = color.to_lowercase();
    let color = match color.to_lowercase().as_str() {
        "white" => (255, 255, 255),
        "black" => (0, 0, 0),
        "cyan" => (59, 255, 255),
        "yellow" => (255, 232, 80),
        "green" => (105, 242, 132),
        "red" => (255, 101, 99),
        "blue" => (98, 158, 255),
        "purple" => (213, 151, 255),
        "dashed circle" | "crosshairs" | "large square" | "small square" => (211, 235, 255),
        _ => return Err("Choose white, black, cyan, yellow, green, red, blue, or purple.".into()),
    };
    let point = desktop::input::cursor()?;
    let marker =
        crate::PointerMarker::new().map_err(|_| "Amica could not create the pointer marker.")?;
    marker.set_ring(slint::Color::from_rgb_u8(color.0, color.1, color.2));
    marker.set_shape(style.as_str().into());
    let size = match style.as_str() {
        "large square" => 92,
        "small square" => 48,
        _ => 70,
    };
    marker
        .window()
        .set_size(slint::PhysicalSize::new(size, size));
    marker.window().set_position(slint::PhysicalPosition::new(
        point.x - size as i32 / 2,
        point.y - size as i32 / 2,
    ));
    marker
        .show()
        .map_err(|_| "Amica could not show the pointer marker.")?;
    let weak = marker.as_weak();
    if let Some(previous) = STATE.with_borrow_mut(|state| state.marker.replace(marker)) {
        let _ = previous.hide();
    }
    Timer::single_shot(Duration::from_millis(1100), move || {
        if let Some(marker) = weak.upgrade() {
            let _ = marker.hide();
        }
    });
    Ok("Highlighted the current pointer location.".into())
}

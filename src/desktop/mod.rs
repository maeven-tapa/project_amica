#[cfg(windows)]
pub(crate) mod accessibility;
#[cfg(windows)]
pub(crate) mod browser;
#[cfg(windows)]
pub(crate) mod capture;
#[cfg(windows)]
pub(crate) mod clipboard;
#[cfg(windows)]
mod external;
#[cfg(windows)]
pub(crate) mod files;
#[cfg(windows)]
pub(crate) mod input;
#[cfg(windows)]
pub(crate) mod layout;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(windows)]
pub(crate) mod system;
mod window_action;
#[cfg(windows)]
pub(crate) mod windows;

use slint::{ComponentHandle, Timer};

use crate::AmicaWindow;

pub use window_action::{ExternalAction, PowerAction, TargetWindow};
pub use window_action::{WINDOW_MODE_IDS, WindowAction};

#[cfg(windows)]
pub(crate) use external::window_handle;

pub const APP_IDS: &[&str] = &[
    "spotify",
    "youtube",
    "browser",
    "calculator",
    "notepad",
    "paint",
    "photos",
    "clock",
    "snipping_tool",
    "edge",
    "microsoft_store",
    "file_explorer",
    "settings",
];

pub fn connect(window: &AmicaWindow) {
    let weak = window.as_weak();
    window.on_launch_app(move |id| {
        let Some(window) = weak.upgrade() else { return };
        if let Some(panel) = crate::media::panel_for(&id) {
            window.invoke_show_panel(panel);
            return;
        }
        launch(&window, move || open_app(&id));
    });
}

#[cfg(any(windows, target_os = "linux"))]
pub fn open_media_page(window: &AmicaWindow, url: String) {
    launch(window, move || open_page(&url));
}

fn launch(
    window: &AmicaWindow,
    operation: impl FnOnce() -> Result<(), &'static str> + Send + 'static,
) {
    if window.get_app_launch_pending() {
        return;
    }
    window.set_app_launch_pending(true);
    window.set_app_launch_error("".into());
    let target = window.as_weak();
    if std::thread::Builder::new()
        .name("amica-app-launch".into())
        .spawn(move || {
            let result = operation();
            let _ = target.upgrade_in_event_loop(move |window| {
                window.set_app_launch_pending(false);
                window.set_app_launch_error(result.err().unwrap_or_default().into());
                if window.get_active_panel() != crate::Panel::Apps {
                    report_window_error(&window, result);
                }
            });
        })
        .is_err()
    {
        window.set_app_launch_pending(false);
        report_window_error(window, Err("Amica could not start the app launcher."));
    }
}

pub fn open_search_results(window: &AmicaWindow, query: String) {
    launch(window, move || {
        let mut page = reqwest::Url::parse("https://www.google.com/search").unwrap();
        page.query_pairs_mut().append_pair("q", &query);
        open_page(page.as_str())
    });
}

fn open_page(url: &str) -> Result<(), &'static str> {
    #[cfg(windows)]
    return windows::open_page(url);
    #[cfg(target_os = "linux")]
    return linux::open_page(url);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = url;
        Err("Opening web pages is unavailable on this platform.")
    }
}

pub fn open_folder(folder: &str) -> Result<(), &'static str> {
    #[cfg(windows)]
    return windows::open_path(&files::known_folder(folder)?);
    #[cfg(target_os = "linux")]
    return linux::open_folder(folder);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = folder;
        Err("Opening folders is unavailable on this platform.")
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn open_browser_url(url: &str) -> Result<(), &'static str> {
    let url = reqwest::Url::parse(url).map_err(|_| "Enter a valid HTTPS address.")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Use an HTTPS address without credentials.");
    }
    open_page(url.as_str())
}

pub fn open_app(id: &str) -> Result<(), &'static str> {
    if !APP_IDS.contains(&id) {
        return Err("Choose a supported app from the Apps panel.");
    }

    #[cfg(windows)]
    {
        windows::open(id)
    }
    #[cfg(target_os = "linux")]
    {
        linux::open(id)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err("Amica’s app launcher is unavailable on this platform.")
    }
}

pub fn app_id(id: &str) -> Option<&'static str> {
    APP_IDS.iter().copied().find(|&name| name == id)
}

pub fn app_label(id: &str) -> String {
    if cfg!(target_os = "linux") {
        let label = match id {
            "edge" => "the Chromium shortcut",
            "microsoft_store" => "the software manager",
            "file_explorer" => "the file manager",
            "notepad" => "the text editor",
            "snipping_tool" => "the screenshot tool",
            _ => return id.replace('_', " "),
        };
        return label.into();
    }
    id.replace('_', " ")
}

pub fn restore_external_windows() {
    #[cfg(windows)]
    external::restore_all();
}

pub fn report_window_error(window: &AmicaWindow, result: Result<(), &'static str>) {
    let error = result.err().unwrap_or_default();
    window.set_desktop_error(error.into());
    if !error.is_empty() {
        let weak = window.as_weak();
        Timer::single_shot(std::time::Duration::from_secs(8), move || {
            if let Some(window) = weak.upgrade()
                && window.get_desktop_error() == error
            {
                window.set_desktop_error("".into());
            }
        });
    }
}

pub fn request_power(window: &AmicaWindow, action: PowerAction) {
    let label = match action {
        PowerAction::Shutdown => "Shut down this PC",
        PowerAction::Restart => "Restart this PC",
        PowerAction::Sleep => "Sleep this PC",
    };
    confirm(window, label.into(), move |window| {
        execute_power(window, action)
    });
}

pub fn confirm(window: &AmicaWindow, label: String, action: impl FnOnce(&AmicaWindow) + 'static) {
    WindowAction::Show.apply(window);
    window.invoke_exit_saver();
    window.invoke_dismiss_memory();
    let requested = std::time::Instant::now();
    let token = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_default()
        .to_string();
    window.set_power_request_id(token.as_str().into());
    window.set_power_request(label.into());
    let action = std::cell::RefCell::new(Some(action));
    let weak = window.as_weak();
    window.on_power_decision(move |confirmed| {
        let Some(window) = weak.upgrade() else { return };
        if window.get_power_request().is_empty() {
            return;
        }
        window.set_power_request("".into());
        if confirmed
            && requested.elapsed() < std::time::Duration::from_secs(30)
            && let Some(action) = action.borrow_mut().take()
        {
            action(&window);
        }
    });
    let weak = window.as_weak();
    Timer::single_shot(std::time::Duration::from_secs(30), move || {
        if let Some(window) = weak.upgrade()
            && window.get_power_request_id() == token
        {
            window.set_power_request("".into());
        }
    });
}

fn execute_power(window: &AmicaWindow, action: PowerAction) {
    restore_external_windows();
    let weak = window.as_weak();
    if std::thread::Builder::new()
        .name("amica-pc-power".into())
        .spawn(move || {
            let result = action.run();
            let _ = weak.upgrade_in_event_loop(move |window| report_window_error(&window, result));
        })
        .is_err()
    {
        report_window_error(window, Err("Amica could not send the PC power request."));
    }
}

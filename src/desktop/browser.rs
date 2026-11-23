use std::{fs, path::PathBuf, process::Command};

use rusqlite::{Connection, OpenFlags};

use super::{TargetWindow, external};

pub struct HistoryBatch {
    executable: PathBuf,
    profile: String,
    entries: Vec<(String, String)>,
}

impl HistoryBatch {
    pub fn preview(&self) -> String {
        let urls = self
            .entries
            .iter()
            .map(|(title, url)| format!("{title}\n{url}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        format!("Open these pages in profile {}?\n\n{urls}", self.profile)
    }

    pub fn open(self) -> Result<String, String> {
        let count = self.entries.len();
        let mut command = Command::new(&self.executable);
        command
            .arg(format!("--profile-directory={}", self.profile))
            .arg("--new-window");
        command.args(self.entries.iter().map(|(_, url)| url));
        command
            .spawn()
            .map_err(|_| "Windows could not launch the browser for this batch.")?;
        Ok(format!("Requested {count} tabs in the chosen browser."))
    }
}

pub fn recent(target: TargetWindow, count: u8) -> Result<HistoryBatch, String> {
    if !(1..=10).contains(&count) {
        return Err("Choose one to ten recent pages.".into());
    }
    let (vendor, app) = match external::process_name(target.process).as_str() {
        "chrome" => ("Google", "Chrome"),
        "msedge" => ("Microsoft", "Edge"),
        _ => {
            return Err(
                "Recent history previews are available for Chrome and Edge profiles.".into(),
            );
        }
    };
    let executable =
        external::process_path(target.process).ok_or("The selected browser closed.")?;
    let base =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Windows profile is unavailable.")?)
            .join(vendor)
            .join(app)
            .join("User Data");
    let local_state = fs::read(base.join("Local State"))
        .map_err(|_| "Browser profile metadata is unavailable.")?;
    let state: serde_json::Value = serde_json::from_slice(&local_state)
        .map_err(|_| "The browser profile metadata is unreadable.")?;
    let profile = state
        .pointer("/profile/last_used")
        .and_then(serde_json::Value::as_str)
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == ' ')
        })
        .ok_or("The browser did not identify a readable recent profile.")?;
    let db = Connection::open_with_flags(
        base.join(profile).join("History"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "The browser's history file is unavailable.")?;
    let mut statement = db
        .prepare(
            "SELECT title, url FROM urls WHERE hidden = 0 AND url LIKE 'https://%'
        ORDER BY last_visit_time DESC LIMIT ?1",
        )
        .map_err(|_| "This browser history format is unavailable.")?;
    let entries = statement
        .query_map([count], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "The browser did not expose its recent history.")?
        .filter_map(Result::ok)
        .filter(|(_, address)| {
            reqwest::Url::parse(address).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err("The chosen browser profile has no accessible recent HTTPS pages.".into());
    }
    Ok(HistoryBatch {
        executable,
        profile: profile.into(),
        entries,
    })
}

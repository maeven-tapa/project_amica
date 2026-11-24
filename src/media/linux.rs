use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use slint::{ComponentHandle, Timer, TimerMode};

use crate::{AmicaWindow, Panel, actions::command::MediaAction, desktop};

use super::{PlayRequest, Service};

pub(super) fn connect(window: &AmicaWindow) -> Timer {
    window.set_media_error("Use Open in browser to visit this service.".into());
    window.set_media_embedded(false);
    let weak = window.as_weak();
    window.on_open_media_external(move || {
        let Some(window) = weak.upgrade() else { return };
        let service = match window.get_active_panel() {
            Panel::Youtube => Service::YouTube,
            Panel::Spotify => Service::Spotify,
            _ => return,
        };
        desktop::open_media_page(&window, service.browser_page("").into());
    });
    let weak = window.as_weak();
    window.on_play_media(move |service, query| {
        let Some(window) = weak.upgrade() else { return };
        let Some(service) = Service::parse(&service) else {
            return;
        };
        match PlayRequest::new(service, &query) {
            Ok(request) => {
                desktop::open_media_page(&window, service.browser_page(&request.query).into())
            }
            Err(error) => desktop::report_window_error(&window, Err(error)),
        }
    });
    let timer = Timer::default();
    let pending = Arc::new(AtomicBool::new(false));
    let weak = window.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_secs(2), move || {
        let Some(window) = weak.upgrade() else { return };
        if !window.get_saver_active()
            || window.get_active_saver_preset() != "Dashboard"
            || !window.get_saver_show_media()
            || pending.swap(true, Ordering::Relaxed)
        {
            return;
        }
        let pending_update = Arc::clone(&pending);
        let weak = weak.clone();
        if std::thread::Builder::new()
            .name("amica-now-playing".into())
            .spawn(move || {
                let status = now_playing();
                let _ = weak.upgrade_in_event_loop(move |window| {
                    if window.get_home_media() != status {
                        window.set_home_media(status.into());
                    }
                });
                pending_update.store(false, Ordering::Relaxed);
            })
            .is_err()
        {
            pending.store(false, Ordering::Relaxed);
        }
    });
    timer
}

fn now_playing() -> String {
    playerctl(&[
        "--all-players",
        "--format={{status}}\u{1f}{{artist}}\u{1f}{{title}}",
        "metadata",
    ])
    .ok()
    .and_then(|rows| {
        rows.lines().find_map(|row| {
            let details = row.strip_prefix("Playing\u{1f}")?;
            let (artist, title) = details.split_once('\u{1f}')?;
            match (artist.trim(), title.trim()) {
                (_, "") => None,
                ("", title) => Some(title.into()),
                (artist, title) => Some(format!("{title} · {artist}")),
            }
        })
    })
    .unwrap_or_else(|| "No active media".into())
}

fn playerctl(args: &[&str]) -> Result<String, &'static str> {
    let output = Command::new("playerctl")
        .args(args)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "Install playerctl to control Linux media players."
            } else {
                "Could not start playerctl."
            }
        })?;
    if !output.status.success() {
        return Err("No compatible player is active, or the player declined the command.");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

pub(super) fn source() -> Result<String, &'static str> {
    let players = playerctl(&["--list-all"])?;
    for player in players.lines() {
        let option = format!("--player={player}");
        if playerctl(&[&option, "status"]).as_deref() == Ok("Playing") {
            return Ok(player.into());
        }
    }
    players
        .lines()
        .next()
        .map(str::to_owned)
        .ok_or("No compatible media player is active.")
}

pub(super) fn run(action: MediaAction) -> Result<String, &'static str> {
    let player = match &action {
        MediaAction::Play(name, _) if !name.eq_ignore_ascii_case("current") => {
            let players = playerctl(&["--list-all"])?;
            let query = name.trim().to_ascii_lowercase();
            if let Some(exact) = players
                .lines()
                .find(|player| player.eq_ignore_ascii_case(&query))
            {
                exact.to_owned()
            } else {
                let prefix = format!("{query}.");
                let mut matches = players
                    .lines()
                    .filter(|player| player.to_ascii_lowercase().starts_with(&prefix));
                let first = matches
                    .next()
                    .ok_or("No compatible media player matches that name.")?;
                if matches.next().is_some() {
                    return Err("More than one media player matches. Use the current player.");
                }
                first.to_owned()
            }
        }
        _ => source()?,
    };
    let option = format!("--player={player}");
    match action {
        MediaAction::Play(_, play) => {
            playerctl(&[&option, if play { "play" } else { "pause" }])?;
            Ok(if play {
                "Resumed playback."
            } else {
                "Paused playback."
            }
            .into())
        }
        MediaAction::Seek(seconds, relative) => {
            if !relative && seconds < 0 {
                return Err("Use a nonnegative playback position.");
            }
            let position = if relative {
                format!(
                    "{}{}",
                    seconds.unsigned_abs(),
                    if seconds < 0 { '-' } else { '+' }
                )
            } else {
                seconds.to_string()
            };
            playerctl(&[&option, "position", &position])?;
            Ok("Updated the playback position.".into())
        }
        MediaAction::Status(property) => {
            let property = property.to_lowercase();
            let value = match property.as_str() {
                "playback state" | "status" | "playback status" => playerctl(&[&option, "status"]),
                "title" | "track title" => playerctl(&[&option, "metadata", "title"]),
                "artist" => playerctl(&[&option, "metadata", "artist"]),
                "album" => playerctl(&[&option, "metadata", "album"]),
                _ => {
                    return Err(
                        "The player exposes track title, artist, album, and playback status.",
                    );
                }
            }?;
            if value.is_empty() {
                Err("The player did not provide that property.")
            } else {
                Ok(value)
            }
        }
        _ => Err("This media command is unavailable on Linux."),
    }
}

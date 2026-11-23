use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
};

use crate::{actions::command::MediaAction, desktop::windows::ComApartment};

const UNAVAILABLE: &str = "Windows did not expose this operation for the selected player.";

fn session(name: &str) -> Result<Session, &'static str> {
    let manager = Manager::RequestAsync()
        .and_then(|request| request.join())
        .map_err(|_| UNAVAILABLE)?;
    if name.eq_ignore_ascii_case("current") {
        return manager
            .GetCurrentSession()
            .map_err(|_| "There is no active player exposed by Windows.");
    }
    let sessions = manager.GetSessions().map_err(|_| UNAVAILABLE)?;
    let search = name.trim().to_lowercase();
    let search = match search.as_str() {
        "microsoft edge" | "edge" => "msedge",
        "media player" => "zunemusic",
        _ => &search,
    };
    let mut matches = sessions.into_iter().filter(|session| {
        session.SourceAppUserModelId().ok().is_some_and(|source| {
            let source = source.to_string().to_lowercase();
            source == search || (search.len() > 2 && source.contains(search))
        })
    });
    let first = matches
        .next()
        .ok_or("Windows has no active media session for that app.")?;
    if matches.next().is_some() {
        return Err("More than one media session matches. Use the current player.");
    }
    Ok(first)
}

pub fn source() -> Result<String, &'static str> {
    let _apartment = ComApartment::initialize()?;
    Ok(session("current")?
        .SourceAppUserModelId()
        .map_err(|_| UNAVAILABLE)?
        .to_string())
}

pub fn run(action: MediaAction) -> Result<String, &'static str> {
    let _apartment = ComApartment::initialize()?;
    let player = session(match &action {
        MediaAction::Play(name, _) => name,
        _ => "current",
    })?;
    match action {
        MediaAction::Play(_, play) => {
            let result = if play {
                player.TryPlayAsync()
            } else {
                player.TryPauseAsync()
            };
            applied(result.and_then(|result| result.join()))?;
            Ok(if play {
                "Resumed playback."
            } else {
                "Paused playback."
            }
            .into())
        }
        MediaAction::Seek(seconds, relative) => {
            let timeline = player.GetTimelineProperties().map_err(|_| UNAVAILABLE)?;
            let position = timeline.Position().map_err(|_| UNAVAILABLE)?.Duration;
            let min = timeline.StartTime().map_err(|_| UNAVAILABLE)?.Duration;
            let max = timeline.EndTime().map_err(|_| UNAVAILABLE)?.Duration;
            let requested = i64::from(seconds) * 10_000_000;
            let requested = if relative {
                position.saturating_add(requested)
            } else {
                requested
            };
            if max <= min || !(min..=max).contains(&requested) {
                return Err("The requested position is outside the exposed media duration.");
            }
            applied(
                player
                    .TryChangePlaybackPositionAsync(requested)
                    .and_then(|result| result.join()),
            )?;
            Ok(format!(
                "Moved playback to {} seconds.",
                requested / 10_000_000
            ))
        }
        MediaAction::Rate(percent) => {
            if !(25..=400).contains(&percent) {
                return Err("Choose a rate between 0.25× and 4×.");
            }
            applied(
                player
                    .TryChangePlaybackRateAsync(f64::from(percent) / 100.0)
                    .and_then(|result| result.join()),
            )?;
            Ok(format!(
                "Set playback rate to {}×.",
                f64::from(percent) / 100.0
            ))
        }
        MediaAction::Status(property) => {
            let property = property.to_lowercase();
            if matches!(
                property.as_str(),
                "playback state" | "status" | "playback status"
            ) {
                let running = player
                    .GetPlaybackInfo()
                    .and_then(|info| info.PlaybackStatus())
                    .map_err(|_| UNAVAILABLE)?;
                return Ok(format!("Playback status: {running:?}"));
            }
            let details = player
                .TryGetMediaPropertiesAsync()
                .and_then(|result| result.join())
                .map_err(|_| UNAVAILABLE)?;
            let result = match property.as_str() {
                "title" | "track title" => details.Title(),
                "artist" => details.Artist(),
                "album" => details.AlbumTitle(),
                _ => {
                    return Err(
                        "The player exposes track title, artist, album, and playback status.",
                    );
                }
            }
            .map_err(|_| UNAVAILABLE)?;
            if result.is_empty() {
                return Err("The player did not provide that property.");
            }
            Ok(result.to_string())
        }
        _ => Err(UNAVAILABLE),
    }
}

fn applied(result: windows::core::Result<bool>) -> Result<(), &'static str> {
    result
        .is_ok_and(|applied| applied)
        .then_some(())
        .ok_or(UNAVAILABLE)
}

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use reqwest::Url;
use serde_json::{Value, json};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, Microsoft::Web::WebView2::Win32::ICoreWebView2,
    take_pwstr,
};
use windows::core::{PCWSTR, PWSTR, w};

use super::{
    PlayRequest, Service,
    source::{SPOTIFY, Source, YOUTUBE},
};

const YOUTUBE_LOOKUP: &str = r#"
    if (location.origin !== 'https://www.youtube.com') return '';
    return document.querySelector('ytd-video-renderer a#video-title[href^="/watch?"], ytd-playlist-video-renderer a#video-title[href^="/watch?"]')?.href || '';
"#;

const SPOTIFY_LOOKUP: &str = r#"
    if (location.origin !== 'https://open.spotify.com') return '';
    const main = document.querySelector('main') || document;
    for (const kind of ['track', 'episode', 'album', 'playlist', 'show']) {
        for (const link of main.querySelectorAll('a[href*="/' + kind + '/"]')) {
            if (link.getClientRects().length) return link.href;
        }
    }
    return '';
"#;

const YOUTUBE_PLAY: &str = r#"
    if (location.origin !== 'https://www.youtube.com' || location.pathname !== '/watch') return '';
    const video = document.querySelector('#movie_player video');
    if (!video) return '';
    if (!video.paused && !video.ended && video.currentTime > 0) return 'playing';
    if (!start || !video.paused || video.error) return '';
    video.play().catch(() => {});
    return 'requested';
"#;

const SPOTIFY_PLAY: &str = r#"
    if (location.origin !== 'https://open.spotify.com' || !location.pathname.startsWith('/embed/')) return '';
    const pause = document.querySelector('button[aria-label="Pause"], button[title="Pause"]');
    if (pause?.getClientRects().length) return 'playing';
    const play = document.querySelector('button[aria-label="Play"], button[title="Play"]');
    if (!start || !play?.getClientRects().length || play.disabled) return '';
    play.click();
    return 'requested';
"#;

pub(super) struct Playback {
    pub service: Service,
    destination: String,
    looking_up: bool,
    navigated: bool,
    requested: bool,
    deadline: Instant,
    next_check: Instant,
    evaluation: Option<Rc<Cell<Option<String>>>>,
}

impl Playback {
    pub fn new(request: PlayRequest) -> Self {
        let (destination, looking_up) = destination(request.service, &request.query);
        let now = Instant::now();
        Self {
            service: request.service,
            destination,
            looking_up,
            navigated: false,
            requested: false,
            deadline: now + Duration::from_secs(15),
            next_check: now,
            evaluation: None,
        }
    }

    pub fn advance(&mut self, view: &ICoreWebView2) -> Option<Result<(), &'static str>> {
        let now = Instant::now();
        if now >= self.deadline {
            return Some(Err(if self.looking_up {
                "Amica couldn't find a playable result. You can choose one in the media card."
            } else {
                "Playback couldn't start automatically. Press Play in the media card."
            }));
        }
        if let Some(evaluation) = &self.evaluation {
            let result = evaluation.take()?;
            self.evaluation = None;
            if self.looking_up {
                if let Some(url) = player_url(self.service, &result) {
                    self.destination = url;
                    self.looking_up = false;
                    self.navigated = false;
                    self.deadline = now + Duration::from_secs(12);
                }
            } else {
                match result.as_str() {
                    "playing" => return Some(Ok(())),
                    "requested" => self.requested = true,
                    _ => {}
                }
            }
        }
        if !self.navigated {
            let url = wide(&self.destination);
            if unsafe { view.Navigate(PCWSTR(url.as_ptr())) }.is_err() {
                return Some(Err("Amica couldn't open the requested media."));
            }
            self.navigated = true;
            self.next_check = now + Duration::from_millis(350);
            return None;
        }
        if now < self.next_check {
            return None;
        }
        self.next_check = now + Duration::from_millis(300);
        let mut address = PWSTR::null();
        if unsafe { view.Source(&mut address) }.is_err() {
            return None;
        }
        let address = take_pwstr(address);
        if !matches_page(self.service, &self.destination, &address, self.looking_up) {
            return None;
        }
        let script = match (self.service, self.looking_up) {
            (Service::YouTube, true) => YOUTUBE_LOOKUP,
            (Service::Spotify, true) => SPOTIFY_LOOKUP,
            (Service::YouTube, false) => YOUTUBE_PLAY,
            (Service::Spotify, false) => SPOTIFY_PLAY,
        };
        let start = !self.looking_up && !self.requested;
        self.evaluation = evaluate(view, script, start);
        None
    }
}

fn evaluate(view: &ICoreWebView2, script: &str, start: bool) -> Option<Rc<Cell<Option<String>>>> {
    let output = Rc::new(Cell::new(None));
    let receiver = Rc::clone(&output);
    let args = wide(
        &json!({
            "expression": format!("((start) => {{ {script} }})({start})"),
            "userGesture": start,
            "returnByValue": true,
            "allowUnsafeEvalBlockedByCSP": false
        })
        .to_string(),
    );
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |status, text| {
            let result = status
                .ok()
                .and_then(|()| serde_json::from_str::<Value>(&text).ok())
                .and_then(|result| result["result"]["value"].as_str().map(str::to_owned))
                .unwrap_or_default();
            receiver.set(Some(result));
            Ok(())
        }));
    unsafe {
        view.CallDevToolsProtocolMethod(w!("Runtime.evaluate"), PCWSTR(args.as_ptr()), &handler)
    }
    .ok()?;
    Some(output)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn destination(service: Service, query: &str) -> (String, bool) {
    if let Some(url) = player_url(service, query) {
        return (url, false);
    }
    match service {
        Service::YouTube => {
            let query = if query.is_empty() {
                "instrumental music"
            } else {
                query
            };
            let mut url = Url::parse(YOUTUBE).unwrap().join("results").unwrap();
            url.query_pairs_mut().append_pair("search_query", query);
            (url.into(), true)
        }
        Service::Spotify => {
            let mut url = Url::parse(SPOTIFY).unwrap();
            if !query.is_empty() {
                url.path_segments_mut()
                    .unwrap()
                    .pop_if_empty()
                    .extend(["search", query]);
            }
            (url.into(), true)
        }
    }
}

fn player_url(service: Service, address: &str) -> Option<String> {
    match service {
        Service::YouTube => {
            let url = Url::parse(address).ok()?;
            Source::from_url(0, address).external?;
            let id = if url.host_str() == Some("youtu.be") {
                url.path_segments()?.next()?.to_owned()
            } else if let Some(id) = url.path().strip_prefix("/shorts/") {
                id.to_owned()
            } else if url.path() == "/watch" {
                url.query_pairs()
                    .find(|(key, _)| key == "v")?
                    .1
                    .into_owned()
            } else {
                return None;
            };
            if id.len() != 11
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return None;
            }
            Some(format!("{YOUTUBE}watch?v={id}"))
        }
        Service::Spotify => {
            let source = Source::from_url(1, address);
            let player = source.spotify_player?;
            if player.embedded {
                let external = source.external?;
                Some(format!(
                    "{SPOTIFY}embed/{}",
                    external.strip_prefix(SPOTIFY)?
                ))
            } else {
                Some(player.switch_to)
            }
        }
    }
}

fn matches_page(service: Service, destination: &str, address: &str, looking_up: bool) -> bool {
    if !looking_up {
        return player_url(service, address).as_deref() == Some(destination)
            && (service != Service::Spotify
                || address.starts_with("https://open.spotify.com/embed/"));
    }
    let expected = Url::parse(destination).unwrap();
    let Ok(actual) = Url::parse(address) else {
        return false;
    };
    if expected.origin() != actual.origin() {
        return false;
    }
    match service {
        Service::YouTube => {
            let search = |url: &Url| {
                url.query_pairs()
                    .find(|(key, _)| key == "search_query")
                    .map(|(_, value)| value.into_owned())
            };
            actual.path() == "/results" && search(&actual) == search(&expected)
        }
        Service::Spotify => {
            let path = actual.path();
            let path = path
                .strip_prefix("/intl-")
                .and_then(|path| path.find('/').map(|i| &path[i..]))
                .unwrap_or(path);
            path.trim_end_matches('/') == expected.path().trim_end_matches('/')
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_only_loads_matching_provider_media_and_encodes_search_terms() {
        assert_eq!(
            player_url(Service::YouTube, "https://youtu.be/jNQXAC9IVRw?t=3").as_deref(),
            Some("https://www.youtube.com/watch?v=jNQXAC9IVRw")
        );
        assert!(
            player_url(
                Service::YouTube,
                "https://www.youtube.com.example.org/watch?v=jNQXAC9IVRw"
            )
            .is_none()
        );
        let (spotify, search) = destination(Service::Spotify, "A/B + C");
        assert!(search);
        assert_eq!(spotify, "https://open.spotify.com/search/A%2FB%20+%20C");
        assert!(matches_page(
            Service::Spotify,
            &spotify,
            "https://open.spotify.com/intl-en/search/A%2FB%20+%20C",
            true
        ));
        assert!(!matches_page(
            Service::Spotify,
            &spotify,
            "https://open.spotify.com/search/unrelated",
            true
        ));
    }
}

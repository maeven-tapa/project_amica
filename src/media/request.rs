#[cfg(windows)]
use crate::Panel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    YouTube,
    Spotify,
}

impl Service {
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "youtube" => Some(Self::YouTube),
            "spotify" => Some(Self::Spotify),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::YouTube => "youtube",
            Self::Spotify => "spotify",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::YouTube => "YouTube",
            Self::Spotify => "Spotify",
        }
    }

    #[cfg(windows)]
    pub fn panel(self) -> Panel {
        match self {
            Self::YouTube => Panel::Youtube,
            Self::Spotify => Panel::Spotify,
        }
    }

    #[cfg(windows)]
    pub(super) fn index(self) -> usize {
        match self {
            Self::YouTube => 0,
            Self::Spotify => 1,
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn browser_page(self, query: &str) -> reqwest::Url {
        if let Ok(url) = reqwest::Url::parse(query)
            && url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && match self {
                Self::YouTube => matches!(
                    url.host_str(),
                    Some("www.youtube.com" | "youtube.com" | "m.youtube.com" | "youtu.be")
                ),
                Self::Spotify => url.host_str() == Some("open.spotify.com"),
            }
        {
            return url;
        }
        let mut url = reqwest::Url::parse(match self {
            Self::YouTube if query.is_empty() => "https://www.youtube.com/",
            Self::YouTube => "https://www.youtube.com/results",
            Self::Spotify if query.is_empty() => "https://open.spotify.com/",
            Self::Spotify => "https://open.spotify.com/search/",
        })
        .unwrap();
        if !query.is_empty() {
            match self {
                Self::YouTube => {
                    url.query_pairs_mut().append_pair("search_query", query);
                }
                Self::Spotify => {
                    url.path_segments_mut().unwrap().pop_if_empty().push(query);
                }
            }
        }
        url
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayRequest {
    pub service: Service,
    pub query: String,
}

impl PlayRequest {
    pub fn new(service: Service, query: &str) -> Result<Self, &'static str> {
        let query = query.trim();
        if query.chars().count() > 240 || query.chars().any(char::is_control) {
            return Err("Keep the media title or link under 240 characters.");
        }
        let normalized = query.to_lowercase();
        let query = match normalized.as_str() {
            "something" | "something random" | "anything" | "kahit ano" => "",
            _ => query,
        };
        Ok(Self {
            service,
            query: query.to_owned(),
        })
    }
}

pub fn spoken_request(input: &str) -> Option<PlayRequest> {
    let trimmed = input.trim().trim_end_matches(['.', '?', '!', ' ']);
    let mut command = trimmed;
    for prefix in ["hey amica ", "amica ", "please "] {
        if command.to_ascii_lowercase().starts_with(prefix) {
            command = command[prefix.len()..].trim_start_matches([',', ' ']);
        }
    }
    let lowercase = command.to_ascii_lowercase();
    if lowercase.ends_with(" please") {
        command = command[..command.len() - " please".len()].trim_end_matches([',', ' ']);
    }
    let mut words = command.split_whitespace();
    if !matches!(
        words.next()?.to_ascii_lowercase().as_str(),
        "play" | "patugtog" | "ipatugtog"
    ) {
        return None;
    }
    let platform = words.next_back()?;
    let service = Service::parse(&platform.to_ascii_lowercase())?;
    let separator = words.next_back()?;
    if !matches!(separator.to_ascii_lowercase().as_str(), "on" | "in" | "sa") {
        return None;
    }
    let query = words.collect::<Vec<_>>().join(" ");
    let query = if separator.eq_ignore_ascii_case("sa") {
        query.strip_prefix("ng ").unwrap_or(&query)
    } else {
        &query
    };
    PlayRequest::new(service, query).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_commands_preserve_titles_and_require_a_provider() {
        assert_eq!(
            spoken_request("Play something in Spotify").unwrap(),
            PlayRequest::new(Service::Spotify, "").unwrap()
        );
        assert_eq!(
            spoken_request("Please play Araw-Araw by Ben&Ben on YouTube.")
                .unwrap()
                .query,
            "Araw-Araw by Ben&Ben"
        );
        assert_eq!(
            spoken_request("Patugtog ng Buwan sa Spotify")
                .unwrap()
                .service,
            Service::Spotify
        );
        assert!(spoken_request("How do I play this on YouTube?").is_none());
        assert!(spoken_request("Play Buwan on an unknown site").is_none());
    }
}

use reqwest::Url;

pub(super) const YOUTUBE: &str = "https://www.youtube.com/";
pub(super) const SPOTIFY: &str = "https://open.spotify.com/";

#[derive(Default)]
pub(super) struct Source {
    pub external: Option<String>,
    pub spotify_player: Option<SpotifyPlayer>,
}

pub(super) struct SpotifyPlayer {
    pub embedded: bool,
    pub switch_to: String,
}

impl Source {
    pub fn from_url(index: usize, input: &str) -> Self {
        let Ok(url) = Url::parse(input) else {
            return Self::default();
        };
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Self::default();
        }
        match (index, url.host_str()) {
            (0, Some("www.youtube.com" | "youtube.com" | "m.youtube.com" | "youtu.be")) => Self {
                external: Some(url.into()),
                spotify_player: None,
            },
            (1, Some("open.spotify.com")) => Self::spotify(url),
            _ => Self::default(),
        }
    }

    fn spotify(url: Url) -> Self {
        let Some(mut path) = url.path_segments() else {
            return Self::default();
        };
        let mut kind = path.next().unwrap_or_default();
        if kind.starts_with("intl-") {
            kind = path.next().unwrap_or_default();
        }
        let embedded = kind == "embed";
        if embedded {
            kind = path.next().unwrap_or_default();
        }
        let id = path.next().unwrap_or_default();
        if !matches!(
            kind,
            "track" | "episode" | "show" | "album" | "playlist" | "artist"
        ) || id.len() != 22
            || !id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Self {
                external: Some(url.into()),
                spotify_player: None,
            };
        }

        let web_player = format!("{SPOTIFY}{kind}/{id}");
        let switch_to = if embedded {
            web_player.clone()
        } else {
            format!("{SPOTIFY}embed/{kind}/{id}")
        };
        Self {
            external: Some(web_player),
            spotify_player: Some(SpotifyPlayer {
                embedded,
                switch_to,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_switches_between_the_selected_album_and_its_official_embed() {
        let item = "album/4aawyAB9vmqN3uQ7FjRGTy";
        let source = Source::from_url(1, &format!("{SPOTIFY}intl-en/{item}?si=example"));
        let player = source.spotify_player.unwrap();
        assert!(!player.embedded);
        let embedded = Source::from_url(1, &player.switch_to);
        assert_eq!(embedded.external.as_deref(), source.external.as_deref());
        let player = embedded.spotify_player.unwrap();
        assert!(player.embedded);
        assert_eq!(player.switch_to, format!("{SPOTIFY}{item}"));
        assert!(
            Source::from_url(1, "https://open.spotify.com.example.com/")
                .external
                .is_none()
        );
    }
}

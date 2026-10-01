//! What a media session is showing, as plain values the page and the HUD
//! format without touching the system.

use std::{path::PathBuf, time::Duration};

/// One application playing media, as the system's media controls see it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    /// The application's user model id, such as `Spotify.exe`.
    pub app_id: String,
    /// Where the session sits in the system's list; with `app_id` it tells
    /// two sessions of one application apart.
    pub ix: usize,
    pub is_current: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub status: Status,
    pub position: Option<Duration>,
    pub duration: Option<Duration>,
    /// The cover art, saved to the cache.
    pub artwork: Option<PathBuf>,
}

impl Session {
    /// A stable item id for the page.
    pub fn key(&self) -> String {
        format!("media/{}/{}", self.app_id, self.ix)
    }

    /// The application's name as a person knows it.
    pub fn app_name(&self) -> String {
        app_name(&self.app_id)
    }

    /// The title, or the application when the track has none.
    pub fn display_title(&self) -> String {
        match self.title.trim() {
            "" => self.app_name(),
            title => title.to_owned(),
        }
    }

    /// Artist and album, as far as they are known.
    pub fn byline(&self) -> String {
        [self.artist.trim(), self.album.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// `1:23 / 3:45`, or `None` when the application does not say.
    pub fn progress(&self) -> Option<String> {
        let duration = self.duration.filter(|duration| !duration.is_zero())?;
        let position = self.position.unwrap_or_default().min(duration);
        Some(format!(
            "{} / {}",
            format_clock(position),
            format_clock(duration)
        ))
    }

    /// What the HUD says after a command: the track, and the artist when
    /// known.
    pub fn hud_text(&self) -> String {
        let title = self.display_title();
        let text = match self.artist.trim() {
            "" => title,
            artist => format!("{title} — {artist}"),
        };
        match self.status {
            Status::Paused | Status::Stopped => format!("Paused: {text}"),
            _ => text,
        }
    }
}

/// The playback status of a session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Status {
    #[default]
    Closed,
    Opened,
    Changing,
    Stopped,
    Playing,
    Paused,
}

impl Status {
    /// Maps `GlobalSystemMediaTransportControlsSessionPlaybackStatus`.
    pub fn from_raw(value: i32) -> Self {
        match value {
            1 => Self::Opened,
            2 => Self::Changing,
            3 => Self::Stopped,
            4 => Self::Playing,
            5 => Self::Paused,
            _ => Self::Closed,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Closed => "Closed",
            Self::Opened => "Ready",
            Self::Changing => "Loading",
            Self::Stopped => "Stopped",
            Self::Playing => "Playing",
            Self::Paused => "Paused",
        }
    }

    pub fn is_playing(self) -> bool {
        self == Self::Playing
    }
}

/// A command sent to a session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
}

impl Control {
    pub fn title(self) -> &'static str {
        match self {
            Self::PlayPause => "Play/Pause",
            Self::Next => "Next Track",
            Self::Previous => "Previous Track",
        }
    }

    /// The failure toast's title.
    pub fn failure(self) -> &'static str {
        match self {
            Self::PlayPause => "Couldn’t play or pause",
            Self::Next => "Couldn’t skip to the next track",
            Self::Previous => "Couldn’t go to the previous track",
        }
    }
}

/// The friendly name of an application user model id: a known application's
/// name, else the packaged application's or executable's own name.
pub fn app_name(app_id: &str) -> String {
    const KNOWN: &[(&str, &str)] = &[
        ("msedge", "Microsoft Edge"),
        ("chrome", "Google Chrome"),
        ("308046b0af4a39cb", "Firefox"),
        ("firefox", "Firefox"),
        ("spotify", "Spotify"),
        ("spotifyab.spotifymusic", "Spotify"),
        ("microsoft.zunemusic", "Media Player"),
        ("microsoft.zunevideo", "Movies & TV"),
        ("vlc", "VLC"),
        ("brave", "Brave"),
        ("opera", "Opera"),
        ("discord", "Discord"),
        ("foobar2000", "foobar2000"),
        ("applemusic", "Apple Music"),
        ("appleinc.applemusicwin", "Apple Music"),
        ("itunes", "iTunes"),
    ];
    let name = match app_id.contains(['\\', '/']) {
        // An executable's path.
        true => app_id.rsplit(['\\', '/']).next().unwrap_or(app_id),
        // `Package_publisherhash!App`: the package names the application.
        false => {
            let package = app_id.split('!').next().unwrap_or(app_id);
            package.split('_').next().unwrap_or(package)
        }
    };
    let name = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    let lower = name.to_ascii_lowercase();
    if let Some((_, known)) = KNOWN.iter().find(|(id, _)| *id == lower) {
        return (*known).to_owned();
    }
    // `Publisher.Application`: the last part names the application.
    let last = match name.contains(' ') {
        true => name,
        false => name.rsplit('.').next().unwrap_or(name),
    };
    match last.trim() {
        "" => app_id.to_owned(),
        last => last.to_owned(),
    }
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn format_clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}"),
        hours => format!("{hours}:{minutes:02}:{seconds:02}"),
    }
}

/// Where playback is now: the reported position plus the time since it was
/// reported, while playing. Times are in 100-nanosecond ticks, as WinRT's
/// `TimeSpan` and `DateTime` count them.
pub fn current_position(
    position: i64,
    updated_at: i64,
    now: i64,
    duration: i64,
    is_playing: bool,
) -> Duration {
    let elapsed = match is_playing && updated_at > 0 {
        true => (now - updated_at).max(0),
        false => 0,
    };
    let mut ticks = position.max(0) + elapsed;
    if duration > 0 {
        ticks = ticks.min(duration);
    }
    ticks_to_duration(ticks)
}

pub fn ticks_to_duration(ticks: i64) -> Duration {
    Duration::from_nanos(ticks.max(0) as u64 * 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_name() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("MSEdge"), "Microsoft Edge");
        assert_eq!(app_name("Chrome"), "Google Chrome");
        assert_eq!(app_name("308046B0AF4A39CB"), "Firefox");
        assert_eq!(
            app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "Media Player"
        );
        assert_eq!(
            app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"),
            "Spotify"
        );
        assert_eq!(
            app_name(r"C:\Program Files\Foo\Bar Player.exe"),
            "Bar Player"
        );
        assert_eq!(app_name("Contoso.Tunes_abc!App"), "Tunes");
        assert_eq!(app_name(""), "");
    }

    #[test]
    fn test_format_clock() {
        assert_eq!(format_clock(Duration::from_secs(5)), "0:05");
        assert_eq!(format_clock(Duration::from_secs(225)), "3:45");
        assert_eq!(format_clock(Duration::from_secs(3_725)), "1:02:05");
    }

    #[test]
    fn test_current_position() {
        let second = 10_000_000;
        assert_eq!(
            current_position(10 * second, 100 * second, 103 * second, 0, true),
            Duration::from_secs(13)
        );
        assert_eq!(
            current_position(10 * second, 100 * second, 103 * second, 0, false),
            Duration::from_secs(10),
            "a paused position stays put"
        );
        assert_eq!(
            current_position(10 * second, 100 * second, 200 * second, 12 * second, true),
            Duration::from_secs(12),
            "never past the end"
        );
    }

    #[test]
    fn test_session_text() {
        let mut session = Session {
            app_id: "Spotify.exe".into(),
            title: "Teardrop".into(),
            artist: "Massive Attack".into(),
            album: "Mezzanine".into(),
            status: Status::Playing,
            position: Some(Duration::from_secs(83)),
            duration: Some(Duration::from_secs(330)),
            ..Session::default()
        };
        assert_eq!(session.byline(), "Massive Attack · Mezzanine");
        assert_eq!(session.progress().as_deref(), Some("1:23 / 5:30"));
        assert_eq!(session.hud_text(), "Teardrop — Massive Attack");
        session.status = Status::Paused;
        assert_eq!(session.hud_text(), "Paused: Teardrop — Massive Attack");
        session.title.clear();
        session.artist.clear();
        session.duration = None;
        assert_eq!(session.display_title(), "Spotify");
        assert_eq!(session.byline(), "Mezzanine");
        assert_eq!(session.progress(), None);
        assert_eq!(Status::from_raw(4), Status::Playing);
        assert_eq!(Status::from_raw(42), Status::Closed);
    }
}

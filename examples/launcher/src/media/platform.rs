//! The system's media controls on Windows
//! (`Windows.Media.Control`). Every function blocks on WinRT operations:
//! call them on a background thread.

use std::{
    hash::{DefaultHasher, Hash as _, Hasher as _},
    path::PathBuf,
    time::{Duration, SystemTime},
};

use anyhow::{Context as _, Result, anyhow};
use windows::{
    Foundation::IAsyncOperation,
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as MediaSession,
        GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    },
    Storage::Streams::DataReader,
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
};

use super::session::{Control, Session, Status, current_position, ticks_to_duration};

/// Cover art larger than this is not saved.
const LARGEST_ARTWORK: u64 = 8 * 1024 * 1024;
/// Saved cover art older than this is removed when new art is saved.
const ARTWORK_LIFETIME: Duration = Duration::from_secs(7 * 24 * 3600);

fn manager() -> Result<SessionManager> {
    // Harmless when the thread is already initialized.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    Ok(SessionManager::RequestAsync()?.get()?)
}

/// Every media session, the current one first.
pub fn sessions() -> Result<Vec<Session>> {
    let manager = manager()?;
    let current = manager
        .GetCurrentSession()
        .ok()
        .and_then(|session| session.SourceAppUserModelId().ok())
        .map(|id| id.to_string_lossy());
    let mut sessions = Vec::new();
    for (ix, session) in manager.GetSessions()?.into_iter().enumerate() {
        match read(&session, ix) {
            Ok(mut read) => {
                read.is_current = current.as_deref() == Some(read.app_id.as_str())
                    && !sessions.iter().any(|other: &Session| other.is_current);
                sessions.push(read);
            }
            Err(error) => tracing::debug!("cannot read media session {ix}: {error}"),
        }
    }
    sessions.sort_by_key(|session| !session.is_current);
    Ok(sessions)
}

fn read(session: &MediaSession, ix: usize) -> Result<Session> {
    let app_id = session.SourceAppUserModelId()?.to_string_lossy();
    let status = session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus())
        .map(|status| Status::from_raw(status.0))
        .unwrap_or_default();
    let mut read = Session {
        app_id,
        ix,
        status,
        ..Session::default()
    };
    if let Ok(properties) = session.TryGetMediaPropertiesAsync().and_then(|op| op.get()) {
        read.title = properties
            .Title()
            .map(|text| text.to_string_lossy())
            .unwrap_or_default();
        read.artist = properties
            .Artist()
            .map(|text| text.to_string_lossy())
            .unwrap_or_default();
        if read.artist.trim().is_empty() {
            read.artist = properties
                .AlbumArtist()
                .map(|text| text.to_string_lossy())
                .unwrap_or_default();
        }
        read.album = properties
            .AlbumTitle()
            .map(|text| text.to_string_lossy())
            .unwrap_or_default();
        read.artwork = artwork(&properties, &read)
            .inspect_err(|error| tracing::debug!("no cover art for {}: {error:#}", read.app_id))
            .ok()
            .flatten();
    }
    if let Ok(timeline) = session.GetTimelineProperties() {
        let ticks = |time: windows::core::Result<windows::Foundation::TimeSpan>| {
            time.map(|time| time.Duration).unwrap_or_default()
        };
        let duration = ticks(timeline.EndTime()) - ticks(timeline.StartTime());
        if duration > 0 {
            let updated_at = timeline
                .LastUpdatedTime()
                .map(|time| time.UniversalTime)
                .unwrap_or_default();
            let position = ticks(timeline.Position()) - ticks(timeline.StartTime());
            read.duration = Some(ticks_to_duration(duration));
            read.position = Some(current_position(
                position,
                updated_at,
                now_ticks(),
                duration,
                status.is_playing(),
            ));
        }
    }
    Ok(read)
}

/// Now, as a WinRT `DateTime` counts: 100-nanosecond ticks since 1601.
fn now_ticks() -> i64 {
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH_TICKS + (since_epoch.as_nanos() / 100) as i64
}

fn artwork_directory() -> Option<PathBuf> {
    dirs::cache_dir().map(|directory| directory.join("gpui-kit-launcher").join("artwork"))
}

/// Saves the session's cover art to the cache, once per track.
fn artwork(
    properties: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
    session: &Session,
) -> Result<Option<PathBuf>> {
    let Some(directory) = artwork_directory() else {
        return Ok(None);
    };
    let mut hasher = DefaultHasher::new();
    (
        &session.app_id,
        &session.title,
        &session.artist,
        &session.album,
    )
        .hash(&mut hasher);
    let stem = format!("{:016x}", hasher.finish());
    for extension in ["png", "jpg"] {
        let path = directory.join(format!("{stem}.{extension}"));
        if path.exists() {
            return Ok(Some(path));
        }
    }
    let Ok(reference) = properties.Thumbnail() else {
        return Ok(None);
    };
    let stream = reference.OpenReadAsync()?.get()?;
    let size = stream.Size()?;
    if size == 0 || size > LARGEST_ARTWORK {
        return Ok(None);
    }
    let extension = match stream.ContentType()?.to_string_lossy().as_str() {
        "image/png" => "png",
        _ => "jpg",
    };
    let reader = DataReader::CreateDataReader(&stream)?;
    reader.LoadAsync(size as u32)?.get()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes)?;
    std::fs::create_dir_all(&directory)?;
    remove_old_artwork(&directory);
    let path = directory.join(format!("{stem}.{extension}"));
    std::fs::write(&path, bytes).context("cannot save the cover art")?;
    Ok(Some(path))
}

fn remove_old_artwork(directory: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > ARTWORK_LIFETIME);
        if old {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// Sends `control` to the session `app_id` at `ix`, or to the current session
/// when `target` is `None`. Fails when there is no such session or it
/// declines.
pub fn send(control: Control, target: Option<(&str, usize)>) -> Result<()> {
    let manager = manager()?;
    let session = match target {
        None => manager
            .GetCurrentSession()
            .map_err(|_| anyhow!("Nothing is playing."))?,
        Some((app_id, ix)) => {
            let sessions: Vec<MediaSession> = manager.GetSessions()?.into_iter().collect();
            let matches = |session: &MediaSession| {
                session
                    .SourceAppUserModelId()
                    .is_ok_and(|id| id.to_string_lossy() == app_id)
            };
            sessions
                .get(ix)
                .filter(|session| matches(session))
                .or_else(|| sessions.iter().find(|session| matches(session)))
                .cloned()
                .ok_or_else(|| anyhow!("The application stopped playing."))?
        }
    };
    let operation: IAsyncOperation<bool> = match control {
        Control::PlayPause => session.TryTogglePlayPauseAsync()?,
        Control::Next => session.TrySkipNextAsync()?,
        Control::Previous => session.TrySkipPreviousAsync()?,
    };
    match operation.get()? {
        true => Ok(()),
        false => Err(anyhow!("The application didn’t accept it.")),
    }
}

/// The current session after a command, for the HUD.
pub fn current() -> Result<Option<Session>> {
    let manager = manager()?;
    let Ok(session) = manager.GetCurrentSession() else {
        return Ok(None);
    };
    let mut read = read(&session, 0)?;
    read.is_current = true;
    Ok(Some(read))
}

#[cfg(test)]
mod tests {
    /// Lists the media sessions without controlling any.
    #[test]
    #[ignore = "reads the machine's media sessions"]
    fn test_lists_sessions() {
        for session in super::sessions().unwrap() {
            println!(
                "{} ({}) {:?}: {} — {} [{:?} / {:?}] art {:?}",
                session.app_name(),
                session.app_id,
                session.status,
                session.title,
                session.byline(),
                session.position,
                session.duration,
                session.artwork
            );
        }
    }
}

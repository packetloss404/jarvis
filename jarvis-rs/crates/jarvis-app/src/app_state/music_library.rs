//! Local music library: scan directories, read tags, cache results.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Supported audio file extensions.
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "ogg", "m4a", "wav", "opus", "wma", "aac"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub duration: Option<f64>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub has_cover: bool,
    pub file_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicLibrary {
    pub music_dir: String,
    pub tracks: Vec<Track>,
    pub scanned_at: Option<String>,
}

/// Generate a stable ID from a file path.
fn path_id(path: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

/// Read audio tags from a single file using lofty.
pub fn read_tags(path: &Path) -> Option<Track> {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::tag::Accessor;

    let metadata = std::fs::metadata(path).ok()?;
    let path_str = path.to_string_lossy().to_string();

    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let props = tagged.properties();

    let (title, artist, album, album_artist, track_number, disc_number, genre, year) =
        if let Some(t) = tag {
            (
                t.title().map(|s| s.to_string()),
                t.artist().map(|s| s.to_string()),
                t.album().map(|s| s.to_string()),
                t.get_string(&lofty::tag::ItemKey::AlbumArtist)
                    .map(|s| s.to_string()),
                t.track(),
                t.disk(),
                t.genre().map(|s| s.to_string()),
                t.year(),
            )
        } else {
            (None, None, None, None, None, None, None, None)
        };

    let has_cover = tag
        .map(|t| !t.pictures().is_empty())
        .unwrap_or(false);

    let duration = props.duration().as_secs_f64();
    let duration = if duration > 0.0 { Some(duration) } else { None };

    Some(Track {
        id: path_id(&path_str),
        path: path_str,
        title,
        artist,
        album,
        album_artist,
        track_number,
        disc_number,
        duration,
        genre,
        year,
        has_cover,
        file_size: metadata.len(),
    })
}

/// Scan a directory recursively for audio files and read their tags.
pub fn scan_directory(music_dir: &Path) -> MusicLibrary {
    let mut tracks = Vec::new();
    scan_recursive(music_dir, &mut tracks);
    tracks.sort_by(|a, b| {
        let artist_cmp = a
            .artist
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .cmp(&b.artist.as_deref().unwrap_or("").to_lowercase());
        if artist_cmp != std::cmp::Ordering::Equal {
            return artist_cmp;
        }
        let album_cmp = a
            .album
            .as_deref()
            .unwrap_or("")
            .cmp(&b.album.as_deref().unwrap_or(""));
        if album_cmp != std::cmp::Ordering::Equal {
            return album_cmp;
        }
        a.track_number.cmp(&b.track_number)
    });

    let scanned_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| {
            let secs = d.as_secs();
            format!("{secs}")
        });

    MusicLibrary {
        music_dir: music_dir.to_string_lossy().to_string(),
        tracks,
        scanned_at,
    }
}

fn scan_recursive(dir: &Path, tracks: &mut Vec<Track>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_recursive(&path, tracks);
        } else if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                    match read_tags(&path) {
                        Some(track) => tracks.push(track),
                        None => {
                            tracing::debug!(path = %path.display(), "Failed to read tags");
                        }
                    }
                }
            }
        }
    }
}

/// Default music directory.
pub fn default_music_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Music")))
        .unwrap_or_else(|| PathBuf::from("Music"))
}

/// Path for the library cache file.
pub fn cache_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("jarvis").join("music-library.json"))
}

/// Load cached library from disk.
pub fn load_cached_library() -> Option<MusicLibrary> {
    let path = cache_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Save library to disk cache.
pub fn save_library_cache(library: &MusicLibrary) {
    if let Some(path) = cache_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(library) {
            let _ = std::fs::write(&path, json);
        }
    }
}


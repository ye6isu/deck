use lofty::{read_from_path, AudioFile, TaggedFileExt, Accessor};
use std::path::Path;

#[derive(Clone, Default)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: f64,
    pub duration_str: String,
    pub cover_image: Option<Vec<u8>>,
}

pub fn extract_metadata(path: &Path) -> TrackMetadata {
    extract_metadata_internal(path, true)
}

pub fn extract_metadata_fast(path: &Path) -> TrackMetadata {
    extract_metadata_internal(path, false)
}

fn extract_metadata_internal(path: &Path, include_cover: bool) -> TrackMetadata {
    let fallback_stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let mut meta = TrackMetadata {
        title: fallback_stem.clone(),
        artist: "Unknown Artist".to_string(),
        album: "Unknown Album".to_string(),
        duration_secs: 0.0,
        duration_str: "--:--".to_string(),
        cover_image: None,
    };

    if let Ok(tagged_file) = read_from_path(path) {
        let dur = tagged_file.properties().duration();
        meta.duration_secs = dur.as_secs_f64();
        let total_secs = dur.as_secs();
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        meta.duration_str = format!("{:02}:{:02}", mins, secs);

        if let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) {
            if let Some(t) = tag.title() {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    meta.title = trimmed.to_string();
                }
            }
            if let Some(a) = tag.artist() {
                let trimmed = a.trim();
                if !trimmed.is_empty() {
                    meta.artist = trimmed.to_string();
                }
            }
            if let Some(al) = tag.album() {
                let trimmed = al.trim();
                if !trimmed.is_empty() {
                    meta.album = trimmed.to_string();
                }
            }
            if include_cover {
                if let Some(pic) = tag.pictures().first() {
                    meta.cover_image = Some(pic.data().to_vec());
                }
            }
        }
    }

    if meta.title.trim().is_empty() {
        meta.title = fallback_stem;
    }

    meta
}

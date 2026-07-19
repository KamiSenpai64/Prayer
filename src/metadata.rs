use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use walkdir::WalkDir;
use mp4ameta::Tag;

#[derive(Clone)]
pub struct TrackMetadata {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub track_number: String,
}

#[derive(Clone)]
pub struct AlbumGroup {
    pub name: String,
    pub tracks: Vec<TrackMetadata>,
}

pub struct MetadataState {
    pub tmp_dir: PathBuf,
    pub albums: Vec<AlbumGroup>,
    pub selected_album: usize,
    pub selected_track: usize,
    pub is_loading: Arc<Mutex<bool>>,
    pub status: String,
}

impl MetadataState {
    pub fn new(tmp_dir: PathBuf) -> Self {
        let mut state = Self {
            tmp_dir,
            albums: Vec::new(),
            selected_album: 0,
            selected_track: 0,
            is_loading: Arc::new(Mutex::new(false)),
            status: "Idle. Press 'r' to rescan tmp directory.".to_string(),
        };
        state.scan_directory();
        state
    }

    pub fn scan_directory(&mut self) {
        *self.is_loading.lock().unwrap() = true;
        self.status = "Scanning directory...".to_string();
        
        let mut parsed_tracks = Vec::new();
        
        if self.tmp_dir.exists() {
            for entry in WalkDir::new(&self.tmp_dir).into_iter().filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("m4a") {
                    if let Ok(tag) = Tag::read_from_path(path) {
                        let title = tag.title().unwrap_or("Unknown Title").to_string();
                        let artist = tag.artist().unwrap_or("Unknown Artist").to_string();
                        let album = tag.album().unwrap_or("Unknown Album").to_string();
                        let year = tag.year().map(|y| y.to_string()).unwrap_or_else(|| "".to_string());
                        let track_number = tag.track_number().map(|t| t.to_string()).unwrap_or_else(|| "".to_string());
                        
                        parsed_tracks.push(TrackMetadata {
                            path: path.to_path_buf(),
                            title,
                            artist,
                            album,
                            year,
                            track_number,
                        });
                    }
                }
            }
        }
        
        // Group by album
        let mut groups: std::collections::HashMap<String, Vec<TrackMetadata>> = std::collections::HashMap::new();
        for track in parsed_tracks {
            let key = format!("{} - {}", track.artist, track.album);
            groups.entry(key).or_default().push(track);
        }
        
        let mut albums: Vec<AlbumGroup> = groups.into_iter().map(|(name, mut tracks)| {
            tracks.sort_by_key(|t| t.track_number.parse::<u32>().unwrap_or(0));
            AlbumGroup { name, tracks }
        }).collect();
        albums.sort_by(|a, b| a.name.cmp(&b.name));
        
        self.albums = albums;
        *self.is_loading.lock().unwrap() = false;
        self.status = format!("Scanned {} albums. Press 'e' to edit selected track, 'E' to bulk edit album.", self.albums.len());
        self.selected_album = 0;
        self.selected_track = 0;
    }

    pub fn save_track_metadata(path: &PathBuf, title: &str, artist: &str, album: &str, year: &str, track_number: &str) -> bool {
        if let Ok(mut tag) = Tag::read_from_path(path) {
            tag.set_title(title);
            tag.set_artist(artist);
            tag.set_album(album);
            tag.set_year(year.trim());
            if let Ok(t) = track_number.trim().parse::<u16>() { tag.set_track_number(t); }
            return tag.write_to_path(path).is_ok();
        }
        false
    }

    pub fn fetch_lyrics(&mut self, album_index: usize) {
        if album_index >= self.albums.len() { return; }
        let album = self.albums[album_index].clone();
        
        *self.is_loading.lock().unwrap() = true;
        self.status = format!("Fetching lyrics for {} tracks...", album.tracks.len());
        
        let is_loading_arc = self.is_loading.clone();
        
        thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            for track in album.tracks {
                let url = format!(
                    "https://lrclib.net/api/search?track_name={}&artist_name={}&album_name={}",
                    urlencoding::encode(&track.title),
                    urlencoding::encode(&track.artist),
                    urlencoding::encode(&track.album)
                );
                
                if let Ok(resp) = client.get(&url).send() {
                    if let Ok(results) = resp.json::<Vec<serde_json::Value>>() {
                        if let Some(first) = results.first() {
                            if let Some(synced) = first.get("syncedLyrics").and_then(|v| v.as_str()) {
                                if !synced.trim().is_empty() {
                                    let lrc_path = track.path.with_extension("lrc");
                                    let _ = std::fs::write(lrc_path, synced);
                                }
                            }
                        }
                    }
                }
            }
            *is_loading_arc.lock().unwrap() = false;
        });
    }

    pub fn move_album_to_library(&mut self, album_index: usize, dest_dir: PathBuf) {
        if album_index >= self.albums.len() { return; }
        let album = self.albums.remove(album_index);
        
        if !dest_dir.exists() {
            let _ = std::fs::create_dir_all(&dest_dir);
        }
        
        for track in album.tracks {
            let artist_dir = dest_dir.join(&track.artist);
            let album_dir_name = if track.year.is_empty() { track.album.clone() } else { format!("({}) - {}", track.year, track.album) };
            let album_dir = artist_dir.join(&album_dir_name);
            
            let _ = std::fs::create_dir_all(&album_dir);
            
            let dest_path = album_dir.join(track.path.file_name().unwrap());
            let _ = std::fs::rename(&track.path, &dest_path);
            
            let lrc_path = track.path.with_extension("lrc");
            if lrc_path.exists() {
                let dest_lrc = dest_path.with_extension("lrc");
                let _ = std::fs::rename(&lrc_path, &dest_lrc);
            }
        }
        
        if self.selected_album >= self.albums.len() {
            self.selected_album = self.albums.len().saturating_sub(1);
            self.selected_track = 0;
        }
        self.status = format!("Moved {} to library.", album.name);
    }
}

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
                    let mut tag_title = String::new();
                    let mut tag_artist = String::new();
                    let mut tag_album = String::new();
                    let mut tag_year = String::new();
                    let mut tag_track_number = String::new();
                    
                    if let Ok(tag) = Tag::read_from_path(path) {
                        tag_title = tag.title().unwrap_or("").to_string();
                        tag_artist = tag.artist().unwrap_or("").to_string();
                        tag_album = tag.album().unwrap_or("").to_string();
                        tag_year = tag.year().map(|y| y.to_string()).unwrap_or_else(|| "".to_string());
                        tag_track_number = tag.track_number().map(|t| t.to_string()).unwrap_or_else(|| "".to_string());
                    } else {
                        // File might be a corrupted yt-dlp container, attempt a quick remux to fix it
                        let temp_path = path.with_extension("m4a.tmp");
                        if let Ok(mut child) = std::process::Command::new("ffmpeg")
                            .arg("-y").arg("-i").arg(path)
                            .arg("-c").arg("copy")
                            .arg(&temp_path)
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn() 
                        {
                            let _ = child.wait();
                            if temp_path.exists() && std::fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0) > 0 {
                                let _ = std::fs::rename(&temp_path, path);
                                // Try reading again after remuxing
                                if let Ok(tag) = Tag::read_from_path(path) {
                                    tag_title = tag.title().unwrap_or("").to_string();
                                    tag_artist = tag.artist().unwrap_or("").to_string();
                                    tag_album = tag.album().unwrap_or("").to_string();
                                    tag_year = tag.year().map(|y| y.to_string()).unwrap_or_else(|| "".to_string());
                                    tag_track_number = tag.track_number().map(|t| t.to_string()).unwrap_or_else(|| "".to_string());
                                }
                            } else {
                                let _ = std::fs::remove_file(&temp_path);
                            }
                        }
                    }
                    
                    if tag_artist.is_empty() || tag_artist == "Unknown Artist" {
                        if let Some(parent) = path.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()) {
                            let n = parent.to_string_lossy().into_owned();
                            if n != "tmp" { tag_artist = n; }
                        }
                    }
                    if tag_album.is_empty() || tag_album == "Unknown Album" {
                        if let Some(parent) = path.parent().and_then(|p| p.file_name()) {
                            let p = parent.to_string_lossy().into_owned();
                            if p.starts_with("(") && p.contains(") - ") {
                                let parts: Vec<&str> = p.splitn(2, ") - ").collect();
                                if parts.len() == 2 {
                                    tag_album = parts[1].to_string();
                                    if tag_year.is_empty() { tag_year = parts[0][1..].to_string(); }
                                }
                            } else {
                                tag_album = p;
                            }
                        }
                    }
                    if tag_title.is_empty() || tag_title == "Unknown Title" {
                        if let Some(file_name) = path.file_stem() {
                            let mut name = file_name.to_string_lossy().into_owned();
                            if name.contains(" - ") {
                                let parts: Vec<&str> = name.splitn(2, " - ").collect();
                                if parts[0].chars().all(|c| c.is_digit(10)) {
                                    if tag_track_number.is_empty() { tag_track_number = parts[0].to_string(); }
                                    name = parts[1].to_string();
                                }
                            }
                            tag_title = name;
                        }
                    }

                    parsed_tracks.push(TrackMetadata {
                        path: path.to_path_buf(),
                        title: if tag_title.is_empty() { "Unknown Title".to_string() } else { tag_title },
                        artist: if tag_artist.is_empty() { "Unknown Artist".to_string() } else { tag_artist },
                        album: if tag_album.is_empty() { "Unknown Album".to_string() } else { tag_album },
                        year: tag_year,
                        track_number: tag_track_number,
                    });
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
        let mut tag = Tag::read_from_path(path).unwrap_or_else(|_| Tag::default());
        tag.set_title(title);
        tag.set_artist(artist);
        tag.set_album_artist(artist); // Set album artist as well for better grouping
        tag.set_album(album);
        tag.set_year(year.trim());
        if let Ok(t) = track_number.trim().parse::<u16>() { tag.set_track_number(t); }
        tag.write_to_path(path).is_ok()
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
                    "https://lrclib.net/api/search?track_name={}&artist_name={}",
                    urlencoding::encode(&track.title),
                    urlencoding::encode(&track.artist)
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
                let dest_lrc_path = dest_path.with_extension("lrc");
                let _ = std::fs::rename(&lrc_path, &dest_lrc_path);
            }
            
            if let Some(parent) = track.path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
        
        if self.selected_album >= self.albums.len() {
            self.selected_album = self.albums.len().saturating_sub(1);
            self.selected_track = 0;
        }
        self.status = format!("Moved {} to library.", album.name);
    }
}

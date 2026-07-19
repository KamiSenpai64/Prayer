mod downloader;
mod metadata;
use audiotags::Tag;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Clear, Gauge, List, ListItem, ListState, Padding, Paragraph, Tabs, Wrap},
    Frame, Terminal,
};
use ratatui_image::{
    picker::Picker,
    protocol::StatefulProtocol,
    Resize, StatefulImage,
};
use regex::Regex;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs::{self, File},
    io::{self, BufRead, BufReader, Cursor, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use walkdir::WalkDir;

use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{Decoder as SymphoniaDecoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

// --- Config ---
#[derive(Serialize, Deserialize, Clone)]
struct AppConfig {
    music_directory: String,
    theme_color: String,
    enable_animations: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            music_directory: format!("{}/Music", home),
            theme_color: "Cyan".to_string(),
            enable_animations: true,
        }
    }
}

fn load_config() -> AppConfig {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let config_dir = PathBuf::from(home).join(".config").join("prayer");
    fs::create_dir_all(&config_dir).ok();
    
    let config_path = config_dir.join("config.toml");
    if let Ok(data) = fs::read_to_string(&config_path) {
        if let Ok(config) = toml::from_str(&data) {
            return config;
        }
    }
    
    let default_cfg = AppConfig::default();
    if let Ok(toml_str) = toml::to_string_pretty(&default_cfg) {
        fs::write(config_path, toml_str).ok();
    }
    default_cfg
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct PlaylistsData {
    lists: HashMap<String, Vec<PathBuf>>,
}

fn load_playlists() -> PlaylistsData {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = PathBuf::from(home).join(".config").join("prayer").join("playlists.toml");
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(pl) = toml::from_str(&data) {
            return pl;
        }
    }
    PlaylistsData::default()
}

fn save_playlists(data: &PlaylistsData) {
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = PathBuf::from(home).join(".config").join("prayer").join("playlists.toml");
    if let Ok(toml_str) = toml::to_string_pretty(data) {
        fs::write(path, toml_str).ok();
    }
}

// --- Icons ---
const ICON_MUSIC: &str = "";
const ICON_USER: &str = "";
const ICON_ALBUM: &str = "󰝚";
const ICON_PLAY: &str = "";
const ICON_PAUSE: &str = "";
const ICON_STOP: &str = "";
const ICON_QUEUE: &str = "";
const ICON_SEARCH: &str = "";
const ICON_FOLDER: &str = "";

// --- Symphonia Decoder ---
struct MyMediaSource { inner: Cursor<Vec<u8>> }
impl io::Read for MyMediaSource { fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> { self.inner.read(buf) } }
impl io::Seek for MyMediaSource { fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> { self.inner.seek(pos) } }
impl MediaSource for MyMediaSource {
    fn is_seekable(&self) -> bool { true }
    fn byte_len(&self) -> Option<u64> { Some(self.inner.get_ref().len() as u64) }
}

struct MyDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymphoniaDecoder>,
    sample_buf: SampleBuffer<i16>,
    sample_index: usize,
    spec: SignalSpec,
    total_duration: Option<Duration>,
}

impl MyDecoder {
    fn new(cursor: Cursor<Vec<u8>>, ext: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let source = MyMediaSource { inner: cursor };
        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let mut hint = Hint::new(); hint.with_extension(ext);
        let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())?;
        let track = probed.format.default_track().ok_or("No default track found")?;
        let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
        let total_duration = track.codec_params.time_base.zip(track.codec_params.n_frames).map(|(tb, frames)| {
            let time = tb.calc_time(frames); Duration::new(time.seconds, (time.frac * 1_000_000_000.0) as u32)
        });
        let mut format = probed.format;
        let mut decode_errors: usize = 0;
        let decoded = loop {
            let current_frame = format.next_packet()?;
            match decoder.decode(&current_frame) {
                Ok(decoded) => break decoded,
                Err(e) => { decode_errors += 1; if decode_errors > 3 { return Err(Box::new(e)); } else { continue; } }
            }
        };
        let spec = decoded.spec().clone();
        let duration = decoded.capacity() as u64;
        let mut sample_buf = SampleBuffer::new(duration, spec.clone());
        sample_buf.copy_interleaved_ref(decoded);
        Ok(MyDecoder { format, decoder, sample_buf, sample_index: 0, spec, total_duration })
    }
}
impl Iterator for MyDecoder {
    type Item = i16;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.sample_index < self.sample_buf.samples().len() {
                let sample = self.sample_buf.samples()[self.sample_index]; self.sample_index += 1; return Some(sample);
            }
            let packet = self.format.next_packet().ok()?;
            if let Ok(audio_buf) = self.decoder.decode(&packet) {
                self.spec = audio_buf.spec().clone();
                let duration = audio_buf.capacity() as u64;
                self.sample_buf = SampleBuffer::new(duration, self.spec);
                self.sample_buf.copy_interleaved_ref(audio_buf);
                self.sample_index = 0;
            }
        }
    }
}
impl Source for MyDecoder {
    fn current_frame_len(&self) -> Option<usize> { Some(self.sample_buf.samples().len().saturating_sub(self.sample_index)) }
    fn channels(&self) -> u16 { self.spec.channels.count() as u16 }
    fn sample_rate(&self) -> u32 { self.spec.rate }
    fn total_duration(&self) -> Option<Duration> { self.total_duration }
}

// --- App Data ---
#[derive(Clone, Debug)]
struct LyricLine { timestamp: Duration, text: String }

#[derive(Clone, Debug)]
struct Track {
    path: PathBuf,
    title: String,
    artist: String,
    album: String,
    year: String,
    track_number: u16,
    file_type: String,
    file_size: u64,
    lyrics: Vec<LyricLine>,
}

#[derive(PartialEq, Clone, Copy)]
enum ActiveTab { Player, Albums, Playlists, Downloader, Metadata }

#[derive(PartialEq)]
enum Focus { Artist, Album, Track, Queue, AlbumsView, PlaylistsList, PlaylistsTracks, Downloader, MetadataAlbums, MetadataTracks }

#[derive(PartialEq)]
enum Modal { None, Search, Help, PlaylistSelect, PlaylistCreate, EditMetadata, DownloaderSearch, MoveAlbum }

struct App {
    config: AppConfig,
    all_tracks: Vec<Track>,
    artists: Vec<String>,
    albums_by_artist: HashMap<String, Vec<String>>,
    tracks_by_album: HashMap<(String, String), Vec<Track>>,
    all_albums: Vec<(String, String)>, 

    focus: Focus,
    artist_state: ListState,
    album_state: ListState,
    track_state: ListState,
    queue_state: ListState,
    albums_view_state: ListState,

    playlists: PlaylistsData,
    playlists_list_state: ListState,
    playlists_track_state: ListState,
    playlist_select_state: ListState,
    playlist_target_track: Option<Track>,
    new_playlist_name: String,

    queue: Vec<Track>,
    history: Vec<Track>,
    active_tab: ActiveTab,
    modal: Modal,

    search_query: String,
    search_results: Vec<Track>,
    search_state: ListState,

    picker: Picker,
    current_cover: Option<Box<StatefulProtocol>>,
    hover_cover: Option<Box<StatefulProtocol>>,
    current_bitrate: u32,
    current_sample_rate: u32,

    sink: Sink,
    _stream: OutputStream,
    _stream_handle: OutputStreamHandle,
    current_track: Option<Track>,
    playback_start: Option<Instant>,
    paused_at: Option<Duration>,
    error_msg: Option<String>,
    tick_count: u64,
    volume: f32,
    is_focused_mode: bool,
    downloader: downloader::DownloaderState,
    metadata_editor: metadata::MetadataState,
    
    edit_title: String,
    edit_artist: String,
    edit_album: String,
    edit_year: String,
    edit_track: String,
    edit_filename: String,
    edit_focus: usize,
    is_bulk_edit: bool,
    move_album_dest: String,
}

impl App {
    fn new(config: AppConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let (_stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;

        let all_tracks = scan_directory(&config.music_directory);
        
        let mut artists_set = HashSet::new();
        let mut albums_by_artist: HashMap<String, HashSet<String>> = HashMap::new();
        let mut tracks_by_album: HashMap<(String, String), Vec<Track>> = HashMap::new();

        for track in &all_tracks {
            artists_set.insert(track.artist.clone());
            let album_display = if track.year.trim() == "----" || track.year.is_empty() { track.album.clone() } else { format!("[{}] {}", track.year, track.album) };
            albums_by_artist.entry(track.artist.clone()).or_default().insert(album_display.clone());
            tracks_by_album.entry((track.artist.clone(), album_display)).or_default().push(track.clone());
        }

        let mut artists: Vec<String> = artists_set.into_iter().collect();
        artists.sort_by_key(|a| a.to_lowercase());

        let mut final_albums = HashMap::new();
        for (artist, albums) in albums_by_artist {
            let mut album_list: Vec<String> = albums.into_iter().collect();
            album_list.sort_by_key(|a| a.to_lowercase());
            final_albums.insert(artist, album_list);
        }

        for tracks in tracks_by_album.values_mut() {
            tracks.sort_by(|a, b| a.track_number.cmp(&b.track_number).then(a.title.to_lowercase().cmp(&b.title.to_lowercase())));
        }
        
        let mut all_albums = vec![];
        for artist in &artists {
            if let Some(albums) = final_albums.get(artist) {
                for album in albums {
                    all_albums.push((artist.clone(), album.clone()));
                }
            }
        }

        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());

        let mut app = App {
            config,
            all_tracks,
            artists,
            albums_by_artist: final_albums,
            tracks_by_album,
            all_albums,
            focus: Focus::Artist,
            artist_state: ListState::default(),
            album_state: ListState::default(),
            track_state: ListState::default(),
            queue_state: ListState::default(),
            albums_view_state: ListState::default(),
            playlists: load_playlists(),
            playlists_list_state: ListState::default(),
            playlists_track_state: ListState::default(),
            playlist_select_state: ListState::default(),
            playlist_target_track: None,
            new_playlist_name: String::new(),
            queue: Vec::new(),
            history: Vec::new(),
            active_tab: ActiveTab::Player,
            modal: Modal::None,
            search_query: String::new(),
            search_results: Vec::new(),
            search_state: ListState::default(),
            picker,
            current_cover: None,
            hover_cover: None,
            current_bitrate: 0,
            current_sample_rate: 0,
            sink,
            _stream,
            _stream_handle: stream_handle,
            current_track: None,
            playback_start: None,
            paused_at: None,
            error_msg: None,
            tick_count: 0,
            volume: 1.0,
            is_focused_mode: false,
            downloader: downloader::DownloaderState::new(
                dirs::config_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join("prayer")
                    .join("tmp")
            ),
            metadata_editor: metadata::MetadataState::new(
                dirs::config_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join("prayer")
                    .join("tmp")
            ),
            edit_title: String::new(),
            edit_artist: String::new(),
            edit_album: String::new(),
            edit_year: String::new(),
            edit_track: String::new(),
            edit_filename: String::new(),
            edit_focus: 0,
            is_bulk_edit: false,
            move_album_dest: String::new(),
        };

        if !app.artists.is_empty() {
            app.artist_state.select(Some(0));
            app.update_cascading_selection();
        }
        if !app.all_albums.is_empty() {
            app.albums_view_state.select(Some(0));
            app.update_hover_cover();
        }
        if !app.playlists.lists.is_empty() {
            app.playlists_list_state.select(Some(0));
            app.playlists_track_state.select(Some(0));
        }

        Ok(app)
    }

    fn tick(&mut self) {
        if self.config.enable_animations {
            self.tick_count = self.tick_count.wrapping_add(1);
        }
    }

    fn update_hover_cover(&mut self) {
        self.hover_cover = None;
        if let Some(idx) = self.albums_view_state.selected() {
            if let Some((artist, album)) = self.all_albums.get(idx) {
                if let Some(tracks) = self.tracks_by_album.get(&(artist.clone(), album.clone())) {
                    if let Some(first_track) = tracks.first() {
                        if let Some(img) = get_cover_for_path(&first_track.path) {
                            let protocol = self.picker.new_resize_protocol(img);
                            self.hover_cover = Some(Box::new(protocol));
                        }
                    }
                }
            }
        }
    }

    fn update_cascading_selection(&mut self) {
        if let Some(artist_idx) = self.artist_state.selected() {
            let artist = &self.artists[artist_idx];
            let albums = self.albums_by_artist.get(artist).unwrap();
            if self.album_state.selected().is_none() || self.album_state.selected().unwrap() >= albums.len() {
                self.album_state.select(if albums.is_empty() { None } else { Some(0) });
            }
            if let Some(album_idx) = self.album_state.selected() {
                let album = &albums[album_idx];
                if let Some(tracks) = self.tracks_by_album.get(&(artist.clone(), album.clone())) {
                    if self.track_state.selected().is_none() || self.track_state.selected().unwrap() >= tracks.len() {
                        self.track_state.select(if tracks.is_empty() { None } else { Some(0) });
                    }
                }
            }
        }
    }

    fn update_search(&mut self) {
        let q = self.search_query.to_lowercase();
        self.search_results.clear();
        if q.is_empty() { 
            self.search_state.select(None);
            return; 
        }
        for track in &self.all_tracks {
            if track.title.to_lowercase().contains(&q)
                || track.artist.to_lowercase().contains(&q)
                || track.album.to_lowercase().contains(&q)
                || track.year.contains(&q) 
            {
                self.search_results.push(track.clone());
            }
        }
        self.search_results.sort_by(|a, b| {
            a.artist.to_lowercase().cmp(&b.artist.to_lowercase())
             .then(a.album.to_lowercase().cmp(&b.album.to_lowercase()))
             .then(a.track_number.cmp(&b.track_number))
             .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        if !self.search_results.is_empty() {
            self.search_state.select(Some(0));
        } else {
            self.search_state.select(None);
        }
    }

    fn next_tab(&mut self) {
        self.active_tab = match self.active_tab {
            ActiveTab::Player => ActiveTab::Albums,
            ActiveTab::Albums => ActiveTab::Playlists,
            ActiveTab::Playlists => ActiveTab::Downloader,
            ActiveTab::Downloader => ActiveTab::Metadata,
            ActiveTab::Metadata => ActiveTab::Player,
        };
        self.sync_focus_to_tab();
    }
    fn prev_tab(&mut self) {
        self.active_tab = match self.active_tab {
            ActiveTab::Player => ActiveTab::Metadata,
            ActiveTab::Albums => ActiveTab::Player,
            ActiveTab::Playlists => ActiveTab::Albums,
            ActiveTab::Downloader => ActiveTab::Playlists,
            ActiveTab::Metadata => ActiveTab::Downloader,
        };
        self.sync_focus_to_tab();
    }
    fn sync_focus_to_tab(&mut self) {
        if self.active_tab == ActiveTab::Albums { self.focus = Focus::AlbumsView; } 
        else if self.active_tab == ActiveTab::Player { self.focus = Focus::Track; }
        else if self.active_tab == ActiveTab::Playlists { self.focus = Focus::PlaylistsList; }
        else if self.active_tab == ActiveTab::Downloader { self.focus = Focus::Downloader; }
        else if self.active_tab == ActiveTab::Metadata { self.focus = Focus::MetadataAlbums; }
    }

    fn reload_library(&mut self) {
        let all_tracks = scan_directory(&self.config.music_directory);
        
        let mut artists_set = HashSet::new();
        let mut albums_by_artist: HashMap<String, HashSet<String>> = HashMap::new();
        let mut tracks_by_album: HashMap<(String, String), Vec<Track>> = HashMap::new();

        for track in &all_tracks {
            artists_set.insert(track.artist.clone());
            let album_display = if track.year.trim() == "----" || track.year.is_empty() { track.album.clone() } else { format!("[{}] {}", track.year, track.album) };
            albums_by_artist.entry(track.artist.clone()).or_default().insert(album_display.clone());
            tracks_by_album.entry((track.artist.clone(), album_display)).or_default().push(track.clone());
        }

        let mut artists: Vec<String> = artists_set.into_iter().collect();
        artists.sort_by_key(|a| a.to_lowercase());

        let mut final_albums = HashMap::new();
        for (artist, albums) in albums_by_artist {
            let mut album_list: Vec<String> = albums.into_iter().collect();
            album_list.sort_by_key(|a| a.to_lowercase());
            final_albums.insert(artist, album_list);
        }

        for tracks in tracks_by_album.values_mut() {
            tracks.sort_by(|a, b| a.track_number.cmp(&b.track_number).then(a.title.to_lowercase().cmp(&b.title.to_lowercase())));
        }
        
        let mut all_albums = vec![];
        for artist in &artists {
            if let Some(albums) = final_albums.get(artist) {
                for album in albums {
                    all_albums.push((artist.clone(), album.clone()));
                }
            }
        }
        
        self.all_tracks = all_tracks;
        self.artists = artists;
        self.albums_by_artist = final_albums;
        self.tracks_by_album = tracks_by_album;
        self.all_albums = all_albums;
    }

    fn next_focus(&mut self) {
        if self.active_tab == ActiveTab::Player {
            self.focus = match self.focus {
                Focus::Artist => Focus::Album, Focus::Album => Focus::Track,
                Focus::Track => Focus::Queue, Focus::Queue => Focus::Artist,
                _ => Focus::Artist,
            }
        } else if self.active_tab == ActiveTab::Playlists {
            self.focus = match self.focus {
                Focus::PlaylistsList => Focus::PlaylistsTracks,
                _ => Focus::PlaylistsList,
            }
        } else if self.active_tab == ActiveTab::Metadata {
            self.focus = match self.focus {
                Focus::MetadataAlbums => Focus::MetadataTracks,
                _ => Focus::MetadataAlbums,
            }
        }
    }
    fn prev_focus(&mut self) {
        if self.active_tab == ActiveTab::Player {
            self.focus = match self.focus {
                Focus::Artist => Focus::Queue, Focus::Album => Focus::Artist,
                Focus::Track => Focus::Album, Focus::Queue => Focus::Track,
                _ => Focus::Artist,
            }
        } else if self.active_tab == ActiveTab::Playlists {
            self.focus = match self.focus {
                Focus::PlaylistsTracks => Focus::PlaylistsList,
                _ => Focus::PlaylistsTracks,
            }
        } else if self.active_tab == ActiveTab::Metadata {
            self.focus = match self.focus {
                Focus::MetadataTracks => Focus::MetadataAlbums,
                _ => Focus::MetadataTracks,
            }
        }
    }

    fn move_up(&mut self) {
        match self.focus {
            Focus::Artist => if let Some(i) = self.artist_state.selected() {
                self.artist_state.select(Some(i.saturating_sub(1))); self.album_state.select(Some(0)); self.track_state.select(Some(0));
            },
            Focus::Album => if let Some(i) = self.album_state.selected() {
                self.album_state.select(Some(i.saturating_sub(1))); self.track_state.select(Some(0));
            },
            Focus::Track => if let Some(i) = self.track_state.selected() { self.track_state.select(Some(i.saturating_sub(1))); },
            Focus::Queue => if let Some(i) = self.queue_state.selected() { self.queue_state.select(Some(i.saturating_sub(1))); },
            Focus::AlbumsView => if let Some(i) = self.albums_view_state.selected() {
                self.albums_view_state.select(Some(i.saturating_sub(1))); self.update_hover_cover();
            },
            Focus::PlaylistsList => if let Some(i) = self.playlists_list_state.selected() {
                self.playlists_list_state.select(Some(i.saturating_sub(1))); self.playlists_track_state.select(Some(0));
            },
            Focus::PlaylistsTracks => if let Some(i) = self.playlists_track_state.selected() {
                self.playlists_track_state.select(Some(i.saturating_sub(1)));
            },
            Focus::Downloader => {
                self.downloader.selected_index = self.downloader.selected_index.saturating_sub(1);
            },
            Focus::MetadataAlbums => {
                self.metadata_editor.selected_album = self.metadata_editor.selected_album.saturating_sub(1);
                self.metadata_editor.selected_track = 0;
            },
            Focus::MetadataTracks => {
                self.metadata_editor.selected_track = self.metadata_editor.selected_track.saturating_sub(1);
            },
        }
        if self.active_tab == ActiveTab::Player && self.focus != Focus::Queue { self.update_cascading_selection(); }
    }

    fn move_down(&mut self) {
        match self.focus {
            Focus::Artist => if let Some(i) = self.artist_state.selected() {
                let next = (i + 1).min(self.artists.len().saturating_sub(1));
                self.artist_state.select(Some(next)); self.album_state.select(Some(0)); self.track_state.select(Some(0));
            },
            Focus::Album => if let Some(artist_idx) = self.artist_state.selected() {
                let artist = &self.artists[artist_idx];
                if let Some(albums) = self.albums_by_artist.get(artist) {
                    if let Some(i) = self.album_state.selected() {
                        let next = (i + 1).min(albums.len().saturating_sub(1));
                        self.album_state.select(Some(next)); self.track_state.select(Some(0));
                    }
                }
            },
            Focus::Track => if let Some(artist_idx) = self.artist_state.selected() {
                if let Some(album_idx) = self.album_state.selected() {
                    let artist = &self.artists[artist_idx];
                    let album = &self.albums_by_artist.get(artist).unwrap()[album_idx];
                    if let Some(tracks) = self.tracks_by_album.get(&(artist.clone(), album.clone())) {
                        if let Some(i) = self.track_state.selected() {
                            let next = (i + 1).min(tracks.len().saturating_sub(1));
                            self.track_state.select(Some(next));
                        }
                    }
                }
            },
            Focus::Queue => if let Some(i) = self.queue_state.selected() {
                let next = (i + 1).min(self.queue.len().saturating_sub(1)); self.queue_state.select(Some(next));
            } else if !self.queue.is_empty() { self.queue_state.select(Some(0)); },
            Focus::AlbumsView => if let Some(i) = self.albums_view_state.selected() {
                let next = (i + 1).min(self.all_albums.len().saturating_sub(1));
                self.albums_view_state.select(Some(next)); self.update_hover_cover();
            },
            Focus::PlaylistsList => if let Some(i) = self.playlists_list_state.selected() {
                let next = (i + 1).min(self.playlists.lists.len().saturating_sub(1));
                self.playlists_list_state.select(Some(next)); self.playlists_track_state.select(Some(0));
            } else if !self.playlists.lists.is_empty() { self.playlists_list_state.select(Some(0)); },
            Focus::PlaylistsTracks => if let Some(pl_idx) = self.playlists_list_state.selected() {
                let mut pl_names: Vec<String> = self.playlists.lists.keys().cloned().collect();
                pl_names.sort();
                if let Some(pl_name) = pl_names.get(pl_idx) {
                    if let Some(paths) = self.playlists.lists.get(pl_name) {
                        if let Some(i) = self.playlists_track_state.selected() {
                            let next = (i + 1).min(paths.len().saturating_sub(1));
                            self.playlists_track_state.select(Some(next));
                        } else if !paths.is_empty() { self.playlists_track_state.select(Some(0)); }
                    }
                }
            },
            Focus::Downloader => {
                let len = self.downloader.results.lock().unwrap().len();
                if len > 0 {
                    self.downloader.selected_index = (self.downloader.selected_index + 1).min(len - 1);
                }
            },
            Focus::MetadataAlbums => {
                let len = self.metadata_editor.albums.len();
                if len > 0 {
                    self.metadata_editor.selected_album = (self.metadata_editor.selected_album + 1).min(len - 1);
                    self.metadata_editor.selected_track = 0;
                }
            },
            Focus::MetadataTracks => {
                if let Some(album) = self.metadata_editor.albums.get(self.metadata_editor.selected_album) {
                    let len = album.tracks.len();
                    if len > 0 {
                        self.metadata_editor.selected_track = (self.metadata_editor.selected_track + 1).min(len - 1);
                    }
                }
            },
        }
        if self.active_tab == ActiveTab::Player && self.focus != Focus::Queue { self.update_cascading_selection(); }
    }

    fn play_selected(&mut self) {
        if let Some(track) = self.get_selected_track_anywhere() {
            self.queue.clear();
            self.play_track(track);
            if !self.is_focused_mode {
                self.active_tab = ActiveTab::Player;
                self.sync_focus_to_tab();
            }
        } else if self.focus == Focus::AlbumsView {
            self.play_selected_album();
        }
    }
    
    fn play_selected_album(&mut self) {
        if let Some(idx) = self.albums_view_state.selected() {
            if let Some((artist, album)) = self.all_albums.get(idx) {
                if let Some(tracks) = self.tracks_by_album.get(&(artist.clone(), album.clone())) {
                    if let Some(first) = tracks.first() {
                        self.queue.clear();
                        for t in tracks.iter().skip(1) { self.queue.push(t.clone()); }
                        if !self.queue.is_empty() { self.queue_state.select(Some(0)); } else { self.queue_state.select(None); }
                        self.play_track(first.clone());
                        if !self.is_focused_mode {
                            self.active_tab = ActiveTab::Player;
                            self.sync_focus_to_tab();
                        }
                    }
                }
            }
        }
    }

    fn toggle_queue_selected(&mut self) {
        if let Some(track) = self.get_selected_track_anywhere() {
            if let Some(idx) = self.queue.iter().position(|t| t.path == track.path) {
                self.queue.remove(idx);
            } else {
                self.queue.push(track);
            }
            if self.queue_state.selected().is_none() && !self.queue.is_empty() { 
                self.queue_state.select(Some(0)); 
            }
        } else if self.focus == Focus::AlbumsView {
            if let Some(idx) = self.albums_view_state.selected() {
                if let Some((artist, album)) = self.all_albums.get(idx) {
                    if let Some(tracks) = self.tracks_by_album.get(&(artist.clone(), album.clone())) {
                        for track in tracks { self.queue.push(track.clone()); }
                        if self.sink.empty() && self.current_track.is_none() && !self.queue.is_empty() {
                            let t = self.queue.remove(0); self.play_track(t);
                        }
                    }
                }
            }
        }
    }

    fn delete_selected_playlist_track(&mut self) {
        if self.active_tab == ActiveTab::Playlists {
            if self.focus == Focus::PlaylistsTracks {
                let mut pl_names: Vec<String> = self.playlists.lists.keys().cloned().collect();
                pl_names.sort();
                if let Some(pl_idx) = self.playlists_list_state.selected() {
                    if let Some(pl_name) = pl_names.get(pl_idx) {
                        if let Some(t_idx) = self.playlists_track_state.selected() {
                            let mut removed = false;
                            let mut empty = false;
                            let mut len = 0;
                            if let Some(paths) = self.playlists.lists.get_mut(pl_name) {
                                if t_idx < paths.len() {
                                    paths.remove(t_idx);
                                    removed = true;
                                    empty = paths.is_empty();
                                    len = paths.len();
                                }
                            }
                            if removed {
                                save_playlists(&self.playlists);
                                if empty {
                                    self.playlists_track_state.select(None);
                                } else if t_idx >= len {
                                    self.playlists_track_state.select(Some(len - 1));
                                }
                            }
                        }
                    }
                }
            } else if self.focus == Focus::PlaylistsList {
                let mut pl_names: Vec<String> = self.playlists.lists.keys().cloned().collect();
                pl_names.sort();
                if let Some(pl_idx) = self.playlists_list_state.selected() {
                    if let Some(pl_name) = pl_names.get(pl_idx) {
                        self.playlists.lists.remove(pl_name);
                        save_playlists(&self.playlists);
                        if self.playlists.lists.is_empty() {
                            self.playlists_list_state.select(None);
                        } else {
                            self.playlists_list_state.select(Some(pl_idx.saturating_sub(1)));
                        }
                        self.playlists_track_state.select(Some(0));
                    }
                }
            }
        }
    }

    fn get_selected_track(&self) -> Option<Track> {
        if let Some(artist_idx) = self.artist_state.selected() {
            if let Some(album_idx) = self.album_state.selected() {
                if let Some(track_idx) = self.track_state.selected() {
                    let artist = &self.artists[artist_idx];
                    let album = &self.albums_by_artist.get(artist).unwrap()[album_idx];
                    if let Some(tracks) = self.tracks_by_album.get(&(artist.clone(), album.clone())) {
                        if track_idx < tracks.len() { return Some(tracks[track_idx].clone()); }
                    }
                }
            }
        }
        None
    }

    fn get_selected_track_anywhere(&self) -> Option<Track> {
        if self.active_tab == ActiveTab::Player {
            if self.focus == Focus::Track {
                return self.get_selected_track();
            } else if self.focus == Focus::Queue {
                if let Some(idx) = self.queue_state.selected() {
                    if idx < self.queue.len() { return Some(self.queue[idx].clone()); }
                }
            }
        } else if self.active_tab == ActiveTab::Playlists {
            if self.focus == Focus::PlaylistsTracks {
                let mut pl_names: Vec<String> = self.playlists.lists.keys().cloned().collect();
                pl_names.sort();
                if let Some(pl_idx) = self.playlists_list_state.selected() {
                    if let Some(pl_name) = pl_names.get(pl_idx) {
                        if let Some(paths) = self.playlists.lists.get(pl_name) {
                            if let Some(t_idx) = self.playlists_track_state.selected() {
                                if let Some(path) = paths.get(t_idx) {
                                    return self.all_tracks.iter().find(|t| &t.path == path).cloned();
                                }
                            }
                        }
                    }
                }
            }
        } else if self.modal == Modal::Search {
            if let Some(idx) = self.search_state.selected() {
                if idx < self.search_results.len() { return Some(self.search_results[idx].clone()); }
            }
        }
        None
    }

    fn play_track(&mut self, track: Track) {
        self.error_msg = None; 
        
        let file_data = match fs::read(&track.path) {
            Ok(data) => data, Err(e) => { self.error_msg = Some(format!("Error reading file: {}", e)); return; }
        };

        let cursor = Cursor::new(file_data);
        let ext = track.path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        let source = match MyDecoder::new(cursor, &ext) {
            Ok(s) => s, Err(e) => { self.error_msg = Some(format!("Audio Decode Error: {}", e)); return; }
        };

        self.current_sample_rate = source.sample_rate();
        if let Some(dur) = source.total_duration() {
            let secs = dur.as_secs_f64();
            self.current_bitrate = if secs > 0.0 { (track.file_size as f64 * 8.0 / 1000.0 / secs) as u32 } else { 0 };
        } else { self.current_bitrate = 0; }

        self.sink.stop();
        self.sink.append(source);
        self.sink.set_volume(self.volume);
        self.sink.play();
        
        if let Some(current) = self.current_track.take() {
            self.history.push(current);
        }
        self.current_track = Some(track.clone());
        self.playback_start = Some(Instant::now());
        self.paused_at = None;
        
        if let Some(img) = get_cover_for_path(&track.path) {
            let protocol = self.picker.new_resize_protocol(img);
            self.current_cover = Some(Box::new(protocol));
        } else {
            self.current_cover = None;
        }
    }

    fn play_next(&mut self) {
        if !self.queue.is_empty() {
            let next_track = self.queue.remove(0);
            self.play_track(next_track);
        } else if let Some(current) = &self.current_track {
            let artist = current.artist.clone();
            let mut found = false;
            for ((a, _), tracks) in &self.tracks_by_album {
                if a == &artist {
                    if let Some(idx) = tracks.iter().position(|t| t.path == current.path) {
                        if idx + 1 < tracks.len() {
                            self.play_track(tracks[idx + 1].clone());
                            found = true;
                            break;
                        }
                    }
                }
            }
            if !found {
                self.sink.stop();
                if let Some(current) = self.current_track.take() {
                    self.history.push(current);
                }
            }
        }
    }

    fn play_previous(&mut self) {
        if let Some(prev) = self.history.pop() {
            self.play_track(prev);
            self.history.pop();
        }
    }

    fn toggle_playback(&mut self) {
        if self.sink.is_paused() {
            self.sink.play();
            if let Some(paused) = self.paused_at { self.playback_start = Some(Instant::now() - paused); self.paused_at = None; }
        } else {
            self.sink.pause();
            if let Some(start) = self.playback_start { self.paused_at = Some(start.elapsed()); }
        }
    }

    fn current_playback_time(&self) -> Duration {
        if let Some(paused) = self.paused_at { paused } 
        else if let Some(start) = self.playback_start { start.elapsed() } 
        else { Duration::from_secs(0) }
    }
}

fn get_cover_for_path(path: &Path) -> Option<image::DynamicImage> {
    if let Ok(tag) = Tag::new().read_from_path(path) {
        if let Some(pic) = tag.album_cover() {
            if let Ok(img) = image::load_from_memory(pic.data) { return Some(img); }
        }
    }
    None
}

fn scan_directory(dir: &str) -> Vec<Track> {
    let mut tracks = Vec::new();
    let supported_exts = ["mp3", "flac", "wav", "m4a"];
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if supported_exts.contains(&ext.to_lowercase().as_str()) {
                    let mut title = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    let mut artist = "Unknown Artist".to_string();
                    let mut album = "Unknown Album".to_string();
                    let mut year = "----".to_string();
                    let mut track_number = 0;

                    if let Ok(tags) = Tag::new().read_from_path(path) {
                        if let Some(t) = tags.title() { title = t.to_string(); }
                        if let Some(a) = tags.artist() { artist = a.to_string(); }
                        if let Some(al) = tags.album_title() { album = al.to_string(); }
                        if let Some(y) = tags.year() { year = y.to_string(); }
                        if let Some(num) = tags.track_number() { track_number = num; }
                    }
                    let file_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                    let lyrics = extract_lyrics(path);
                    tracks.push(Track { path: path.to_path_buf(), title, artist, album, year, track_number, file_type: ext.to_uppercase(), file_size, lyrics });
                }
            }
        }
    }
    tracks
}

fn extract_lyrics(audio_path: &Path) -> Vec<LyricLine> {
    let mut lrc_path = audio_path.to_path_buf(); lrc_path.set_extension("lrc");
    if let Some(lyrics) = parse_lrc_file(&lrc_path) { return lyrics; }
    let mut lrc_appended = audio_path.to_path_buf();
    lrc_appended.set_file_name(format!("{}.lrc", audio_path.file_name().unwrap_or_default().to_string_lossy()));
    if let Some(lyrics) = parse_lrc_file(&lrc_appended) { return lyrics; }
    Vec::new()
}

fn parse_lrc_file(path: &PathBuf) -> Option<Vec<LyricLine>> {
    let file = File::open(path).ok()?;
    let mut lines = Vec::new();
    let re = Regex::new(r"^\[(\d{1,3}):(\d{2})(?:[:.](\d{2,3}))?\](.*)").unwrap();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Some(caps) = re.captures(&line) {
            let mins: u64 = caps[1].parse().unwrap_or(0);
            let secs: u64 = caps[2].parse().unwrap_or(0);
            let millis: u64 = caps.get(3).map_or(0, |m| m.as_str().parse().unwrap_or(0));
            let ms_multiplier = if caps.get(3).map_or(0, |m| m.as_str().len()) == 2 { 10 } else { 1 };
            let timestamp = Duration::from_secs(mins * 60 + secs) + Duration::from_millis(millis * ms_multiplier);
            lines.push(LyricLine { timestamp, text: caps[4].trim().to_string() });
        }
    }
    if lines.is_empty() { return None; }
    lines.sort_by_key(|l| l.timestamp);
    Some(lines)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = load_config();
    if let Some(dir) = env::args().nth(1) {
        config.music_directory = dir;
    }

    let mut app = App::new(config)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    if let Err(err) = res { println!("{:?}", err) }
    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        if app.sink.empty() && app.current_track.is_some() {
            app.play_next();
        }

        terminal.draw(|f| ui(f, app))?;

        let mut finished = false;
        if let Ok(mut lock) = app.downloader.download_finished.lock() {
            if *lock {
                *lock = false;
                finished = true;
            }
        }
        if finished {
            app.active_tab = ActiveTab::Metadata;
            app.sync_focus_to_tab();
            app.metadata_editor.scan_directory();
            for i in 0..app.metadata_editor.albums.len() {
        app.metadata_editor.fetch_lyrics(i);
            }
        }

        if crossterm::event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = crossterm::event::read()? {
                if key.kind != crossterm::event::KeyEventKind::Press { continue; }
                
                // Clear fetching lyrics status on next keypress if finished
                if !*app.metadata_editor.is_loading.lock().unwrap() && app.metadata_editor.status.starts_with("Fetching") {
                    app.metadata_editor.status = "Finished fetching lyrics.".to_string();
                }
                match app.modal {
                    Modal::Search => {
                        match key.code {
                            KeyCode::Esc => app.modal = Modal::None,
                            KeyCode::Enter => {
                                if app.active_tab == ActiveTab::Downloader {
                                    app.downloader.search(app.search_query.clone());
                                } else {
                                    app.play_selected();
                                }
                                app.modal = Modal::None;
                            },
                            KeyCode::Up => {
                                if let Some(i) = app.search_state.selected() { app.search_state.select(Some(i.saturating_sub(1))); }
                            },
                            KeyCode::Down => {
                                if let Some(i) = app.search_state.selected() {
                                    let next = (i + 1).min(app.search_results.len().saturating_sub(1)); app.search_state.select(Some(next));
                                } else if !app.search_results.is_empty() { app.search_state.select(Some(0)); }
                            },
                            KeyCode::Backspace => { app.search_query.pop(); app.update_search(); },
                            KeyCode::Char(c) => { app.search_query.push(c); app.update_search(); },
                            _ => {}
                        }
                    },
                    Modal::Help => {
                        if key.code == KeyCode::Esc || key.code == KeyCode::Char('?') { app.modal = Modal::None; }
                    },
                    Modal::PlaylistSelect => {
                        let mut pl_names: Vec<String> = app.playlists.lists.keys().cloned().collect();
                        pl_names.sort();
                        match key.code {
                            KeyCode::Esc => app.modal = Modal::None,
                            KeyCode::Up => {
                                if let Some(i) = app.playlist_select_state.selected() {
                                    app.playlist_select_state.select(Some(i.saturating_sub(1)));
                                }
                            }
                            KeyCode::Down => {
                                if let Some(i) = app.playlist_select_state.selected() {
                                    app.playlist_select_state.select(Some((i + 1).min(pl_names.len()))); 
                                } else { app.playlist_select_state.select(Some(0)); }
                            }
                            KeyCode::Enter => {
                                if let Some(idx) = app.playlist_select_state.selected() {
                                    if idx == 0 {
                                        app.modal = Modal::PlaylistCreate;
                                        app.new_playlist_name.clear();
                                    } else {
                                        let pl_name = &pl_names[idx - 1];
                                        if let Some(track) = &app.playlist_target_track {
                                            if !app.playlists.lists.get(pl_name).unwrap().contains(&track.path) {
                                                app.playlists.lists.get_mut(pl_name).unwrap().push(track.path.clone());
                                                save_playlists(&app.playlists);
                                            }
                                        }
                                        app.modal = Modal::None;
                                    }
                                }
                            }
                            _ => {}
                        }
                    },
                    Modal::PlaylistCreate => {
                        if key.code == KeyCode::Esc { app.modal = Modal::None; }
                        else if key.code == KeyCode::Enter {
                            if !app.new_playlist_name.is_empty() {
                                let name = app.new_playlist_name.clone();
                                app.playlists.lists.entry(name.clone()).or_insert_with(Vec::new);
                                if let Some(track) = &app.playlist_target_track {
                                    app.playlists.lists.get_mut(&name).unwrap().push(track.path.clone());
                                }
                                save_playlists(&app.playlists);
                                app.modal = Modal::None;
                            }
                        }
                        else if key.code == KeyCode::Backspace { app.new_playlist_name.pop(); }
                        else if let KeyCode::Char(c) = key.code { app.new_playlist_name.push(c); }
                    },
                    Modal::EditMetadata => {
                        let _ = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/tui_player_keys.log").and_then(|mut f| {
                            use std::io::Write;
                            writeln!(f, "EditMetadata key: {:?} kind: {:?} focus: {} bulk: {}", key.code, key.kind, app.edit_focus, app.is_bulk_edit)
                        });
                        match key.code {
                            KeyCode::Esc => app.modal = Modal::None,
                            KeyCode::Tab | KeyCode::Down => {
                                let max = if app.is_bulk_edit { 3 } else { 6 };
                                app.edit_focus = (app.edit_focus + 1) % max;
                            },
                            KeyCode::BackTab | KeyCode::Up => {
                                let max = if app.is_bulk_edit { 3 } else { 6 };
                                app.edit_focus = (app.edit_focus + max - 1) % max;
                            },
                            KeyCode::Enter => {
                                if let Some(album) = app.metadata_editor.albums.get(app.metadata_editor.selected_album) {
                                    if app.is_bulk_edit {
                                        for track in &album.tracks {
                                            crate::metadata::MetadataState::save_track_metadata(&track.path, &track.title, &app.edit_artist, &app.edit_album, &app.edit_year, &track.track_number);
                                        }
                                        app.metadata_editor.scan_directory();
                                    } else if let Some(track) = album.tracks.get(app.metadata_editor.selected_track) {
                                        let old_path = track.path.clone();
                                        if crate::metadata::MetadataState::save_track_metadata(&old_path, &app.edit_title, &app.edit_artist, &app.edit_album, &app.edit_year, &app.edit_track) {
                                            if !app.edit_filename.is_empty() {
                                                let new_path = old_path.with_file_name(&app.edit_filename);
                                                if std::fs::rename(&old_path, &new_path).is_ok() {
                                                    let old_lrc = old_path.with_extension("lrc");
                                                    let new_lrc = new_path.with_extension("lrc");
                                                    if old_lrc.exists() {
                                                        let _ = std::fs::rename(&old_lrc, &new_lrc);
                                                    }
                                                }
                                            }
                                            app.metadata_editor.scan_directory();
                                        }
                                    }
                                }
                                app.modal = Modal::None;
                            },
                            KeyCode::Backspace => {
                                match app.edit_focus {
                                    0 => if app.is_bulk_edit { app.edit_artist.pop(); } else { app.edit_title.pop(); },
                                    1 => if app.is_bulk_edit { app.edit_album.pop(); } else { app.edit_artist.pop(); },
                                    2 => if app.is_bulk_edit { app.edit_year.pop(); } else { app.edit_album.pop(); },
                                    3 => if !app.is_bulk_edit { app.edit_year.pop(); },
                                    4 => if !app.is_bulk_edit { app.edit_track.pop(); },
                                    5 => if !app.is_bulk_edit { app.edit_filename.pop(); },
                                    _ => {}
                                }
                                let _ = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/tui_player_keys.log").and_then(|mut f| {
                                    use std::io::Write;
                                    writeln!(f, "After Backspace, title is now: {}", app.edit_title)
                                });
                            },
                            KeyCode::Char(c) => {
                                match app.edit_focus {
                                    0 => if app.is_bulk_edit { app.edit_artist.push(c); } else { app.edit_title.push(c); },
                                    1 => if app.is_bulk_edit { app.edit_album.push(c); } else { app.edit_artist.push(c); },
                                    2 => if app.is_bulk_edit { app.edit_year.push(c); } else { app.edit_album.push(c); },
                                    3 => if !app.is_bulk_edit { app.edit_year.push(c); },
                                    4 => if !app.is_bulk_edit { app.edit_track.push(c); },
                                    5 => if !app.is_bulk_edit { app.edit_filename.push(c); },
                                    _ => {}
                                }
                                let _ = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/tui_player_keys.log").and_then(|mut f| {
                                    use std::io::Write;
                                    writeln!(f, "After Char({}), title is now: {}", c, app.edit_title)
                                });
                            },
                            _ => {}
                        }
                    },
                    Modal::DownloaderSearch => {
                        match key.code {
                            KeyCode::Esc => app.modal = Modal::None,
                            KeyCode::Enter => {
                                app.downloader.search(app.downloader.query.clone());
                                app.modal = Modal::None;
                            },
                            KeyCode::Backspace => { app.downloader.query.pop(); },
                            KeyCode::Char(c) => { app.downloader.query.push(c); },
                            _ => {}
                        }
                    },
                    Modal::MoveAlbum => {
                        match key.code {
                            KeyCode::Esc => app.modal = Modal::None,
                            KeyCode::Enter => {
                                if !app.move_album_dest.is_empty() {
                                    app.metadata_editor.move_album_to_library(app.metadata_editor.selected_album, std::path::PathBuf::from(&app.move_album_dest));
                                    app.reload_library();
                                    app.sync_focus_to_tab();
                                }
                                app.modal = Modal::None;
                            },
                            KeyCode::Backspace => { app.move_album_dest.pop(); },
                            KeyCode::Char(c) => { app.move_album_dest.push(c); },
                            _ => {}
                        }
                    },
                    Modal::None => {
                        if (key.code == KeyCode::Char('n') || key.code == KeyCode::Char('N')) 
                            && key.modifiers.contains(KeyModifiers::ALT) 
                            && key.modifiers.contains(KeyModifiers::SHIFT) {
                            app.is_focused_mode = !app.is_focused_mode;
                            continue;
                        }
                        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::ALT) {
                            if let Some(track) = app.get_selected_track_anywhere() {
                                app.playlist_target_track = Some(track);
                                app.modal = Modal::PlaylistSelect;
                                app.playlist_select_state.select(Some(0));
                            }
                            continue;
                        }
                        if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            app.modal = Modal::Search;
                            app.search_query.clear();
                            app.update_search();
                            continue;
                        }
                        if key.code == KeyCode::Char('/') && app.active_tab == ActiveTab::Downloader {
                            app.modal = Modal::DownloaderSearch;
                            app.downloader.query.clear();
                            continue;
                        }
                        if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::CONTROL) {
                            if key.modifiers.contains(KeyModifiers::SHIFT) { app.prev_tab(); } else { app.next_tab(); }
                            continue;
                        }
                        if key.code == KeyCode::BackTab && key.modifiers.contains(KeyModifiers::CONTROL) {
                            app.prev_tab(); continue;
                        }
                        match key.code {
                            KeyCode::Char('q') => return Ok(()),
                            KeyCode::Char('?') => app.modal = Modal::Help,
                            KeyCode::Char('n') | KeyCode::Char('.') | KeyCode::Char('>') => app.play_next(),
                            KeyCode::Char('p') | KeyCode::Char(',') | KeyCode::Char('<') => app.play_previous(),
                            KeyCode::Char('-') => { app.volume = (app.volume - 0.1).max(0.0); app.sink.set_volume(app.volume); },
                            KeyCode::Char('+') | KeyCode::Char('=') => { app.volume = (app.volume + 0.1).min(1.0); app.sink.set_volume(app.volume); },
                            KeyCode::Char('1') => { app.active_tab = ActiveTab::Player; app.sync_focus_to_tab(); },
                            KeyCode::Char('2') => { app.active_tab = ActiveTab::Albums; app.sync_focus_to_tab(); },
                            KeyCode::Char('3') => { app.active_tab = ActiveTab::Playlists; app.sync_focus_to_tab(); },
                            KeyCode::PageDown => app.next_tab(),
                            KeyCode::PageUp => app.prev_tab(),
                            KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                            KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                            KeyCode::Right => app.next_focus(),
                            KeyCode::Left | KeyCode::Char('h') => app.prev_focus(),
                            KeyCode::Tab => app.next_focus(),
                            KeyCode::BackTab => app.prev_focus(),
                            KeyCode::Enter => app.play_selected(),
                            KeyCode::Char('s') => {
                                if app.active_tab == ActiveTab::Downloader {
                                    app.downloader.toggle_selection();
                                }
                            },
                            KeyCode::Char('a') => {
                                if app.active_tab == ActiveTab::Downloader {
                                    app.downloader.search_albums_only = !app.downloader.search_albums_only;
                                } else {
                                    app.toggle_queue_selected();
                                }
                            },
                            KeyCode::Char('c') => {
                                if app.active_tab == ActiveTab::Downloader {
                                    app.downloader.cancel_download();
                                }
                            },
                            KeyCode::Char('d') => {
                                if app.active_tab == ActiveTab::Downloader {
                                    app.downloader.download_selected();
                                } else {
                                    app.delete_selected_playlist_track();
                                }
                            },
                            KeyCode::Char('e') => {
                                if app.active_tab == ActiveTab::Metadata {
                                    if let Some(album) = app.metadata_editor.albums.get(app.metadata_editor.selected_album) {
                                        if let Some(track) = album.tracks.get(app.metadata_editor.selected_track) {
                                            app.edit_title = track.title.clone();
                                            app.edit_artist = track.artist.clone();
                                            app.edit_album = track.album.clone();
                                            app.edit_year = track.year.clone();
                                            app.edit_track = track.track_number.to_string();
                                            app.edit_filename = track.path.file_name().unwrap().to_string_lossy().into_owned();
                                            app.edit_focus = 0;
                                            app.is_bulk_edit = false;
                                            app.modal = Modal::EditMetadata;
                                        }
                                    }
                                }
                            },
                            KeyCode::Char('E') => {
                                if app.active_tab == ActiveTab::Metadata {
                                    if let Some(album) = app.metadata_editor.albums.get(app.metadata_editor.selected_album) {
                                        if let Some(track) = album.tracks.first() {
                                            app.edit_artist = track.artist.clone();
                                            app.edit_album = track.album.clone();
                                            app.edit_year = track.year.clone();
                                            app.edit_focus = 0;
                                            app.is_bulk_edit = true;
                                            app.modal = Modal::EditMetadata;
                                        }
                                    }
                                }
                            },
                            KeyCode::Char('l') => {
                                if app.active_tab == ActiveTab::Metadata {
                                    if app.focus == Focus::MetadataAlbums || app.focus == Focus::MetadataTracks {
                                        app.metadata_editor.fetch_lyrics(app.metadata_editor.selected_album);
                                    } else {
                                        app.next_focus();
                                    }
                                } else {
                                    app.next_focus();
                                }
                            },
                            KeyCode::Char('m') => {
                                if app.active_tab == ActiveTab::Metadata {
                                    if app.focus == Focus::MetadataAlbums || app.focus == Focus::MetadataTracks {
                                        app.move_album_dest = app.config.music_directory.clone();
                                        app.modal = Modal::MoveAlbum;
                                    }
                                }
                            },
                            KeyCode::Char('r') => {
                                if app.active_tab == ActiveTab::Metadata {
                                    app.metadata_editor.scan_directory();
                                }
                            },
                            KeyCode::Char(' ') => {
                                if app.focus == Focus::AlbumsView {
                                    app.play_selected_album();
                                } else {
                                    app.toggle_playback();
                                }
                            },

                            _ => {}
                        }
                    }
                }
            }
        } else {
            app.tick();
        }
    }
}

// --- UI Styling ---
fn theme_color(app: &App) -> Color {
    match app.config.theme_color.to_lowercase().as_str() {
        "cyan" => Color::Cyan,
        "magenta" => Color::Magenta,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "red" => Color::Red,
        "blue" => Color::Blue,
        _ => Color::Cyan,
    }
}

fn create_block(title: &str, is_focused: bool, theme: Color) -> Block<'static> {
    let border_style = if is_focused { Style::default().fg(theme).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(format!(" {} ", title))
        .padding(Padding::symmetric(1, 0))
}

fn ui(f: &mut Frame, app: &mut App) {
    let theme = theme_color(app);
    let main_chunks = Layout::default().direction(Direction::Vertical).constraints([
        Constraint::Length(3), Constraint::Min(0), Constraint::Length(3),
    ]).split(f.area());

    let tab_titles = vec![
        format!(" {} Player ", ICON_MUSIC),
        format!(" {} Albums ", ICON_ALBUM),
        format!(" {} Playlists ", ICON_MUSIC),
        format!(" {} Downloader ", ICON_SEARCH),
        format!(" {} Metadata ", ICON_FOLDER)
    ];
    let tabs = Tabs::new(tab_titles)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Prayer Music Suite ").title_alignment(Alignment::Center).border_style(Style::default().fg(theme)))
        .select(match app.active_tab { ActiveTab::Player => 0, ActiveTab::Albums => 1, ActiveTab::Playlists => 2, ActiveTab::Downloader => 3, ActiveTab::Metadata => 4 })
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(theme).add_modifier(Modifier::BOLD));
    f.render_widget(tabs, main_chunks[0]);

    if app.is_focused_mode {
        draw_focused_mode(f, app, main_chunks[1], theme);
    } else {
        match app.active_tab {
            ActiveTab::Player => draw_player_tab(f, app, main_chunks[1], theme),
            ActiveTab::Albums => draw_albums_tab(f, app, main_chunks[1], theme),
            ActiveTab::Playlists => draw_playlists_tab(f, app, main_chunks[1], theme),
            ActiveTab::Downloader => draw_downloader_tab(f, app, main_chunks[1], theme),
            ActiveTab::Metadata => draw_metadata_tab(f, app, main_chunks[1], theme),
        }
    }

    let time = app.current_playback_time().as_secs();
    let status = if app.sink.is_paused() { format!(" {} PAUSED ", ICON_PAUSE) } else if !app.sink.empty() { format!(" {} PLAYING ", ICON_PLAY) } else { format!(" {} STOPPED ", ICON_STOP) };
    let title = if let Some(track) = &app.current_track {
        let scroller = (app.tick_count / 10) as usize;
        let mut msg = format!("{} - {}", track.title, track.artist);
        if msg.len() > 40 && app.config.enable_animations {
            let offset = scroller % (msg.len() + 10);
            msg = format!("{}      {}", msg, msg);
            msg = msg.chars().skip(offset).take(40).collect();
        }
        format!(" {} | {} ", status, msg)
    } else { status.to_string() };

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme)))
        .gauge_style(Style::default().fg(theme).bg(Color::Black))
        .percent(0).label(format!("{} | {:02}:{:02} | [?] Help", title, time / 60, time % 60));
    f.render_widget(gauge, main_chunks[2]);

    if app.modal != Modal::None { draw_modal(f, app, theme); }
}

fn draw_player_tab(f: &mut Frame, app: &mut App, area: Rect, theme: Color) {
    let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);
    let columns = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(25), Constraint::Percentage(25), Constraint::Percentage(25), Constraint::Percentage(25)]).split(chunks[0]);
    let active_style = Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD);

    let artist_items: Vec<ListItem> = app.artists.iter().map(|a| ListItem::new(format!("{} {}", ICON_USER, a))).collect();
    let artist_list = List::new(artist_items).block(create_block("Artists", app.focus == Focus::Artist, theme))
        .highlight_style(if app.focus == Focus::Artist { active_style } else { Style::default().bg(Color::DarkGray) });
    f.render_stateful_widget(artist_list, columns[0], &mut app.artist_state);

    let mut album_items = Vec::new();
    if let Some(artist_idx) = app.artist_state.selected() {
        if let Some(albums) = app.albums_by_artist.get(&app.artists[artist_idx]) {
            album_items = albums.iter().map(|a| ListItem::new(format!("{} {}", ICON_ALBUM, a))).collect();
        }
    }
    let album_list = List::new(album_items).block(create_block("Albums", app.focus == Focus::Album, theme))
        .highlight_style(if app.focus == Focus::Album { active_style } else { Style::default().bg(Color::DarkGray) });
    f.render_stateful_widget(album_list, columns[1], &mut app.album_state);

    let mut track_items = Vec::new();
    if let Some(artist_idx) = app.artist_state.selected() {
        if let Some(album_idx) = app.album_state.selected() {
            let artist = &app.artists[artist_idx];
            if let Some(album) = app.albums_by_artist.get(artist).and_then(|albums| albums.get(album_idx)) {
                if let Some(tracks) = app.tracks_by_album.get(&(artist.clone(), album.clone())) {
                    track_items = tracks.iter().map(|t| {
                        let prefix = if t.track_number > 0 { format!("{:02}. ", t.track_number) } else { "".to_string() };
                        ListItem::new(format!("{} {}{}", ICON_MUSIC, prefix, t.title))
                    }).collect();
                }
            }
        }
    }
    let track_list = List::new(track_items).block(create_block("Tracks", app.focus == Focus::Track, theme))
        .highlight_style(if app.focus == Focus::Track { active_style } else { Style::default().bg(Color::DarkGray) });
    f.render_stateful_widget(track_list, columns[2], &mut app.track_state);

    let queue_items: Vec<ListItem> = app.queue.iter().map(|t| ListItem::new(format!("{} {}", ICON_QUEUE, t.title))).collect();
    let queue_list = List::new(queue_items).block(create_block("Queue", app.focus == Focus::Queue, theme))
        .highlight_style(if app.focus == Focus::Queue { active_style } else { Style::default().bg(Color::DarkGray) });
    f.render_stateful_widget(queue_list, columns[3], &mut app.queue_state);

    let bot_columns = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(30), Constraint::Percentage(40), Constraint::Percentage(30)]).split(chunks[1]);

    let mut info_text = vec![];
    if let Some(track) = &app.current_track {
        info_text.push(Line::from(vec![Span::styled("File Name: ", Style::default().fg(theme)), Span::raw(track.path.file_name().unwrap_or_default().to_string_lossy())]));
        info_text.push(Line::from(vec![Span::styled("Type: ", Style::default().fg(theme)), Span::raw(track.file_type.clone())]));
        info_text.push(Line::from(vec![Span::styled("Size: ", Style::default().fg(theme)), Span::raw(format!("{:.2} MB", track.file_size as f64 / 1024.0 / 1024.0))]));
        info_text.push(Line::from(vec![Span::styled("Bitrate: ", Style::default().fg(theme)), Span::raw(format!("{} kbps", app.current_bitrate))]));
        info_text.push(Line::from(vec![Span::styled("Sample Rate: ", Style::default().fg(theme)), Span::raw(format!("{} Hz", app.current_sample_rate))]));
    } else { info_text.push(Line::from("No track playing.")); }
    let info_block = Paragraph::new(info_text).wrap(Wrap { trim: true }).block(create_block("File Info", false, theme));
    f.render_widget(info_block, bot_columns[0]);

    let lyrics_block = create_block("Synced Lyrics", false, Color::Magenta);
    let inner_lyrics_area = lyrics_block.inner(bot_columns[1]);
    f.render_widget(lyrics_block, bot_columns[1]);

    let mut lyrics_text = vec![];
    if let Some(err) = &app.error_msg { lyrics_text.push(Line::from(Span::styled(err, Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)))); }
    else if let Some(track) = &app.current_track {
        let current_time = app.current_playback_time();
        if track.lyrics.is_empty() { lyrics_text.push(Line::from(Span::styled("No .lrc file found.", Style::default().fg(Color::DarkGray)))); }
        else {
            let mut active_idx = 0;
            for (i, line) in track.lyrics.iter().enumerate() { if current_time >= line.timestamp { active_idx = i; } else { break; } }
            let display_lines = inner_lyrics_area.height as usize;
            let start = active_idx.saturating_sub(display_lines / 2);
            let end = (start + display_lines).min(track.lyrics.len());
            for (i, line) in track.lyrics[start..end].iter().enumerate() {
                let actual_i = start + i;
                if actual_i == active_idx { lyrics_text.push(Line::from(Span::styled(format!("▶ {}", line.text), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))); } 
                else { lyrics_text.push(Line::from(Span::styled(format!("  {}", line.text), Style::default().fg(Color::Gray)))); }
            }
        }
    }
    let pad_lines = inner_lyrics_area.height.saturating_sub(lyrics_text.len() as u16);
    let lyrics_container = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(pad_lines), Constraint::Min(0)]).split(inner_lyrics_area);
    f.render_widget(Paragraph::new(lyrics_text).alignment(Alignment::Center), lyrics_container[1]);

    let cover_block = create_block("Cover Art", false, Color::Green);
    let inner_cover_area = cover_block.inner(bot_columns[2]);
    f.render_widget(cover_block, bot_columns[2]);
    if let Some(ref mut protocol) = app.current_cover {
        let image = ratatui_image::StatefulImage::default().resize(ratatui_image::Resize::Fit(None));
        f.render_stateful_widget(image, inner_cover_area, &mut **protocol);
    } else { f.render_widget(Paragraph::new("No cover art.").alignment(Alignment::Center), inner_cover_area); }
}

fn draw_albums_tab(f: &mut Frame, app: &mut App, area: Rect, theme: Color) {
    let chunks = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);
    let album_items: Vec<ListItem> = app.all_albums.iter().map(|(artist, album)| ListItem::new(format!("{} {} - {} {}", ICON_ALBUM, album, ICON_USER, artist))).collect();
    let list = List::new(album_items).block(create_block("All Albums", app.focus == Focus::AlbumsView, theme))
        .highlight_style(if app.focus == Focus::AlbumsView { Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD) } else { Style::default() });
    f.render_stateful_widget(list, chunks[0], &mut app.albums_view_state);

    let cover_block = create_block("Album Art Preview", false, Color::Green);
    let inner_cover = cover_block.inner(chunks[1]);
    f.render_widget(cover_block, chunks[1]);
    if let Some(ref mut protocol) = app.hover_cover {
        let image = ratatui_image::StatefulImage::default().resize(ratatui_image::Resize::Fit(None));
        f.render_stateful_widget(image, inner_cover, &mut **protocol);
    } else { f.render_widget(Paragraph::new("Hover to see cover art.").alignment(Alignment::Center), inner_cover); }
}

fn draw_playlists_tab(f: &mut Frame, app: &mut App, area: Rect, theme: Color) {
    let chunks = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(30), Constraint::Percentage(70)]).split(area);

    let mut pl_names: Vec<String> = app.playlists.lists.keys().cloned().collect();
    pl_names.sort();
    let pl_items: Vec<ListItem> = pl_names.iter().map(|name| ListItem::new(format!("{} {}", ICON_MUSIC, name))).collect();
    let pl_list = List::new(pl_items).block(create_block("Playlists", app.focus == Focus::PlaylistsList, theme))
        .highlight_style(if app.focus == Focus::PlaylistsList { Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD) } else { Style::default() });
    f.render_stateful_widget(pl_list, chunks[0], &mut app.playlists_list_state);

    let mut track_items = Vec::new();
    if let Some(idx) = app.playlists_list_state.selected() {
        if let Some(pl_name) = pl_names.get(idx) {
            if let Some(paths) = app.playlists.lists.get(pl_name) {
                for path in paths {
                    if let Some(track) = app.all_tracks.iter().find(|t| &t.path == path) {
                        track_items.push(ListItem::new(format!("{} {} - {}", ICON_MUSIC, track.title, track.artist)));
                    }
                }
            }
        }
    }
    
    let t_list = List::new(track_items).block(create_block("Tracks", app.focus == Focus::PlaylistsTracks, theme))
        .highlight_style(if app.focus == Focus::PlaylistsTracks { Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD) } else { Style::default() });
    f.render_stateful_widget(t_list, chunks[1], &mut app.playlists_track_state);
}

fn draw_focused_mode(f: &mut Frame, app: &mut App, area: Rect, theme: Color) {
    let chunks = Layout::default().direction(Direction::Horizontal).constraints([
        Constraint::Percentage(25), Constraint::Percentage(50), Constraint::Percentage(25)
    ]).split(area);

    let left_block = create_block(if app.queue.is_empty() { "Current Album" } else { "Queue" }, false, theme);
    if !app.queue.is_empty() {
        let queue_items: Vec<ListItem> = app.queue.iter().map(|t| ListItem::new(format!("{} {}", ICON_QUEUE, t.title))).collect();
        let queue_list = List::new(queue_items).block(left_block).highlight_style(Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(queue_list, chunks[0], &mut app.queue_state);
    } else {
        let mut track_items = Vec::new();
        let mut album_state = ListState::default();
        if let Some(current) = &app.current_track {
            let mut found_tracks = None;
            for ((a, _), t_list) in &app.tracks_by_album {
                if a == &current.artist {
                    if t_list.iter().any(|t| t.path == current.path) {
                        found_tracks = Some(t_list);
                        break;
                    }
                }
            }
            if let Some(t_list) = found_tracks {
                for (i, t) in t_list.iter().enumerate() {
                    let prefix = if t.track_number > 0 { format!("{:02}. ", t.track_number) } else { "".to_string() };
                    let icon = if t.path == current.path { ICON_PLAY } else { ICON_MUSIC };
                    track_items.push(ListItem::new(format!("{} {}{}", icon, prefix, t.title)));
                    if t.path == current.path { album_state.select(Some(i)); }
                }
            }
        }
        let list = List::new(track_items).block(left_block).highlight_style(Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list, chunks[0], &mut album_state);
    }

    let lyrics_block = create_block("Lyrics", false, Color::Magenta);
    let inner_lyrics = lyrics_block.inner(chunks[1]);
    f.render_widget(lyrics_block, chunks[1]);
    let mut lyrics_text = vec![];
    if let Some(err) = &app.error_msg { lyrics_text.push(Line::from(Span::styled(err, Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)))); }
    else if let Some(track) = &app.current_track {
        let current_time = app.current_playback_time();
        if track.lyrics.is_empty() { lyrics_text.push(Line::from(Span::styled("No .lrc file found.", Style::default().fg(Color::DarkGray)))); }
        else {
            let mut active_idx = 0;
            for (i, line) in track.lyrics.iter().enumerate() { if current_time >= line.timestamp { active_idx = i; } else { break; } }
            let display_lines = inner_lyrics.height as usize;
            let start = active_idx.saturating_sub(display_lines / 2);
            let end = (start + display_lines).min(track.lyrics.len());
            for (i, line) in track.lyrics[start..end].iter().enumerate() {
                let actual_i = start + i;
                if actual_i == active_idx { lyrics_text.push(Line::from(Span::styled(format!("▶ {}", line.text), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))); } 
                else { lyrics_text.push(Line::from(Span::styled(format!("  {}", line.text), Style::default().fg(Color::Gray)))); }
            }
        }
    }
    let pad_lines = inner_lyrics.height.saturating_sub(lyrics_text.len() as u16);
    let lyrics_container = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(pad_lines), Constraint::Min(0)]).split(inner_lyrics);
    f.render_widget(Paragraph::new(lyrics_text).alignment(Alignment::Center), lyrics_container[1]);

    let right_chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(60), Constraint::Percentage(40)]).split(chunks[2]);
    let cover_block = create_block("Album Art", false, Color::Green);
    let inner_cover = cover_block.inner(right_chunks[0]);
    f.render_widget(cover_block, right_chunks[0]);
    if let Some(ref mut protocol) = app.current_cover {
        let image = ratatui_image::StatefulImage::default().resize(ratatui_image::Resize::Fit(None));
        f.render_stateful_widget(image, inner_cover, &mut **protocol);
    } else { f.render_widget(Paragraph::new("No cover art.").alignment(Alignment::Center), inner_cover); }

    let mut info_text = vec![];
    if let Some(track) = &app.current_track {
        info_text.push(Line::from(vec![Span::styled("File Name: ", Style::default().fg(theme)), Span::raw(track.path.file_name().unwrap_or_default().to_string_lossy())]));
        info_text.push(Line::from(vec![Span::styled("Type: ", Style::default().fg(theme)), Span::raw(track.file_type.clone())]));
        info_text.push(Line::from(vec![Span::styled("Size: ", Style::default().fg(theme)), Span::raw(format!("{:.2} MB", track.file_size as f64 / 1024.0 / 1024.0))]));
        info_text.push(Line::from(vec![Span::styled("Bitrate: ", Style::default().fg(theme)), Span::raw(format!("{} kbps", app.current_bitrate))]));
        info_text.push(Line::from(vec![Span::styled("Sample Rate: ", Style::default().fg(theme)), Span::raw(format!("{} Hz", app.current_sample_rate))]));
    } else { info_text.push(Line::from("No track playing.")); }
    let info_block = Paragraph::new(info_text).wrap(Wrap { trim: true }).block(create_block("File Info", false, theme));
    f.render_widget(info_block, right_chunks[1]);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default().direction(Direction::Vertical).constraints([
        Constraint::Percentage((100 - percent_y) / 2), Constraint::Percentage(percent_y), Constraint::Percentage((100 - percent_y) / 2)
    ]).split(r);
    Layout::default().direction(Direction::Horizontal).constraints([
        Constraint::Percentage((100 - percent_x) / 2), Constraint::Percentage(percent_x), Constraint::Percentage((100 - percent_x) / 2)
    ]).split(popup_layout[1])[1]
}

fn draw_modal(f: &mut Frame, app: &mut App, theme: Color) {
    if app.modal == Modal::Search {
        let area = centered_rect(60, 60, f.area());
        f.render_widget(Clear, area);
        let block = Block::default().title(" Global Search (Ctrl+S) ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme));
        let inner = block.inner(area);
        f.render_widget(block, area);
        
        let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(3), Constraint::Min(0)]).split(inner);
        let input_block = Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(theme));
        let input_text = Paragraph::new(format!("> {}_", app.search_query)).block(input_block);
        f.render_widget(input_text, chunks[0]);

        let items: Vec<ListItem> = app.search_results.iter().map(|t| {
            let prefix = if t.track_number > 0 { format!("{:02}. ", t.track_number) } else { "".to_string() };
            ListItem::new(format!("{} {}{} - {} ({})", ICON_MUSIC, prefix, t.title, t.artist, t.album))
        }).collect();
        let list = List::new(items).highlight_style(Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list, chunks[1], &mut app.search_state);
    } else if app.modal == Modal::PlaylistSelect {
        let area = centered_rect(40, 50, f.area());
        f.render_widget(Clear, area);
        let block = Block::default().title(" Select Playlist ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut items = vec![ListItem::new(Span::styled("[+ Create New Playlist]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))];
        let mut pl_names: Vec<String> = app.playlists.lists.keys().cloned().collect();
        pl_names.sort();
        for name in pl_names {
            items.push(ListItem::new(name));
        }
        let list = List::new(items).highlight_style(Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list, inner, &mut app.playlist_select_state);

    } else if app.modal == Modal::PlaylistCreate {
        let area = centered_rect(40, 10, f.area());
        f.render_widget(Clear, area);
        let block = Block::default().title(" New Playlist Name ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let input_text = Paragraph::new(format!("> {}_", app.new_playlist_name));
        f.render_widget(input_text, inner);

    } else if app.modal == Modal::EditMetadata {
        let req_height = if app.is_bulk_edit { 11 } else { 20 };
        let percent_y = ((req_height as f32 / f.area().height.max(1) as f32) * 100.0) as u16;
        let area = centered_rect(80, percent_y.clamp(10, 100), f.area());
        f.render_widget(Clear, area);
        let title_text = if app.is_bulk_edit { " Bulk Edit Album (Enter to Save) " } else { " Edit Metadata (Enter to Save) " };
        let block = Block::default().title(title_text).borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let fields = if app.is_bulk_edit {
            vec![
                ("Artist", &app.edit_artist, 0),
                ("Album", &app.edit_album, 1),
                ("Year", &app.edit_year, 2),
            ]
        } else {
            vec![
                ("Title", &app.edit_title, 0),
                ("Artist", &app.edit_artist, 1),
                ("Album", &app.edit_album, 2),
                ("Year", &app.edit_year, 3),
                ("Track Number", &app.edit_track, 4),
                ("Filename", &app.edit_filename, 5),
            ]
        };

        let constraints: Vec<Constraint> = fields.iter().map(|_| Constraint::Length(3)).collect();
        let chunks = Layout::default().direction(Direction::Vertical).constraints(constraints).split(inner);

        for (label, val, idx) in fields {
            let mut text = format!("{}", val);
            if app.edit_focus == idx { text.push('█'); }
            let style = if app.edit_focus == idx { Style::default().fg(theme) } else { Style::default() };
            
            let inner_width = chunks[idx].width.saturating_sub(2);
            let text_len = text.chars().count() as u16;
            let scroll_x = if text_len > inner_width { text_len - inner_width } else { 0 };
            
            let p = Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title(label).border_style(style))
                .scroll((0, scroll_x));
            f.render_widget(p, chunks[idx]);
        }
    } else if app.modal == Modal::Help {
        let area = centered_rect(50, 70, f.area());
        f.render_widget(Clear, area);
        let block = Block::default().title(" Help & Shortcuts ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(Color::Yellow));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let help_text = vec![
            Line::from(vec![Span::styled("Navigation:", Style::default().add_modifier(Modifier::BOLD))]),
            Line::from("  h/j/k/l or Arrows : Move Focus / Select"),
            Line::from("  Tab / Shift+Tab   : Cycle Focus (Panes)"),
            Line::from("  Ctrl+Tab or 1/2/3 : Cycle Tabs"),
            Line::from(""),
            Line::from(vec![Span::styled("Playback:", Style::default().add_modifier(Modifier::BOLD))]),
            Line::from("  Enter : Play selected track (or Search result)"),
            Line::from("  Space : Play/Pause (or Play selected Album in Albums View)"),
            Line::from("  a     : Toggle Add/Remove track to queue"),
            Line::from("  Alt+a : Add track to a playlist"),
            Line::from("  d     : Remove track from playlist"),
            Line::from("  n / . : Play Next track"),
            Line::from("  p / , : Play Previous track"),
            Line::from("  - / + : Volume Down / Volume Up"),
            Line::from(""),
            Line::from(vec![Span::styled("Features:", Style::default().add_modifier(Modifier::BOLD))]),
            Line::from("  Alt+Shift+N : Toggle Focused Playing Mode"),
            Line::from("  Ctrl+s      : Open Search"),
            Line::from("  ?           : Show Help"),
            Line::from("  q           : Quit application"),
            Line::from(""),
            Line::from(vec![Span::styled("Press Esc or ? to close this window.", Style::default().fg(Color::DarkGray))]),
        ];
        f.render_widget(Paragraph::new(help_text).wrap(Wrap { trim: false }).alignment(Alignment::Left), inner);
    } else if app.modal == Modal::MoveAlbum {
        let area = centered_rect(80, 15, f.area());
        f.render_widget(Clear, area);
        let block = Block::default().title(" Move Album To... (Enter to Move) ").borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let text = app.move_album_dest.clone() + "█";
        let inner_width = inner.width;
        let text_len = text.chars().count() as u16;
        let scroll_x = if text_len > inner_width { text_len - inner_width } else { 0 };
        
        let p = Paragraph::new(text).scroll((0, scroll_x));
        f.render_widget(p, inner);
    }
}

fn draw_downloader_tab(f: &mut Frame, app: &mut App, area: Rect, theme: Color) {
    let chunks = Layout::default().direction(Direction::Vertical).constraints([
        Constraint::Length(3),
        Constraint::Min(0),
    ]).split(area);

    let search_title = if app.downloader.search_albums_only { " Search (Albums Only) (Press / to type, a to toggle) " } else { " Search (Tracks & Playlists) (Press / to type, a to toggle) " };
    let search_block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(search_title).border_style(Style::default().fg(theme));
    
    let mut search_text = format!("> {}", app.downloader.query);
    if app.modal == Modal::DownloaderSearch && app.active_tab == ActiveTab::Downloader {
        search_text.push('█'); // Cursor block
    }
    
    let status = app.downloader.status.lock().unwrap();
    let search_para = Paragraph::new(format!("{} | {}", search_text, *status)).block(search_block);
    f.render_widget(search_para, chunks[0]);

    let body_chunks = Layout::default().direction(Direction::Horizontal).constraints([
        Constraint::Percentage(50), Constraint::Percentage(50)
    ]).split(chunks[1]);

    let results = app.downloader.results.lock().unwrap();
    let items: Vec<ListItem> = results.iter().enumerate().map(|(i, r)| {
        let prefix = if i == app.downloader.selected_index { ">>" } else { "  " };
        let checkbox = if app.downloader.selected_results.contains(&i) { "[x]" } else { "[ ]" };
        let title = r.title.as_deref().unwrap_or("Unknown Title");
        let uploader = r.uploader.as_deref().unwrap_or("Unknown Uploader");
        let duration = r.duration.unwrap_or(0.0) as u64;
        let style = if i == app.downloader.selected_index { Style::default().fg(theme).add_modifier(Modifier::BOLD) } else { Style::default() };
        ListItem::new(format!("{} {} {} - {} [{:02}:{:02}]", prefix, checkbox, uploader, title, duration / 60, duration % 60)).style(style)
    }).collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Results (s to select, d to download) ").border_style(Style::default().fg(theme)));
    f.render_widget(list, body_chunks[0]);

    let logs = app.downloader.download_log.lock().unwrap();
    let log_items: Vec<ListItem> = logs.iter().map(|l| ListItem::new(l.as_str())).collect();
    let log_list = List::new(log_items).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Download Log ").border_style(Style::default().fg(theme)));
    let mut state = ListState::default();
    if !logs.is_empty() { state.select(Some(logs.len().saturating_sub(1))); }
    f.render_stateful_widget(log_list, body_chunks[1], &mut state);
}

fn draw_metadata_tab(f: &mut Frame, app: &mut App, area: Rect, theme: Color) {
    let chunks = Layout::default().direction(Direction::Vertical).constraints([
        Constraint::Length(3),
        Constraint::Min(0),
    ]).split(area);

    let status_block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(theme));
    let mut status_msg = app.metadata_editor.status.clone();
    let is_loading = *app.metadata_editor.is_loading.lock().unwrap();
    if is_loading { status_msg = format!("{} [WAIT]", status_msg); }
    else { status_msg = format!("{} | l: fetch lyrics | m: move album to library", status_msg); }
    
    f.render_widget(Paragraph::new(status_msg).block(status_block), chunks[0]);

    let columns = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(30), Constraint::Percentage(70)]).split(chunks[1]);
    
    let active_style = Style::default().bg(Color::DarkGray).fg(theme).add_modifier(Modifier::BOLD);
    
    let album_items: Vec<ListItem> = app.metadata_editor.albums.iter().enumerate().map(|(i, a)| {
        let prefix = if i == app.metadata_editor.selected_album { ">>" } else { "  " };
        let style = if i == app.metadata_editor.selected_album && app.focus == Focus::MetadataAlbums { active_style } else { Style::default() };
        ListItem::new(format!("{} {} ({} tracks)", prefix, a.name, a.tracks.len())).style(style)
    }).collect();
    
    let album_list = List::new(album_items).block(create_block(" Albums ", app.focus == Focus::MetadataAlbums, theme));
    f.render_widget(album_list, columns[0]);

    let mut track_items = Vec::new();
    if let Some(album) = app.metadata_editor.albums.get(app.metadata_editor.selected_album) {
        track_items = album.tracks.iter().enumerate().map(|(i, t)| {
            let prefix = if i == app.metadata_editor.selected_track { ">>" } else { "  " };
            let style = if i == app.metadata_editor.selected_track && app.focus == Focus::MetadataTracks { active_style } else { Style::default() };
            ListItem::new(format!("{} [{}] {} - {} | Year: {}", prefix, t.track_number, t.artist, t.title, t.year)).style(style)
        }).collect();
    }
    
    let track_list = List::new(track_items).block(create_block(" Tracks (Press 'e' to edit) ", app.focus == Focus::MetadataTracks, theme));
    f.render_widget(track_list, columns[1]);
}

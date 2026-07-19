use std::process::Command;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone, Debug, Deserialize)]
pub struct YtResult {
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub id: Option<String>,
    pub duration: Option<f64>,
}

pub struct DownloaderState {
    pub query: String,
    pub results: Arc<Mutex<Vec<YtResult>>>,
    pub status: Arc<Mutex<String>>,
    pub is_searching: Arc<Mutex<bool>>,
    pub is_downloading: Arc<Mutex<bool>>,
    pub tmp_dir: String,
    pub selected_index: usize,
}

impl DownloaderState {
    pub fn new(tmp_dir: String) -> Self {
        Self {
            query: String::new(),
            results: Arc::new(Mutex::new(Vec::new())),
            status: Arc::new(Mutex::new("Idle. Press '/' to search, 's' to download selected.".to_string())),
            is_searching: Arc::new(Mutex::new(false)),
            is_downloading: Arc::new(Mutex::new(false)),
            tmp_dir,
            selected_index: 0,
        }
    }

    pub fn search(&mut self, query: String) {
        self.query = query.clone();
        *self.status.lock().unwrap() = format!("Searching for '{}'...", query);
        *self.is_searching.lock().unwrap() = true;
        self.results.lock().unwrap().clear();
        self.selected_index = 0;

        let results_arc = self.results.clone();
        let status_arc = self.status.clone();
        let is_searching_arc = self.is_searching.clone();

        thread::spawn(move || {
            let output = Command::new("yt-dlp")
                .arg(format!("ytsearch15:{}", query))
                .arg("--dump-json")
                .arg("--no-playlist")
                .output();

            let mut parsed_results = Vec::new();
            if let Ok(output) = output {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Ok(item) = serde_json::from_str::<YtResult>(line) {
                        parsed_results.push(item);
                    }
                }
                *status_arc.lock().unwrap() = format!("Found {} results for '{}'.", parsed_results.len(), query);
            } else {
                *status_arc.lock().unwrap() = "Error running yt-dlp. Is it installed?".to_string();
            }

            *results_arc.lock().unwrap() = parsed_results;
            *is_searching_arc.lock().unwrap() = false;
        });
    }

    pub fn download_selected(&mut self) {
        let results = self.results.lock().unwrap();
        if self.selected_index >= results.len() { return; }
        
        let item = &results[self.selected_index];
        let id = match &item.id {
            Some(i) => i.clone(),
            None => return,
        };
        let title = item.title.clone().unwrap_or_else(|| "Unknown".to_string());
        
        *self.status.lock().unwrap() = format!("Downloading '{}'...", title);
        *self.is_downloading.lock().unwrap() = true;
        
        let tmp_dir = self.tmp_dir.clone();
        let status_arc = self.status.clone();
        let is_downloading_arc = self.is_downloading.clone();
        
        thread::spawn(move || {
            let output = Command::new("yt-dlp")
                .arg("-f").arg("bestaudio[ext=m4a]/bestaudio")
                .arg("--extract-audio").arg("--audio-format").arg("m4a")
                .arg("--cookies-from-browser").arg("firefox")
                .arg("-o").arg(format!("{}/%(artist)s/(%(release_year)s) - %(album)s/%(track_number)s - %(title)s.%(ext)s", tmp_dir))
                .arg("--parse-metadata").arg("title:%(artist)s - %(title)s")
                .arg("--parse-metadata").arg("uploader:%(artist)s")
                .arg(format!("https://www.youtube.com/watch?v={}", id))
                .output();
                
            if let Ok(output) = output {
                if output.status.success() {
                    *status_arc.lock().unwrap() = format!("Successfully downloaded '{}'.", title);
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    *status_arc.lock().unwrap() = format!("Failed to download '{}': {}", title, err);
                }
            } else {
                *status_arc.lock().unwrap() = format!("Failed to run yt-dlp for '{}'.", title);
            }
            
            *is_downloading_arc.lock().unwrap() = false;
        });
    }
}


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
    pub selected_results: std::collections::HashSet<usize>,
    pub status: Arc<Mutex<String>>,
    pub is_searching: Arc<Mutex<bool>>,
    pub is_downloading: Arc<Mutex<bool>>,
    pub download_finished: Arc<Mutex<bool>>,
    pub download_log: Arc<Mutex<Vec<String>>>,
    pub tmp_dir: String,
    pub selected_index: usize,
    pub cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    pub active_pid: Arc<Mutex<Option<u32>>>,
    pub search_albums_only: bool,
}

impl DownloaderState {
    pub fn new(tmp_dir: std::path::PathBuf) -> Self {
        Self {
            query: String::new(),
            results: Arc::new(Mutex::new(Vec::new())),
            selected_results: std::collections::HashSet::new(),
            status: Arc::new(Mutex::new(String::from("Ready."))),
            is_searching: Arc::new(Mutex::new(false)),
            is_downloading: Arc::new(Mutex::new(false)),
            download_finished: Arc::new(Mutex::new(false)),
            download_log: Arc::new(Mutex::new(Vec::new())),
            tmp_dir: tmp_dir.to_string_lossy().into_owned(),
            selected_index: 0,
            cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            active_pid: Arc::new(Mutex::new(None)),
            search_albums_only: true,
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
        let search_albums = self.search_albums_only;

        thread::spawn(move || {
            let search_arg = if query.starts_with("http") { query.clone() } 
            else if search_albums { format!("https://www.youtube.com/results?search_query={}&sp=EgIQAw%253D%253D", urlencoding::encode(&query)) }
            else { format!("ytsearch15:{}", query) };
            
            let output = Command::new("yt-dlp")
                .arg(&search_arg)
                .arg("--dump-json")
                .arg("--flat-playlist")
                .output();

            let mut parsed_results = Vec::new();
            if let Ok(output) = output {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Ok(item) = serde_json::from_str::<YtResult>(line) {
                        parsed_results.push(item);
                    }
                }
            } else {
                *status_arc.lock().unwrap() = "Error running yt-dlp. Is it installed?".to_string();
            }

            *status_arc.lock().unwrap() = format!("Found {} results for '{}'.", parsed_results.len(), query);

            *results_arc.lock().unwrap() = parsed_results;
            *is_searching_arc.lock().unwrap() = false;
        });
    }

    pub fn toggle_selection(&mut self) {
        let results = self.results.lock().unwrap();
        if self.selected_index < results.len() {
            if self.selected_results.contains(&self.selected_index) {
                self.selected_results.remove(&self.selected_index);
            } else {
                self.selected_results.insert(self.selected_index);
            }
        }
    }

    pub fn download_selected(&mut self) {
        if *self.is_downloading.lock().unwrap() { return; }
        let results = self.results.lock().unwrap();
        
        let mut urls_to_download = Vec::new();
        if !self.selected_results.is_empty() {
            for &idx in &self.selected_results {
                if let Some(r) = results.get(idx) {
                    if let Some(id) = &r.id {
                        let url = if id.starts_with("PL") || id.starts_with("OL") || id.starts_with("RD") {
                            format!("https://www.youtube.com/playlist?list={}", id)
                        } else {
                            format!("https://www.youtube.com/watch?v={}", id)
                        };
                        urls_to_download.push(url);
                    }
                }
            }
        } else {
            if let Some(r) = results.get(self.selected_index) {
                if let Some(id) = &r.id {
                    let url = if id.starts_with("PL") || id.starts_with("OL") || id.starts_with("RD") {
                        format!("https://www.youtube.com/playlist?list={}", id)
                    } else {
                        format!("https://www.youtube.com/watch?v={}", id)
                    };
                    urls_to_download.push(url);
                }
            }
        }
        
        if urls_to_download.is_empty() { return; }

        *self.is_downloading.lock().unwrap() = true;
        *self.download_finished.lock().unwrap() = false;
        self.cancel_flag.store(false, std::sync::atomic::Ordering::SeqCst);
        self.download_log.lock().unwrap().clear();
        *self.status.lock().unwrap() = format!("Downloading {} items...", urls_to_download.len());
        
        let status_arc = self.status.clone();
        let is_downloading_arc = self.is_downloading.clone();
        let download_finished_arc = self.download_finished.clone();
        let log_arc = self.download_log.clone();
        let tmp_dir = self.tmp_dir.clone();
        let cancel_arc = self.cancel_flag.clone();
        let pid_arc = self.active_pid.clone();

        thread::spawn(move || {
            for url in urls_to_download {
                if cancel_arc.load(std::sync::atomic::Ordering::SeqCst) { break; }
                let mut cmd = Command::new("yt-dlp");
                cmd.arg("-f").arg("bestaudio[ext=m4a]/bestaudio")
                   .arg("--force-ipv4")
                   .arg("--extract-audio").arg("--audio-format").arg("m4a")
                   .arg("--embed-metadata")
                   .arg("--cookies-from-browser").arg("firefox")
                   .arg("--write-subs").arg("--sub-format").arg("lrc/srv3/vtt")
                   .arg("-o").arg(format!("{}/%(artist,uploader)s/(%(release_year,upload_date>%Y)s) - %(album,playlist_title,title)s/%(playlist_index&{{}} - |)s%(title)s.%(ext)s", tmp_dir))
                   .arg("--parse-metadata").arg("title:%(artist)s - %(title)s")
                   .arg("--parse-metadata").arg("uploader:%(artist)s")
                   .arg(&url)
                   .stdout(std::process::Stdio::piped())
                   .stderr(std::process::Stdio::piped());
                   
                if let Ok(mut child) = cmd.spawn() {
                    *pid_arc.lock().unwrap() = Some(child.id());
                    let stdout = child.stdout.take().unwrap();
                    let stderr = child.stderr.take().unwrap();
                    
                    let log_arc_out = log_arc.clone();
                    thread::spawn(move || {
                        use std::io::{BufRead, BufReader};
                        let reader = BufReader::new(stdout);
                        for line in reader.lines().flatten() {
                            let mut l = log_arc_out.lock().unwrap();
                            l.push(line.clone());
                            if l.len() > 100 { l.remove(0); }
                        }
                    });
                    
                    let log_arc_err = log_arc.clone();
                    thread::spawn(move || {
                        use std::io::{BufRead, BufReader};
                        let reader = BufReader::new(stderr);
                        for line in reader.lines().flatten() {
                            let mut l = log_arc_err.lock().unwrap();
                            l.push(line.clone());
                            if l.len() > 100 { l.remove(0); }
                        }
                    });
                    
                    let _ = child.wait();
                    *pid_arc.lock().unwrap() = None;
                }
            }

            if !cancel_arc.load(std::sync::atomic::Ordering::SeqCst) {
                // Remux all .m4a files to fix container atom structure for mp4ameta
                for entry in walkdir::WalkDir::new(&tmp_dir).into_iter().filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("m4a") {
                        let temp_path = path.with_extension("m4a.tmp");
                        if let Ok(mut child) = Command::new("ffmpeg")
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
                            } else {
                                let _ = std::fs::remove_file(&temp_path);
                            }
                        }
                    }
                }
            }

            *is_downloading_arc.lock().unwrap() = false;
            *download_finished_arc.lock().unwrap() = true;
            *status_arc.lock().unwrap() = "Download complete.".to_string();
        });
    }

    pub fn cancel_download(&mut self) {
        self.cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(pid) = *self.active_pid.lock().unwrap() {
            let _ = std::process::Command::new("kill").arg("-9").arg(pid.to_string()).output();
        }
    }
}

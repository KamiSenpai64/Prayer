use serde::Deserialize;
use std::process::Command;

#[derive(Debug, Deserialize, Clone)]
pub struct YtResult {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
}

fn main() {
    let output = Command::new("yt-dlp")
        .arg("ytsearch1:slipknot")
        .arg("--dump-json")
        .arg("--no-playlist")
        .output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        match serde_json::from_str::<YtResult>(line) {
            Ok(item) => println!("Success: {:?}", item),
            Err(e) => println!("Error parsing line: {}", e),
        }
    }
}

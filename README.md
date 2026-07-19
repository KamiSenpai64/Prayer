# Prayer

A terminal-based music player and metadata manager written in Rust.

## Features
- **Library Management**: Browse, play, and organize your music library.
- **yt-dlp Downloader**: Built-in youtube/soundcloud search and music downloader with automatic metadata writing.
- **Metadata Editor**: Powerful bulk editor to safely modify ID3/M4A tags for tracks and albums.
- **Playlists**: Create custom playlists and queue tracks seamlessly.
- **Sleek TUI**: Fast, customizable Terminal User Interface built with Ratatui.

## Installation

### Prerequisites
Make sure you have [Rust and Cargo](https://rustup.rs/) installed on your system.
You will also need `yt-dlp` and `ffmpeg` installed on your system for downloading and metadata parsing.

```bash
# Ubuntu/Debian
sudo apt install yt-dlp ffmpeg

# Arch Linux
sudo pacman -S yt-dlp ffmpeg
```

### Install from Source
1. Clone this repository or download the source code:
```bash
git clone https://github.com/yourusername/prayer.git
cd prayer
```
2. Build and install it globally via Cargo:
```bash
cargo install --path .
```
3. Run the application from anywhere by typing `prayer` in your terminal!

## Configuration
Upon running for the first time, Prayer will generate a config file at `~/.config/prayer/config.toml`. You can edit this file to change themes, music directory paths, and animations.

## Keyboard Shortcuts
Press `?` inside the application to see the full list of keyboard shortcuts!

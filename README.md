# TUI Player

A sleek, heavily customized, terminal-based music player built in Rust. Enjoy a visually rich TUI (Terminal User Interface) that brings all your local music alive.

## Features

- **Modern TUI**: Aesthetic, dynamic layout with high-quality styling.
- **Focused Mode (`ALT+SHIFT+N`)**: Instantly enter a distraction-free Dashboard featuring large synced lyrics, queue, file info, and album art layout.
- **Album Art Previews**: Seamless rendering of Album Art directly in the terminal (utilizing compatible terminal capabilities).
- **Playlists Manager**: Create, view, add, and remove tracks from completely localized, persistent playlists.
- **Global Search (`CTRL+S`)**: Instantly search your entire music library by track, artist, album, or year.
- **Synced Lyrics (.lrc)**: Automatically locates and displays synced `.lrc` lyrics in real-time.
- **Automated Queueing**: Smart queue management. When a song finishes, the next song plays automatically. Press `Space` on an album to intelligently queue the entire album.
- **Configurable**: Fully configured via `~/.config/tui_player/config.toml`. Easily change your music directory and primary theme colors!

## Shortcuts

| Keybinding | Action |
| --- | --- |
| `h/j/k/l` or `Arrows` | Move Focus / Select |
| `Tab` / `Shift+Tab` | Cycle Pane Focus |
| `Ctrl+Tab` or `1/2/3` | Cycle Tabs |
| `Enter` | Play selected track |
| `Space` | Play/Pause (or play entire album from Albums tab) |
| `n` / `.` / `>` | Play Next track |
| `p` / `,` / `<` | Play Previous track |
| `-` / `+` | Volume Down / Volume Up |
| `a` | Toggle track in Queue |
| `Alt+a` | Add selected track to a Playlist |
| `d` | Remove track from playlist (or delete playlist) |
| `Alt+Shift+N` | Toggle Focused Playing Mode |
| `Ctrl+S` | Open Global Search |
| `?` | Show Help menu |
| `q` | Quit |

## Installation

Ensure you have Rust installed. Clone the repository and run:

```bash
cargo build --release
```

The compiled binary will be located in `target/release/tui_player`. Run the binary and start listening!

## Configuration

The default music directory is `~/Music`. To change it, edit `~/.config/tui_player/config.toml`:

```toml
music_directory = "/path/to/your/music"
theme_color = "Cyan"
enable_animations = true
```

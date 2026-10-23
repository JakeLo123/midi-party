//! Watches the user's config directory so `keymap.toml` and `theme.toml`
//! take effect the moment they are saved.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigEvent {
    KeymapChanged,
    ThemeChanged,
}

pub struct ConfigWatcher {
    _watcher: Option<RecommendedWatcher>,
    rx: Receiver<ConfigEvent>,
    /// Editors save in bursts; collapse events closer than this.
    last: Option<(ConfigEvent, Instant)>,
}

const DEBOUNCE: Duration = Duration::from_millis(150);

impl ConfigWatcher {
    /// Starts watching `dir` (created if missing). Wakes the UI via `ctx` when
    /// a file changes. If the watcher cannot start, polling simply yields nothing.
    pub fn new(dir: &Path, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("config: cannot create {}: {e}", dir.display());
        }
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                return;
            }
            for path in &event.paths {
                let which = match path.file_name().and_then(|n| n.to_str()) {
                    Some("keymap.toml") => ConfigEvent::KeymapChanged,
                    Some("theme.toml") => ConfigEvent::ThemeChanged,
                    _ => continue,
                };
                if tx.send(which).is_ok() {
                    ctx.request_repaint();
                }
            }
        });
        let watcher = match watcher {
            Ok(mut w) => match w.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => Some(w),
                Err(e) => {
                    eprintln!("config: cannot watch {}: {e}", dir.display());
                    None
                }
            },
            Err(e) => {
                eprintln!("config: watcher unavailable: {e}");
                None
            }
        };
        Self { _watcher: watcher, rx, last: None }
    }

    /// Debounced events since the last call.
    pub fn poll(&mut self) -> Vec<ConfigEvent> {
        let now = Instant::now();
        let mut out = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            let duplicate = self.last.is_some_and(|(e, at)| e == event && now.duration_since(at) < DEBOUNCE);
            if !duplicate && !out.contains(&event) {
                out.push(event);
            }
            self.last = Some((event, now));
        }
        out
    }
}

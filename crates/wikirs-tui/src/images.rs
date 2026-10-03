//! Inline images (tui.md#rendering-work-the-build-needs): pictures of
//! embedded Attachments drawn with the terminal's own image protocol (kitty,
//! iTerm2, sixel) or, failing that, half-block characters.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
};

use ratatui::{Frame, layout::Rect};
use ratatui_image::{
    FontSize, StatefulImage,
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};

/// How images are drawn, and each Attachment's picture once decoded.
pub struct Images {
    picker: Picker,
    /// `None` for a file that isn't an image (or can't be read).
    cache: RefCell<HashMap<PathBuf, Option<StatefulProtocol>>>,
}

impl Images {
    #[must_use]
    pub fn new(picker: Picker) -> Self {
        Self {
            picker,
            cache: RefCell::default(),
        }
    }

    /// Works out what the terminal can show from its environment and pixel
    /// size. It never queries over stdin: a terminal that doesn't answer would
    /// hang startup, and the query's reader could swallow keystrokes.
    #[must_use]
    pub fn detect() -> Self {
        let font_size = ratatui::crossterm::terminal::window_size()
            .ok()
            .filter(|s| s.width > 0 && s.height > 0 && s.columns > 0 && s.rows > 0)
            .map_or(
                FontSize {
                    width: 10,
                    height: 20,
                },
                |s| FontSize {
                    width: s.width / s.columns,
                    height: s.height / s.rows,
                },
            );
        // iTerm2-style terminals and tmux are recognised from the environment.
        // Deprecated in favour of the stdin query, but it's the only way to
        // give the real cell size without one (revisit if a release drops it).
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(font_size);
        if picker.protocol_type() == ProtocolType::Halfblocks && kitty_graphics() {
            picker.set_protocol_type(ProtocolType::Kitty);
        }
        Self::new(picker)
    }

    /// Forgets decoded pictures, e.g. after files changed on disk.
    pub fn clear(&self) {
        self.cache.borrow_mut().clear();
    }

    /// Draws the image at `file` into `area`, scaled to fit. False if it
    /// isn't an image.
    pub fn draw(&self, f: &mut Frame, area: Rect, file: &Path) -> bool {
        let mut cache = self.cache.borrow_mut();
        let picture = cache.entry(file.to_path_buf()).or_insert_with(|| {
            let image = image::ImageReader::open(file)
                .ok()?
                .with_guessed_format()
                .ok()?
                .decode()
                .ok()?;
            Some(self.picker.new_resize_protocol(image))
        });
        match picture {
            Some(picture) => {
                f.render_stateful_widget(StatefulImage::default(), area, picture);
                true
            }
            None => false,
        }
    }
}

/// Terminals that speak the kitty graphics protocol, by their environment.
fn kitty_graphics() -> bool {
    let var = |name| std::env::var(name).unwrap_or_default();
    !var("KITTY_WINDOW_ID").is_empty()
        || var("TERM") == "xterm-kitty"
        || var("TERM_PROGRAM").eq_ignore_ascii_case("ghostty")
}

//! The GUI Interface (docs/spec/gui.md): gpui + gpui-kit over the core, calling
//! typed Operations in-process, with `watch` taken in on the UI thread
//! (interfaces.md#gui-and-tui).
//!
//! [`layout`] turns the core's document model into styled text with Link
//! ranges (no gpui), [`page`] draws that, and [`workbench::Workbench`] is the
//! window, driving the open-Page `Session` the TUI shares.

// gpui is written against its prelude: `use gpui_kit::*`.
#![allow(clippy::wildcard_imports)]

pub mod layout;
pub mod page;
pub mod workbench;

use gpui_kit::{component::Root, *};
use wikirs_core::Wiki;

pub use workbench::Workbench;

/// Binds the Workbench's keys: ⌘S / ctrl-S saves, ⌘E / ctrl-E toggles source.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-s", workbench::Save, None),
        KeyBinding::new("secondary-e", workbench::ToggleSource, None),
    ]);
}

/// Opens a Workbench window on `wiki` and runs until it's closed, opening
/// `page` first if given.
pub fn run(wiki: Wiki, page: Option<String>) {
    application().with_assets(assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("wikirs · {}", wiki.root().display()).into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| {
                    let mut workbench = Workbench::new(wiki, true, window, cx);
                    if let Some(page) = &page {
                        workbench.open(page, window, cx);
                    }
                    workbench
                });
                cx.new(|cx| Root::new(view, window, cx))
            },
        );
        if let Err(err) = opened {
            eprintln!("error: could not open a window: {err}");
            cx.quit();
        }
        cx.activate(true);
        cx.on_window_closed(|cx, _| cx.quit()).detach();
    });
}

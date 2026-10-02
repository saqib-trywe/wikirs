//! The TUI against a real Wiki (testing.md#tui-and-gui): keys in, frames out.
//! Frames are `insta` snapshots of a `TestBackend`; external changes arrive as
//! the watch events the watcher would send.

use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
};
use serde_json::json;
use wikirs_core::{
    Wiki, find,
    watch::{EventKind, WatchEvent},
};
use wikirs_tui::{
    App, Request,
    app::{Focus, Mode},
    ui,
};
use wikirs_ui::Notice;

const PAGES: &[(&str, &str)] = &[
    ("eng", "# Engineering\n\nStart at [[eng/rust]].\n"),
    (
        "eng/rust",
        "# Rust\n\n- [[eng/rust/async|Async]]\n- [[eng/missing]]\n\nfiller\n\nfiller\n\nfiller\n\n## Pinning\n\nPin it. #lang/rust\n",
    ),
    (
        "eng/rust/async",
        "---\ntags: [lang/rust/async]\n---\n# Async notes\n\nSee [[eng/rust#Pinning]] and ![[eng/rust/async/diagram.png]].\n\n| a | b |\n|:--|--:|\n| 1 | **two** |\n\n> [!NOTE]\n> Careful with `Pin`.\n\n- [x] done\n- [ ] todo\n",
    ),
    ("inbox", "# Inbox\n\n- read [[eng/rust/async]]\n"),
];

struct Fixture {
    dir: tempfile::TempDir,
    app: App,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wiki")).unwrap();
        let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
        for (path, content) in PAGES {
            find("create_page")
                .unwrap()
                .call(&wiki, json!({ "path": path, "content": content }))
                .unwrap();
        }
        Self {
            dir,
            app: App::new(wiki),
        }
    }

    fn file(&self, page: &str) -> std::path::PathBuf {
        page.split('/')
            .fold(self.dir.path().join("wiki"), |p, s| p.join(s))
            .with_extension("md")
    }

    fn read(&self, page: &str) -> String {
        std::fs::read_to_string(self.file(page)).unwrap()
    }

    /// Types `keys`: characters as themselves, `<Esc>`, `<Enter>`, `<Tab>`,
    /// `<BS>`, `<Down>`, `<Up>`, `<C-x>` for ctrl-x.
    fn keys(&mut self, keys: &str) {
        let mut rest = keys;
        while let Some(c) = rest.chars().next() {
            let (key, len) = if c == '<'
                && let Some(end) = rest.find('>')
            {
                let name = &rest[1..end];
                let key = match name {
                    "Esc" => KeyEvent::from(KeyCode::Esc),
                    "Enter" => KeyEvent::from(KeyCode::Enter),
                    "Tab" => KeyEvent::from(KeyCode::Tab),
                    "BS" => KeyEvent::from(KeyCode::Backspace),
                    "Down" => KeyEvent::from(KeyCode::Down),
                    "Up" => KeyEvent::from(KeyCode::Up),
                    "Right" => KeyEvent::from(KeyCode::Right),
                    ctrl if ctrl.starts_with("C-") => KeyEvent::new(
                        KeyCode::Char(ctrl.chars().nth(2).unwrap()),
                        KeyModifiers::CONTROL,
                    ),
                    other => panic!("unknown key <{other}>"),
                };
                (key, end + 1)
            } else {
                (KeyEvent::from(KeyCode::Char(c)), c.len_utf8())
            };
            self.app.key(key);
            self.app.pump();
            rest = &rest[len..];
        }
    }

    fn open(&mut self, page: &str) {
        self.app.open(page);
        assert_eq!(self.current(), page);
    }

    fn current(&self) -> &str {
        &self.app.session.page.as_ref().unwrap().path
    }

    fn buffer(&self) -> &str {
        &self.app.session.page.as_ref().unwrap().buffer
    }

    fn banner(&self) -> String {
        match &self.app.session.notice {
            None => String::new(),
            Some(Notice::Info(m) | Notice::Error(m)) => m.clone(),
            Some(Notice::Create(p)) => format!("create {p}"),
            Some(Notice::ChangedOnDisk { .. }) => "changed on disk".into(),
        }
    }

    fn frame(&self) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| ui::draw(f, &self.app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// What the watcher sends when another writer changes `page`.
    fn external(&mut self, kind: EventKind, page: &str) {
        let version = (kind != EventKind::PageDeleted)
            .then(|| wikirs_core::plan::version_of(self.read(page).as_bytes()));
        self.app.on_watch(&WatchEvent {
            kind,
            path: Some(page.to_string()),
            from: None,
            version,
        });
    }
}

// ----------------------------------------------------------------- frames

#[test]
fn page_view() {
    let mut fx = Fixture::new();
    fx.open("eng/rust/async");
    insta::assert_snapshot!(fx.frame());
}

#[test]
fn links_panel_focus_highlights_the_link() {
    let mut fx = Fixture::new();
    fx.open("eng/rust");
    fx.keys("<Tab>j");
    assert_eq!(fx.app.focus, Focus::Links);
    insta::assert_snapshot!(fx.frame());
}

#[test]
fn palette_and_form() {
    let mut fx = Fixture::new();
    fx.open("eng/rust/async");
    fx.keys("<C-k>move");
    insta::assert_snapshot!("palette", fx.frame());
    fx.keys("<Enter>");
    insta::assert_snapshot!("form", fx.frame());
}

#[test]
fn command_line_completes_operation_names() {
    let mut fx = Fixture::new();
    fx.keys(":move_p<Tab>");
    let Mode::Command { line, .. } = &fx.app.mode else {
        panic!("not on the command line")
    };
    assert_eq!(line, "move_page ");
    fx.keys("<BS><BS><BS><BS><BS><Tab>");
    let Mode::Command { line, hint } = &fx.app.mode else {
        panic!()
    };
    assert_eq!(line, "move_");
    assert_eq!(hint.as_deref(), Some("move_page  move_attachment"));
    insta::assert_snapshot!(fx.frame());
}

// ------------------------------------------------------------- navigation

#[test]
fn typing_a_number_follows_that_link() {
    let mut fx = Fixture::new();
    fx.open("eng/rust");
    fx.keys("1");
    assert_eq!(fx.current(), "eng/rust/async");
}

#[test]
fn a_link_with_a_heading_scrolls_to_it() {
    let mut fx = Fixture::new();
    fx.open("eng/rust/async");
    fx.keys("1");
    assert_eq!(fx.current(), "eng/rust");
    assert!(fx.app.scroll >= 10, "not scrolled to Pinning");
}

#[test]
fn following_a_broken_link_offers_to_create_the_page() {
    let mut fx = Fixture::new();
    fx.open("eng/rust");
    fx.keys("2");
    assert_eq!(fx.banner(), "create eng/missing");
    assert_eq!(fx.current(), "eng/rust");
    fx.keys("c");
    assert_eq!(fx.current(), "eng/missing");
    assert!(fx.file("eng/missing").exists());
    // The Link isn't broken any more.
    fx.open("eng/rust");
    assert!(!wikirs_tui::render::is_broken(
        fx.app.session.page.as_ref().unwrap().resolved[1].as_ref()
    ));
}

#[test]
fn an_embedded_attachment_opens_in_the_system_viewer() {
    let mut fx = Fixture::new();
    fx.open("eng/rust/async");
    fx.keys("2");
    assert_eq!(fx.banner(), "No Attachment `eng/rust/async/diagram.png`");
    std::fs::create_dir_all(fx.file("eng/rust/async").with_extension("")).unwrap();
    std::fs::write(
        fx.dir.path().join("wiki/eng/rust/async/diagram.png"),
        b"png",
    )
    .unwrap();
    // Written behind the Index's back: rebuild it, then reopen to re-resolve.
    fx.keys(":rebuild_index<Enter><Esc>");
    fx.open("eng/rust/async");
    fx.keys("2");
    let Some(Request::OpenExternal(path)) = fx.app.take_request() else {
        panic!("no request: {}", fx.banner())
    };
    assert!(path.ends_with("diagram.png"));
}

#[test]
fn tree_selection_opens_pages_and_placeholders_offer_create() {
    let mut fx = Fixture::new();
    let rows: Vec<_> = fx
        .app
        .session
        .tree
        .iter()
        .map(|r| (r.path.as_str(), r.placeholder))
        .collect();
    assert_eq!(
        rows,
        [
            ("eng", false),
            ("eng/rust", false),
            ("eng/rust/async", false),
            ("inbox", false)
        ]
    );
    fx.keys("jjj<Enter>");
    assert_eq!(fx.current(), "inbox");
}

#[test]
fn quick_open_search_and_backlinks() {
    let mut fx = Fixture::new();
    fx.keys("oinbox<Enter>");
    assert_eq!(fx.current(), "inbox");
    // Quick open matches Page Paths as well as Titles ("Async notes").
    fx.keys("orust/as<Enter>");
    assert_eq!(fx.current(), "eng/rust/async");
    fx.keys("/careful<Enter>");
    assert_eq!(fx.current(), "eng/rust/async");
    fx.keys("b");
    insta::assert_snapshot!("backlinks", fx.frame());
    fx.keys("<Down><Enter>");
    assert_eq!(fx.current(), "inbox");
}

// ----------------------------------------------------------------- editing

#[test]
fn inline_edit_then_save_writes_the_page() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys("eNew line. <Esc>");
    assert!(fx.app.session.page.as_ref().unwrap().dirty());
    assert!(fx.frame().contains("● unsaved"));
    fx.keys("<C-s>");
    assert_eq!(fx.banner(), "Saved `inbox`");
    assert_eq!(
        fx.read("inbox"),
        "New line. # Inbox\n\n- read [[eng/rust/async]]\n"
    );
    // Our own save's watch event doesn't look like a change on disk.
    assert!(!fx.app.session.page.as_ref().unwrap().dirty());
}

#[test]
fn editor_hand_off_loads_the_text_into_the_buffer() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys("E");
    assert_eq!(
        fx.app.take_request(),
        Some(Request::Editor(fx.buffer().to_string()))
    );
    fx.app.editor_returned(Ok("# Inbox\n\nedited\n".into()));
    assert_eq!(fx.buffer(), "# Inbox\n\nedited\n");
    assert!(
        fx.app.session.page.as_ref().unwrap().dirty(),
        "loaded, not saved"
    );
    fx.keys("<C-s>");
    assert_eq!(fx.read("inbox"), "# Inbox\n\nedited\n");
}

#[cfg(unix)]
#[test]
fn edit_with_runs_the_editor_on_a_temp_copy() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("ed.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf 'more\\n' >> \"$1\"\n").unwrap();
    std::process::Command::new("chmod")
        .arg("+x")
        .arg(&script)
        .status()
        .unwrap();
    let edited = wikirs_tui::edit_with(script.to_str().unwrap(), "text\n").unwrap();
    assert_eq!(edited, "text\nmore\n");
    assert!(wikirs_tui::edit_with("false", "x").is_err());
}

#[test]
fn quitting_with_unsaved_edits_asks_first() {
    let mut fx = Fixture::new();
    fx.keys("ex<Esc>q");
    assert!(!fx.app.should_quit());
    assert!(fx.banner().starts_with("Unsaved edits"));
    fx.keys("Q");
    assert!(fx.app.should_quit());
}

#[test]
fn navigating_away_keeps_unsaved_edits() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys("ex<Esc>");
    fx.app.open("eng");
    assert_eq!(fx.current(), "inbox");
    fx.keys("D");
    fx.app.open("eng");
    assert_eq!(fx.current(), "eng");
}

// ---------------------------------------------------------- changed on disk

#[test]
fn a_clean_page_reloads_when_changed_on_disk() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    std::fs::write(fx.file("inbox"), "# Inbox\n\nfrom elsewhere\n").unwrap();
    fx.external(EventKind::PageModified, "inbox");
    assert_eq!(fx.buffer(), "# Inbox\n\nfrom elsewhere\n");
    assert!(!fx.app.session.page.as_ref().unwrap().dirty());
}

#[test]
fn unsaved_edits_get_the_banner_and_keep_mine_overwrites() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys("emine <Esc>");
    std::fs::write(fx.file("inbox"), "theirs\n").unwrap();
    fx.external(EventKind::PageModified, "inbox");
    assert_eq!(fx.banner(), "changed on disk");
    assert!(fx.buffer().starts_with("mine "), "buffer kept");
    insta::assert_snapshot!("banner", fx.frame());
    fx.keys("d");
    insta::assert_snapshot!("compare", fx.frame());
    fx.keys("dm");
    assert_eq!(
        fx.read("inbox"),
        "mine # Inbox\n\n- read [[eng/rust/async]]\n"
    );
    assert_eq!(fx.banner(), "Saved `inbox`");
}

#[test]
fn reload_takes_the_disk_version() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys("emine <Esc>");
    std::fs::write(fx.file("inbox"), "theirs\n").unwrap();
    fx.external(EventKind::PageModified, "inbox");
    fx.keys("r");
    assert_eq!(fx.buffer(), "theirs\n");
    assert!(fx.app.session.notice.is_none());
}

#[test]
fn a_stale_save_conflicts_instead_of_overwriting() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys("emine <Esc>");
    // Changed on disk, and the watcher hasn't told us yet.
    std::fs::write(fx.file("inbox"), "theirs\n").unwrap();
    fx.keys("<C-s>");
    assert_eq!(fx.read("inbox"), "theirs\n");
    assert_eq!(fx.banner(), "changed on disk");
}

#[test]
fn a_deleted_page_is_marked_and_saving_recreates_it() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    std::fs::remove_file(fx.file("inbox")).unwrap();
    fx.external(EventKind::PageDeleted, "inbox");
    assert!(fx.app.session.page.as_ref().unwrap().deleted);
    assert!(fx.frame().contains("✗ deleted"));
    fx.keys("<C-s>");
    assert_eq!(fx.read("inbox"), PAGES[3].1);
    assert!(!fx.app.session.page.as_ref().unwrap().deleted);
}

// -------------------------------------------------------------- Operations

#[test]
fn palette_dry_run_shows_the_plan_and_a_applies_it() {
    let mut fx = Fixture::new();
    fx.open("eng/rust/async");
    fx.keys("<C-k>move_page<Enter><Down>eng/async<Enter>");
    insta::assert_snapshot!("plan", fx.frame());
    assert!(
        fx.file("eng/rust/async").exists(),
        "a dry run writes nothing"
    );
    fx.keys("a");
    assert!(fx.file("eng/async").exists());
    // The open Page follows its move (page_moved).
    assert_eq!(fx.current(), "eng/async");
    assert!(fx.read("inbox").contains("[[eng/async]]"));
}

#[test]
fn shift_r_moves_the_open_page() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys("R<Down><BS><BS><BS><BS><BS>todo<Enter>a");
    assert_eq!(fx.current(), "todo");
}

#[test]
fn command_line_runs_operations_cli_style() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys(":tag_page inbox triage --dry-run<Enter>");
    let Mode::Result(popup) = &fx.app.mode else {
        panic!("no result")
    };
    assert!(popup.apply.is_some());
    assert!(!fx.read("inbox").contains("triage"));
    fx.keys("a");
    assert!(fx.read("inbox").contains("triage"));
    // The open Page took the change in (no unsaved edits).
    assert!(
        fx.app
            .session
            .page
            .as_ref()
            .unwrap()
            .tags
            .contains(&"triage".to_string())
    );

    fx.keys("<Esc>:move-page \"inbox\" '../out'<Enter>");
    assert!(fx.frame().contains("error[invalid_path]"), "{}", fx.frame());
    fx.keys("<Esc>:nope<Enter>");
    assert!(fx.frame().contains("unrecognized subcommand"));
}

#[test]
fn watch_shows_the_event_log() {
    let mut fx = Fixture::new();
    fx.open("inbox");
    fx.keys(":tag_page inbox x<Enter><Esc>:watch<Enter>");
    let frame = fx.frame();
    assert!(frame.contains("page_modified inbox"), "{frame}");
}

//! The open-Page session both UIs drive (process-model.md#open-page-changed-on-disk-gui-and-tui).
//! External changes arrive as the watch events the watcher would send.

use serde_json::json;
use wikirs_core::{
    Wiki, find,
    watch::{EventKind, WatchEvent},
};
use wikirs_ui::{Followed, Notice, Session};

struct Fixture {
    dir: tempfile::TempDir,
    session: Session,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("wiki")).unwrap();
        let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
        for (path, content) in [
            (
                "a",
                "# A\n\nTo [[b#Two]], [[missing]] and ![[a/pic.png]].\n",
            ),
            ("b", "# B\n\n## Two\n\nBack to [[a]]. #topic\n"),
            ("c/d", "# D\n"),
        ] {
            find("create_page")
                .unwrap()
                .call(&wiki, json!({ "path": path, "content": content }))
                .unwrap();
        }
        Self {
            dir,
            session: Session::new(wiki),
        }
    }

    fn file(&self, page: &str) -> std::path::PathBuf {
        self.dir.path().join("wiki").join(format!("{page}.md"))
    }

    fn write(&self, page: &str, text: &str) {
        std::fs::write(self.file(page), text).unwrap();
    }

    fn read(&self, page: &str) -> String {
        std::fs::read_to_string(self.file(page)).unwrap()
    }

    fn page(&self) -> &wikirs_ui::OpenPage {
        self.session.page.as_ref().unwrap()
    }

    fn external(&mut self, kind: EventKind, page: &str) {
        let version = (kind != EventKind::PageDeleted)
            .then(|| wikirs_core::plan::version_of(self.read(page).as_bytes()));
        self.session.on_watch(&WatchEvent {
            kind,
            path: Some(page.into()),
            from: None,
            version,
        });
    }
}

#[test]
fn opens_the_first_page_with_what_derives_from_it() {
    let mut fx = Fixture::new();
    let rows: Vec<_> = fx
        .session
        .tree
        .iter()
        .map(|r| (r.path.as_str(), r.placeholder, r.depth))
        .collect();
    assert_eq!(
        rows,
        [
            ("a", false, 0),
            ("b", false, 0),
            ("c", true, 0),
            ("c/d", false, 1)
        ]
    );
    let page = fx.page();
    assert_eq!(page.path, "a");
    assert_eq!(page.backlinks, ["b"]);
    assert_eq!(page.doc.links.len(), 3);
    assert_eq!(page.outline, [(1, "A".to_string())]);
    assert_eq!(
        page.resolved[1].as_ref().unwrap().status,
        wikirs_core::links::LinkStatus::Broken
    );
    assert!(fx.session.open("b"));
    assert_eq!(
        fx.session.page.as_ref().map(|p| p.tags.clone()),
        Some(vec!["topic".to_string()])
    );
    assert_eq!(fx.session.breadcrumb(), ["B"]);
}

#[test]
fn following_links() {
    let mut fx = Fixture::new();
    assert_eq!(
        fx.session.follow(0),
        Followed::Opened {
            heading: Some("Two".into())
        }
    );
    assert_eq!(fx.page().path, "b");
    assert!(fx.session.open("a"));
    assert_eq!(fx.session.follow(1), Followed::Stayed);
    assert_eq!(fx.session.notice, Some(Notice::Create("missing".into())));
    // A broken embed never offers a Page.
    assert_eq!(fx.session.follow(2), Followed::Stayed);
    assert_eq!(
        fx.session.notice,
        Some(Notice::Error("No Attachment `a/pic.png`".into()))
    );
    std::fs::create_dir(fx.dir.path().join("wiki/a")).unwrap();
    std::fs::write(fx.dir.path().join("wiki/a/pic.png"), b"png").unwrap();
    fx.session.run_json("rebuild_index", json!({})).unwrap();
    let Followed::OpenExternal(file) = fx.session.follow(2) else {
        panic!("{:?}", fx.session.notice)
    };
    assert!(file.ends_with("pic.png") && file.exists());

    // Opening a missing Page offers to create it.
    assert!(!fx.session.open("missing"));
    assert_eq!(fx.session.notice, Some(Notice::Create("missing".into())));
    assert_eq!(fx.page().path, "a");
    fx.session.create("missing");
    assert_eq!(fx.page().path, "missing");
    assert!(fx.file("missing").exists());
    assert!(fx.session.tree.iter().any(|r| r.path == "missing"));
}

#[test]
fn unsaved_edits_block_navigation_until_saved_or_discarded() {
    let mut fx = Fixture::new();
    fx.session.set_buffer("# A\n\nedited [[c/d]]\n".into());
    assert!(fx.session.dirty());
    assert_eq!(fx.page().doc.links.len(), 1, "derived from the buffer");
    assert!(!fx.session.open("b"));
    assert_eq!(fx.page().path, "a");
    fx.session.discard();
    assert!(!fx.session.dirty());
    assert!(fx.session.open("b"));
}

#[test]
fn save_writes_with_base_version() {
    let mut fx = Fixture::new();
    fx.session.set_buffer("# A\n\nnew\n".into());
    fx.session.save();
    assert_eq!(fx.read("a"), "# A\n\nnew\n");
    assert!(!fx.session.dirty());
    assert_eq!(fx.session.notice, Some(Notice::Info("Saved `a`".into())));
    // The save took in its own watch event, which changed nothing.
    assert!(fx.session.event_log.iter().any(|e| e == "page_modified a"));
    assert!(!fx.session.pump());
    assert_eq!(fx.session.notice, Some(Notice::Info("Saved `a`".into())));
}

#[test]
fn a_stale_save_conflicts_then_keep_mine_or_reload() {
    let mut fx = Fixture::new();
    fx.session.set_buffer("mine\n".into());
    fx.write("a", "theirs\n");
    fx.session.save();
    assert_eq!(fx.read("a"), "theirs\n", "never overwritten");
    assert!(matches!(
        &fx.session.notice,
        Some(Notice::ChangedOnDisk { disk, .. }) if disk == "theirs\n"
    ));
    fx.session.keep_mine();
    assert_eq!(fx.read("a"), "mine\n");
    assert!(!fx.session.dirty());

    fx.session.set_buffer("mine again\n".into());
    fx.write("a", "theirs again\n");
    fx.session.save();
    fx.session.reload_from_disk();
    assert_eq!(fx.page().buffer, "theirs again\n");
    assert!(!fx.session.dirty());
    assert_eq!(fx.session.notice, None);
}

#[test]
fn changes_on_disk() {
    let mut fx = Fixture::new();
    // Clean: reload without asking.
    fx.write("a", "# A\n\nv2\n");
    fx.external(EventKind::PageModified, "a");
    assert_eq!(fx.page().buffer, "# A\n\nv2\n");
    // Dirty: keep the buffer and ask.
    fx.session.set_buffer("mine\n".into());
    fx.write("a", "v3\n");
    fx.external(EventKind::PageModified, "a");
    assert_eq!(fx.page().buffer, "mine\n");
    assert!(matches!(
        fx.session.notice,
        Some(Notice::ChangedOnDisk { .. })
    ));
    // Deleted: marked, and saving re-creates it.
    std::fs::remove_file(fx.file("a")).unwrap();
    fx.external(EventKind::PageDeleted, "a");
    assert!(fx.page().deleted);
    fx.session.save();
    assert_eq!(fx.read("a"), "mine\n");
    assert!(!fx.page().deleted);
}

#[test]
fn a_wikirs_move_is_followed() {
    let mut fx = Fixture::new();
    fx.session
        .run_json("move_page", json!({ "from": "a", "to": "z/a" }))
        .unwrap();
    assert_eq!(fx.page().path, "z/a");
    assert_eq!(
        fx.session.notice,
        Some(Notice::Info("Moved `a` → `z/a`".into()))
    );
    assert!(
        fx.session
            .event_log
            .iter()
            .any(|e| e == "page_moved a → z/a")
    );
    assert!(fx.session.tree.iter().any(|r| r.path == "z/a"));
}

#[test]
fn queries_for_pickers_and_the_palette() {
    let fx = Fixture::new();
    let pages: Vec<_> = fx.session.all_pages().into_iter().map(|(p, _)| p).collect();
    assert_eq!(pages, ["a", "b", "c/d"]);
    let hits: Vec<_> = fx.session.search("back").into_iter().map(|h| h.0).collect();
    assert_eq!(hits, ["b"]);
    assert!(fx.session.search("  ").is_empty());
    let names: Vec<_> = Session::palette_matches("move page")
        .iter()
        .map(|o| o.name)
        .collect();
    assert_eq!(names, ["move_page"]);
    assert_eq!(
        Session::palette_matches("").len(),
        wikirs_core::registry().len()
    );
    assert_eq!(fx.session.title_of("c/d"), "D");
    assert_eq!(fx.session.title_of("nope"), "nope");
}

//! Headless smoke tests of the Workbench (testing.md#tui-and-gui): a real
//! window on gpui's test platform, driven by clicks, keys and watch events.

use std::fmt::Write as _;

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use serde_json::json;
use wikirs_core::{
    Wiki, find,
    watch::{EventKind, WatchEvent},
};
use wikirs_gui::Workbench;
use wikirs_ui::Notice;

const PAGES: &[(&str, &str)] = &[
    ("a", "# A\n\nTo [[b#Two|B]] and [[gone]].\n"),
    // Made long in `setup`, so that `## Two` is far below the fold.
    ("b", "# B\n\nFILLER\n## Two\n\nBack to [[a]]. #topic\n"),
    // A Link, then a paragraph that is all Link text: a click on the second
    // must follow the second Link, not the first.
    (
        "e",
        "[[a]]\n\n[[b|bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb]]\n",
    ),
    (
        "c/d",
        "# D\n\n| x | y |\n|---|--:|\n| 1 | **2** |\n\n> [!NOTE]\n> n\n\n- [x] t\n\n```rs\nfn f() {}\n```\n\nfoot[^1]\n\n[^1]: note\n",
    ),
];

struct Fixture {
    dir: tempfile::TempDir,
    view: gpui::Entity<Workbench>,
}

fn setup(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
    let filler = (1..=200).fold(String::new(), |mut s, n| {
        let _ = write!(s, "line {n}\n\n");
        s
    });
    for (path, content) in PAGES {
        let content = content.replace("FILLER\n", &filler);
        find("create_page")
            .unwrap()
            .call(&wiki, json!({ "path": path, "content": content }))
            .unwrap();
    }
    cx.update(|cx| {
        gpui_kit::init(cx);
        wikirs_gui::bind_keys(cx);
    });
    let (view, cx) = cx.add_window_view(|window, cx| Workbench::new(wiki, false, window, cx));
    cx.run_until_parked();
    (Fixture { dir, view }, cx)
}

impl Fixture {
    fn path(&self, cx: &mut VisualTestContext) -> String {
        self.view
            .read_with(cx, |wb, _| wb.session.page.as_ref().unwrap().path.clone())
    }

    fn file(&self, page: &str) -> std::path::PathBuf {
        self.dir.path().join("wiki").join(format!("{page}.md"))
    }
}

#[gpui::test]
fn opens_the_first_page_and_renders_every_construct(cx: &mut TestAppContext) {
    let (fx, cx) = setup(cx);
    assert_eq!(fx.path(cx), "a");
    fx.view
        .update_in(cx, |wb, window, cx| wb.open("c/d", window, cx));
    cx.run_until_parked();
    assert_eq!(fx.path(cx), "c/d");
    // Source and preview side by side, and back.
    cx.simulate_keystrokes("cmd-e");
    assert!(fx.view.read_with(cx, |wb, _| wb.source));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-e");
    assert!(!fx.view.read_with(cx, |wb, _| wb.source));
}

#[gpui::test]
fn clicking_a_link_follows_it(cx: &mut TestAppContext) {
    let (fx, cx) = setup(cx);
    let bounds = cx
        .debug_bounds("link-0")
        .expect("the Links panel shows Link 0");
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(fx.path(cx), "b");
    // `[[b#Two]]` scrolls to that heading.
    assert!(fx.view.read_with(cx, |wb, _| wb.scrolled()) > gpui::px(100.));

    // A Broken Link offers to create the Page.
    fx.view
        .update_in(cx, |wb, window, cx| wb.open("a", window, cx));
    cx.run_until_parked();
    let bounds = cx.debug_bounds("link-1").unwrap();
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
    let notice = fx.view.read_with(cx, |wb, _| wb.session.notice.clone());
    assert_eq!(notice, Some(Notice::Create("gone".into())));
    fx.view
        .update_in(cx, |wb, window, cx| wb.create("gone", window, cx));
    cx.run_until_parked();
    assert_eq!(fx.path(cx), "gone");
    assert!(fx.file("gone").exists());
}

#[gpui::test]
fn editing_then_cmd_s_saves(cx: &mut TestAppContext) {
    let (fx, cx) = setup(cx);
    fx.view.update_in(cx, |wb, _, cx| {
        wb.session.set_buffer("# A\n\nedited\n".into());
        cx.notify();
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(fx.file("a")).unwrap(),
        "# A\n\nedited\n"
    );
    assert!(!fx.view.read_with(cx, |wb, _| wb.session.dirty()));
}

#[gpui::test]
fn the_editor_and_the_session_stay_in_step(cx: &mut TestAppContext) {
    let (fx, cx) = setup(cx);
    // Typing in the source editor edits the buffer.
    cx.simulate_keystrokes("cmd-e");
    cx.run_until_parked();
    fx.view.update_in(cx, wikirs_gui::Workbench::focus_editor);
    cx.simulate_input("Hi ");
    cx.run_until_parked();
    let buffer = fx
        .view
        .read_with(cx, |wb, _| wb.session.page.as_ref().unwrap().buffer.clone());
    assert!(buffer.starts_with("Hi "), "{buffer:?}");
    // ⌘S works with the editor focused too.
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert!(
        std::fs::read_to_string(fx.file("a"))
            .unwrap()
            .starts_with("Hi ")
    );
    // After opening another Page, the editor holds that Page's text, so typing
    // edits it (not a copy of the last one).
    fx.view
        .update_in(cx, |wb, window, cx| wb.open("c/d", window, cx));
    cx.run_until_parked();
    fx.view.update_in(cx, Workbench::focus_editor);
    cx.simulate_input("Z");
    cx.run_until_parked();
    let buffer = fx
        .view
        .read_with(cx, |wb, _| wb.session.page.as_ref().unwrap().buffer.clone());
    assert!(
        buffer.contains("# D") && !buffer.contains("# A"),
        "{buffer:?}"
    );
    // Opening another Page is refused while there are unsaved edits.
    cx.simulate_input("x");
    cx.run_until_parked();
    fx.view
        .update_in(cx, |wb, window, cx| wb.open("b", window, cx));
    assert_eq!(fx.path(cx), "c/d");
}

#[gpui::test]
fn changed_on_disk_banner_and_keep_mine(cx: &mut TestAppContext) {
    let (fx, cx) = setup(cx);
    fx.view
        .update_in(cx, |wb, _, _| wb.session.set_buffer("mine\n".into()));
    std::fs::write(fx.file("a"), "theirs\n").unwrap();
    let version = wikirs_core::plan::version_of(b"theirs\n");
    fx.view.update_in(cx, |wb, window, cx| {
        wb.session.on_watch(&WatchEvent {
            kind: EventKind::PageModified,
            path: Some("a".into()),
            from: None,
            version: Some(version),
        });
        wb.changed(window, cx);
    });
    cx.run_until_parked();
    assert!(fx.view.read_with(cx, |wb, _| matches!(
        wb.session.notice,
        Some(Notice::ChangedOnDisk { .. })
    )));
    fx.view.update_in(cx, wikirs_gui::Workbench::keep_mine);
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(fx.file("a")).unwrap(), "mine\n");
}

#[gpui::test]
fn watch_events_arrive_through_the_poll(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
    find("create_page")
        .unwrap()
        .call(&wiki, json!({ "path": "a", "content": "# A\n" }))
        .unwrap();
    cx.update(gpui_kit::init);
    let other = wiki.clone();
    let (view, cx) = cx.add_window_view(|window, cx| Workbench::new(wiki, true, window, cx));
    cx.run_until_parked();
    // Another handle on the same Wiki moves the open Page.
    find("move_page")
        .unwrap()
        .call(&other, json!({ "from": "a", "to": "z" }))
        .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    let path = view.read_with(cx, |wb, _| wb.session.page.as_ref().unwrap().path.clone());
    assert_eq!(path, "z");
}

#[gpui::test]
fn clicking_link_text_in_the_page_follows_it(cx: &mut TestAppContext) {
    let (fx, cx) = setup(cx);
    fx.view
        .update_in(cx, |wb, window, cx| wb.open("e", window, cx));
    cx.run_until_parked();
    // rich-1 is `[[a]]`, rich-2 the long Link to `b`.
    let bounds = cx
        .debug_bounds("rich-2")
        .expect("the second paragraph is clickable");
    let inside = gpui::point(bounds.origin.x + gpui::px(40.), bounds.center().y);
    cx.simulate_click(inside, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(fx.path(cx), "b");
}

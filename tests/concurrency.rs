//! Real-process tests for the write lock, hash checks and journal recovery
//! (docs/spec/testing.md, ADR 0006). Each wikirs invocation is its own process.

use std::{
    path::PathBuf,
    process::{Command, Output},
};

use serde_json::Value;

/// A Wiki root plus its cache dir, cheap to hand to writer threads.
#[derive(Clone)]
struct WikiPaths {
    root: PathBuf,
    cache: PathBuf,
}

impl WikiPaths {
    fn cmd(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wikirs"));
        cmd.env("WIKIRS_CACHE_DIR", &self.cache)
            .arg("--wiki")
            .arg(&self.root)
            .arg("--json")
            .args(args);
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_slice(&self.run(args).stdout).unwrap()
    }

    fn page(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap()
    }
}

/// A fresh Wiki in a tempdir; the tempdir lives as long as this value.
fn temp_wiki() -> (tempfile::TempDir, WikiPaths) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let paths = WikiPaths {
        root: dir.path().join("wiki"),
        cache: dir.path().join("cache"),
    };
    (dir, paths)
}

#[test]
fn racing_writers_never_lose_an_update() {
    let (_dir, wiki) = temp_wiki();
    assert!(
        wiki.run(&["create-page", "--path", "log", "--content", "start\n"])
            .status
            .success()
    );

    // Each writer repeatedly reads the Page and appends a line, passing the
    // version it read. A lost update would leave fewer lines than successes.
    let writers: Vec<_> = (0..4)
        .map(|w| {
            let wiki = wiki.clone();
            std::thread::spawn(move || {
                let (mut applied, mut conflicts) = (0, 0);
                for i in 0..15 {
                    let page = wiki.json(&["get-page", "log"]);
                    let content = page["result"]["content"].as_str().unwrap();
                    let version = page["result"]["version"].as_str().unwrap();
                    let next = format!("{content}w{w}-{i}\n");
                    let out = wiki.run(&[
                        "write-page",
                        "log",
                        "--content",
                        &next,
                        "--base-version",
                        version,
                    ]);
                    match out.status.code() {
                        Some(0) => applied += 1,
                        Some(5) => conflicts += 1,
                        other => panic!(
                            "unexpected exit {other:?}: {}",
                            String::from_utf8_lossy(&out.stdout)
                        ),
                    }
                }
                (applied, conflicts)
            })
        })
        .collect();
    let (applied, conflicts) = writers
        .into_iter()
        .map(|h| h.join().unwrap())
        .fold((0, 0), |(a, c), (a2, c2)| (a + a2, c + c2));

    let lines = wiki.page("log.md").lines().count();
    assert_eq!(
        lines,
        1 + applied,
        "every applied write is in the file ({conflicts} conflicts)"
    );
    assert_eq!(applied + conflicts, 60);
    assert!(conflicts > 0, "the writers never actually raced");
}

#[cfg(feature = "test-hooks")]
mod crash {
    use super::*;

    fn find_file(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
        std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let p = e.path().join(name);
            p.exists().then_some(p)
        })
    }

    /// Runs a mutation that aborts after `n` edits, leaving its journal behind.
    fn crash_after(wiki: &WikiPaths, n: usize, args: &[&str]) {
        let out = wiki
            .cmd(args)
            .env("WIKIRS_TEST_CRASH_AFTER_EDITS", n.to_string())
            .output()
            .unwrap();
        assert!(!out.status.success(), "the crash hook didn't fire");
        assert!(
            find_file(&wiki.cache, "journal.json").is_some(),
            "no journal left behind"
        );
    }

    #[test]
    fn a_crashed_plan_is_finished_at_the_next_open() {
        let (_dir, wiki) = temp_wiki();
        assert!(
            wiki.run(&["create-page", "--path", "p", "--content", "one\n"])
                .status
                .success()
        );
        crash_after(&wiki, 0, &["write-page", "p", "--content", "two\n"]);
        assert_eq!(
            wiki.page("p.md"),
            "one\n",
            "nothing applied before the crash"
        );

        // Any open (here a query) finds the journal and rolls the Plan forward.
        let page = wiki.json(&["get-page", "p"]);
        assert_eq!(page["result"]["content"], "two\n");
        assert!(find_file(&wiki.cache, "journal.json").is_none());
    }

    #[test]
    fn recovery_never_overwrites_an_external_edit() {
        let (_dir, wiki) = temp_wiki();
        assert!(
            wiki.run(&["create-page", "--path", "p", "--content", "one\n"])
                .status
                .success()
        );
        crash_after(&wiki, 0, &["write-page", "p", "--content", "two\n"]);
        std::fs::write(wiki.root.join("p.md"), "edited in vim\n").unwrap();

        let page = wiki.json(&["get-page", "p"]);
        assert_eq!(page["result"]["content"], "edited in vim\n");
        let unrecovered =
            find_file(&wiki.cache, "unrecovered.json").expect("edit reported as unrecovered");
        let report: Value = serde_json::from_slice(&std::fs::read(unrecovered).unwrap()).unwrap();
        assert_eq!(report[0]["path"], "p.md");
    }

    /// A move is several edits (Link splices, then file moves); a crash after
    /// any number of them must still end in the fully moved Wiki.
    #[test]
    fn a_move_crashed_at_every_step_is_finished_at_the_next_open() {
        for applied in 0..4 {
            let (_dir, wiki) = temp_wiki();
            std::fs::create_dir_all(wiki.root.join("eng/rust")).unwrap();
            std::fs::write(
                wiki.root.join("eng/rust.md"),
                "# Rust\n\n[a](rust/async.md)\n",
            )
            .unwrap();
            std::fs::write(wiki.root.join("eng/rust/async.md"), "# Async\n").unwrap();
            std::fs::write(
                wiki.root.join("notes.md"),
                "[[eng/rust]] and [[eng/rust/async]]\n",
            )
            .unwrap();
            // Edits: modify notes.md, move eng/rust.md, move eng/rust/async.md.
            let out = wiki
                .cmd(&["move-page", "eng/rust", "lang/rust"])
                .env("WIKIRS_TEST_CRASH_AFTER_EDITS", applied.to_string())
                .output()
                .unwrap();
            if applied < 3 {
                assert!(
                    !out.status.success(),
                    "crash hook didn't fire after {applied} edits"
                );
            }
            let status = wiki.json(&["index-status"]);
            assert_eq!(
                status["result"]["unrecovered_edits"],
                serde_json::json!([]),
                "after {applied}"
            );
            assert_eq!(
                wiki.page("notes.md"),
                "[[lang/rust]] and [[lang/rust/async]]\n",
                "after {applied}"
            );
            assert_eq!(
                wiki.page("lang/rust.md"),
                "# Rust\n\n[a](rust/async.md)\n",
                "after {applied}"
            );
            assert_eq!(
                wiki.page("lang/rust/async.md"),
                "# Async\n",
                "after {applied}"
            );
            assert!(
                !wiki.root.join("eng").exists(),
                "after {applied}: emptied folders are removed"
            );
        }
    }

    #[test]
    fn a_crashed_create_is_finished_too() {
        let (_dir, wiki) = temp_wiki();
        crash_after(
            &wiki,
            0,
            &["create-page", "--path", "new", "--content", "hi\n"],
        );
        assert!(!wiki.root.join("new.md").exists());
        assert_eq!(wiki.json(&["get-page", "new"])["result"]["content"], "hi\n");
    }
}

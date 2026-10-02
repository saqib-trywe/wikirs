//! The CLI as a user sees it (testing.md#cli): exit codes, stdout vs stderr,
//! the `--json` shape, and snapshots of the human output. Behaviour itself is
//! covered by the parity suite.

use std::fmt::Write;

use assert_cmd::Command;
use serde_json::Value;

/// A scratch Wiki with a few Pages, and everything per machine in the same tempdir.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let wiki = dir.path().join("wiki");
        std::fs::create_dir_all(wiki.join("eng")).unwrap();
        std::fs::create_dir(wiki.join(".wikirs")).unwrap();
        std::fs::write(wiki.join(".wikirs/config.toml"), "").unwrap();
        std::fs::write(
            wiki.join("eng.md"),
            "# Engineering\n\nSee [[eng/rust]] and [[missing]]. #team\n",
        )
        .unwrap();
        std::fs::write(
            wiki.join("eng/rust.md"),
            "---\ntags: [lang]\n---\n# Rust\n\n## Ownership\n\nBorrowing, see [Engineering](../eng.md).\n",
        )
        .unwrap();
        std::fs::write(wiki.join("notes.md"), "# Notes\n\nNothing yet.\n").unwrap();
        Self { dir }
    }

    fn wikirs(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_wikirs"));
        cmd.current_dir(self.dir.path().join("wiki"))
            .env_remove("WIKIRS_WIKI")
            .env("WIKIRS_CACHE_DIR", self.dir.path().join("cache"))
            .env("WIKIRS_CONFIG_DIR", self.dir.path().join("config"))
            .env("WIKIRS_STATE_DIR", self.dir.path().join("state"));
        cmd
    }

    /// Runs `args`; returns (exit code, stdout, stderr).
    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = self.wikirs().args(args).output().unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    }

    /// Human stdout of a successful run.
    fn human(&self, args: &[&str]) -> String {
        let (code, stdout, stderr) = self.run(args);
        assert_eq!(code, 0, "{args:?}: {stderr}");
        stdout
    }
}

#[test]
fn exit_codes_follow_the_error_kind() {
    let f = Fixture::new();
    let cases: &[(&[&str], i32)] = &[
        (&["list-pages"], 0),
        (&["get-page", "eng", "--input", "{}"], 2), // clap: `--input` replaces the arguments
        (&["no-such-command"], 2),
        (&["write-page", "--content", "--input"], 2), // `--input` is the content here
        (&["get-page", "--input", r#"{"page":"/abs"}"#], 2), // invalid_path
        (&["get-page", "--input", "{not json"], 2),
        (&["get-page", "nowhere"], 3),
        (&["create-page", "--path", "notes"], 4),
        (
            &[
                "write-page",
                "notes",
                "--content",
                "x",
                "--base-version",
                "0",
            ],
            5,
        ),
        (
            &["edit-page", "notes", "--edit", r#"{"old":"zzz","new":"y"}"#],
            6,
        ),
        (&["check"], 0),
        (&["check", "--fail-on-diagnostics"], 7),
        (
            &[
                "check",
                "--fail-on-diagnostics",
                "--input",
                r#"{"scope":{"path_prefix":"notes"}}"#,
            ],
            0,
        ),
    ];
    for (args, expected) in cases {
        let (code, _, stderr) = f.run(args);
        assert_eq!(code, *expected, "{args:?}: {stderr}");
    }
    let (code, _, _) = f.run(&["--json", "get-page", "nowhere"]);
    assert_eq!(code, 3, "--json keeps the exit code");
}

#[test]
fn results_go_to_stdout_and_errors_and_warnings_to_stderr() {
    let f = Fixture::new();
    let (_, stdout, stderr) = f.run(&["get-page", "nowhere"]);
    assert_eq!(stdout, "");
    assert_eq!(stderr, "error[not_found]: page `nowhere` does not exist\n");

    let (_, stdout, stderr) = f.run(&[
        "write-page",
        "notes",
        "--content",
        "x",
        "--base-version",
        "0",
    ]);
    assert_eq!(stdout, "");
    assert!(
        stderr.starts_with("error[conflict]: ") && stderr.contains("\nhint: "),
        "{stderr}"
    );

    // Deleting a linked Page warns that the link will break.
    let (code, stdout, stderr) = f.run(&["delete-page", "eng/rust"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "deleted eng/rust.md\n");
    assert!(stderr.starts_with("warning: "), "{stderr}");
}

#[test]
fn json_prints_one_envelope_or_error_on_stdout() {
    let f = Fixture::new();
    let (_, stdout, stderr) = f.run(&["--json", "list-pages"]);
    assert_eq!(stderr, "");
    let envelope: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(envelope["result"]["total"], 3);
    assert!(envelope["warnings"].is_array());

    let (_, stdout, stderr) = f.run(&["get-page", "nowhere", "--json"]);
    assert_eq!(stderr, "");
    let error: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(error["error"]["kind"], "not_found");
    assert_eq!(error["error"]["details"]["what"], "page");

    let (_, stdout, _) = f.run(&["--json", "list-pages", "--input", "["]);
    let error: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(error["error"]["details"]["field"], "input");
}

#[test]
fn input_reads_json_from_stdin() {
    let f = Fixture::new();
    let out = f
        .wikirs()
        .args(["outline", "--input", "-"])
        .write_stdin(r#"{"page":"eng/rust"}"#)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "Rust\n  Ownership\n"
    );
}

#[test]
fn human_output_of_queries() {
    let f = Fixture::new();
    let mut shown = String::new();
    for args in [
        &["list-pages"][..],
        &["children", "--depth", "2"],
        &["get-page", "eng"],
        &["outline", "eng/rust"],
        &["links", "eng"],
        &["backlinks", "eng"],
        &["tag-tree"],
        &["check"],
        &["search", "borrowing"],
        &["get-config", "links.syntax"],
    ] {
        let _ = writeln!(shown, "$ wikirs {}\n{}", args.join(" "), f.human(args));
    }
    insta::assert_snapshot!(shown);
}

#[test]
fn human_output_of_mutations_is_a_diff_then_a_summary() {
    let f = Fixture::new();
    let mut shown = String::new();
    for args in [
        &["move-page", "eng/rust", "eng/rustlang", "--dry-run"][..],
        &["move-page", "eng/rust", "eng/rustlang"],
        &["tag-page", "notes", "todo", "--dry-run"],
        &[
            "create-page",
            "--title",
            "Ideas",
            "--content",
            "Some ideas.",
            "--dry-run",
        ],
        &["delete-page", "notes", "--dry-run"],
        &["delete-page", "notes"],
    ] {
        let _ = writeln!(shown, "$ wikirs {}\n{}", args.join(" "), f.human(args));
    }
    insta::assert_snapshot!(shown);
}

#[test]
fn a_moved_wiki_is_pointed_at_config_adopt() {
    let f = Fixture::new();
    f.human(&[
        "set-config",
        "links.syntax",
        "wikilink",
        "--scope",
        "machine",
    ]);
    let moved = f.dir.path().join("moved");
    std::fs::rename(f.dir.path().join("wiki"), &moved).unwrap();
    let run = |args: &[&str]| {
        let out = f.wikirs().current_dir(&moved).args(args).output().unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    };

    let (_, stdout, stderr) = run(&["get-config", "links.syntax"]);
    assert_eq!(stdout, "links.syntax = \"standard\"  (default)\n");
    assert!(
        stderr.contains("hint: ") && stderr.contains("`wikirs config adopt`"),
        "{stderr}"
    );
    let (_, _, stderr) = run(&["--json", "get-config", "links.syntax"]);
    assert_eq!(stderr, "", "no hint in JSON mode");

    let (code, stdout, stderr) = run(&["config", "adopt"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.starts_with("adopted the settings of "), "{stdout}");
    let (_, stdout, stderr) = run(&["get-config", "links.syntax"]);
    assert_eq!(stdout, "links.syntax = \"wikilink\"  (machine)\n");
    assert_eq!(stderr, "");

    let (code, stdout, _) = run(&["--json", "config", "adopt"]);
    assert_eq!(code, 2);
    let error: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(error["error"]["kind"], "invalid_input");
}

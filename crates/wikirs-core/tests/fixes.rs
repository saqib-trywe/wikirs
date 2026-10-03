//! Fixes from the known-quirks list: broken embeds report an Attachment,
//! search ignores frontmatter, and moves rewrite reference definitions.

use serde_json::{Value, json};
use wikirs_core::{Wiki, find};

fn wiki() -> (tempfile::TempDir, Wiki) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
    (dir, wiki)
}

fn call(wiki: &Wiki, op: &str, input: Value) -> Value {
    find(op).unwrap().call(wiki, input).unwrap()
}

fn create(wiki: &Wiki, path: &str, content: &str) {
    call(
        wiki,
        "create_page",
        json!({ "path": path, "content": content }),
    );
}

#[test]
fn a_broken_embed_is_a_missing_attachment() {
    let (_dir, wiki) = wiki();
    create(&wiki, "a", "x");
    let resolved = |raw: &str| {
        call(
            &wiki,
            "resolve_link",
            json!({ "from_page": "a", "raw": raw }),
        )["result"]
            .clone()
    };
    let embed = resolved("![[a/diagram.png]]");
    assert_eq!(
        (embed["status"].as_str(), embed["target_kind"].as_str()),
        (Some("broken"), Some("attachment"))
    );
    let page = resolved("[[nope]]");
    assert_eq!(page["target_kind"], "page");
    // A Page whose name has a dot still resolves to the Page.
    create(&wiki, "notes.v2", "y");
    let dotted = resolved("[[notes.v2]]");
    assert_eq!(
        (dotted["status"].as_str(), dotted["target_kind"].as_str()),
        (Some("ok"), Some("page"))
    );
}

#[test]
fn search_ignores_frontmatter() {
    let (_dir, wiki) = wiki();
    create(
        &wiki,
        "a",
        "---\ntags: [zebra]\nsummary: quokka\n---\n# A\n\nbody text\n",
    );
    let hits =
        |text: &str| call(&wiki, "search", json!({ "text": text }))["result"]["total"].as_i64();
    assert_eq!(hits("quokka"), Some(0));
    assert_eq!(hits("body"), Some(1));
    let snippet =
        call(&wiki, "search", json!({ "text": "body" }))["result"]["hits"][0]["snippet"].clone();
    assert!(!snippet.as_str().unwrap().contains("tags"), "{snippet}");
}

#[test]
fn moves_rewrite_reference_definitions() {
    let (dir, wiki) = wiki();
    create(&wiki, "b", "# B\n");
    create(
        &wiki,
        "a",
        "See [the B][b], [again][B] and [b].\n\n[b]: b.md \"Bee\"\n",
    );
    create(&wiki, "sub/c", "[home]\n\n[home]: ../a.md\n");
    let out = call(&wiki, "move_page", json!({ "from": "b", "to": "sub/b" }));
    assert_eq!(
        out["warnings"],
        json!([]),
        "no Link left for the user to fix"
    );
    let a = std::fs::read_to_string(dir.path().join("wiki/a.md")).unwrap();
    assert_eq!(
        a,
        "See [the B][b], [again][B] and [b].\n\n[b]: sub/b.md \"Bee\"\n"
    );
    // A relative definition inside a moved Page follows the move.
    call(&wiki, "move_page", json!({ "from": "sub/c", "to": "c" }));
    let c = std::fs::read_to_string(dir.path().join("wiki/c.md")).unwrap();
    assert_eq!(c, "[home]\n\n[home]: a.md\n");
    // Backlinks see the reference Links at their new target.
    let back =
        call(&wiki, "backlinks", json!({ "target": "sub/b" }))["result"]["backlinks"].clone();
    assert_eq!(back.as_array().unwrap().len(), 3, "{back}");
}

#[test]
fn a_definition_after_an_inline_link_is_spliced_in_order() {
    let (dir, wiki) = wiki();
    create(&wiki, "b", "# B\n");
    // The reference Link comes first, then an inline one, then the definition.
    create(&wiki, "a", "[ref one][b]\n\n[inline](b.md)\n\n[b]: b.md\n");
    let out = call(&wiki, "move_page", json!({ "from": "b", "to": "z/b" }));
    assert_eq!(out["result"]["links_rewritten"], 2, "{out}");
    let a = std::fs::read_to_string(dir.path().join("wiki/a.md")).unwrap();
    assert_eq!(a, "[ref one][b]\n\n[inline](z/b.md)\n\n[b]: z/b.md\n");
}

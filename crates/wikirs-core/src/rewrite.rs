//! Rewriting a Link's source text when its target moves (Page identity
//! decision, item 8): keep the syntax, alias, `#heading` and link text; change
//! only the path.

/// A path relative from folder `from_dir` (Wiki-root relative, "" for the
/// root) to file `to_file`.
#[must_use]
pub fn relative(from_dir: &str, to_file: &str) -> String {
    let from: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = to_file.split('/').filter(|s| !s.is_empty()).collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(a, b)| a == b)
        .count()
        // Never share the file name itself.
        .min(to.len().saturating_sub(1));
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    parts.join("/")
}

/// Percent-encodes what can't appear bare in a standard link destination.
fn encode_dest(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            c => out.push(c),
        }
    }
    out
}

/// The byte range of the target path inside a wikilink's source text, e.g.
/// `eng/rust` in `![[eng/rust#h|alias]]`.
fn wiki_target_range(raw: &str) -> Option<std::ops::Range<usize>> {
    let open = raw.find("[[")? + 2;
    let close = raw[open..].find("]]")? + open;
    let end = raw[open..close]
        .find(['#', '|'])
        .map_or(close, |i| open + i);
    Some(open..end)
}

/// The byte range of the path inside a standard link's destination, e.g.
/// `../a%20b.md` in `[x](../a%20b.md#h "title")`. `None` for reference-style
/// links, whose destination lives elsewhere.
fn standard_target_range(raw: &str) -> Option<std::ops::Range<usize>> {
    let open = raw.rfind("](")? + 2;
    let body = &raw[open..raw.len().checked_sub(1)?];
    if let Some(inner) = body.strip_prefix('<') {
        let end = inner.find('>')?;
        let path_end = inner[..end].find('#').unwrap_or(end);
        return Some(open + 1..open + 1 + path_end);
    }
    let dest_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let path_end = body[..dest_end].find('#').unwrap_or(dest_end);
    Some(open..open + path_end)
}

/// The path currently written in a Link (percent-decoded for standard
/// Links), or `None` if it can't be located (reference-style Links).
#[must_use]
pub fn written_path(raw: &str, wiki: bool) -> Option<String> {
    if wiki {
        wiki_target_range(raw).map(|r| raw[r].trim().to_string())
    } else {
        standard_target_range(raw).map(|r| {
            percent_encoding::percent_decode_str(&raw[r])
                .decode_utf8_lossy()
                .into_owned()
        })
    }
}

/// `raw` with its target path replaced by `new_path` (a wikilink path, or a
/// standard link path, which gets percent-encoded). `None` if the Link can't
/// be rewritten in place.
#[must_use]
pub fn with_path(raw: &str, wiki: bool, new_path: &str) -> Option<String> {
    let (range, text) = if wiki {
        (wiki_target_range(raw)?, new_path.to_string())
    } else {
        let range = standard_target_range(raw)?;
        let angle = raw[..range.start].ends_with('<');
        (
            range,
            if angle {
                new_path.to_string()
            } else {
                encode_dest(new_path)
            },
        )
    };
    Some(format!(
        "{}{text}{}",
        &raw[..range.start],
        &raw[range.end..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(relative("eng", "eng/rust.md"), "rust.md");
        assert_eq!(relative("eng/rust", "eng/a.md"), "../a.md");
        assert_eq!(relative("", "eng/a.md"), "eng/a.md");
        assert_eq!(relative("eng/rust", "notes.md"), "../../notes.md");
        assert_eq!(relative("a/b", "a/b/c/d.png"), "c/d.png");
    }

    #[test]
    fn rewriting_keeps_everything_but_the_path() {
        assert_eq!(
            with_path("[[eng/rust]]", true, "lang/rust").unwrap(),
            "[[lang/rust]]"
        );
        assert_eq!(
            with_path("![[eng/rust#Pin|Rust]]", true, "lang/rust").unwrap(),
            "![[lang/rust#Pin|Rust]]"
        );
        assert_eq!(
            with_path("[Rust](../rust.md#pin \"t\")", false, "lang/my rust.md").unwrap(),
            "[Rust](lang/my%20rust.md#pin \"t\")"
        );
        assert_eq!(
            with_path("[x](<a b.md>)", false, "c d.md").unwrap(),
            "[x](<c d.md>)"
        );
        assert_eq!(
            with_path("[x][ref]", false, "a.md"),
            None,
            "reference links can't be rewritten in place"
        );
        assert_eq!(
            written_path("[x](a%20b.md#h)", false).as_deref(),
            Some("a b.md")
        );
        assert_eq!(
            written_path("[[eng/rust#h|x]]", true).as_deref(),
            Some("eng/rust")
        );
    }
}

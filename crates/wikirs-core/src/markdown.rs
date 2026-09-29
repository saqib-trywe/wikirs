//! The one markdown parse (ADR 0007, docs/spec/markdown.md): pulldown-cmark
//! with the supported extensions, plus the Inline Tag scanner (Tag model).
//!
//! Whether text is a Link, Tag or heading is decided here and nowhere else.

use std::ops::Range;

use pulldown_cmark::{
    CodeBlockKind, Event, HeadingLevel, LinkType, MetadataBlockKind, Options, Parser, Tag, TagEnd,
};
use serde_json::Value;
use yaml_rust2::{Yaml, YamlLoader};

/// Everything the Index needs from one Page.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parsed {
    /// The frontmatter as JSON, if the Page has a (valid YAML mapping) block.
    pub frontmatter: Option<Value>,
    /// Frontmatter `title`, else the first level-1 heading.
    pub title: Option<String>,
    pub headings: Vec<Heading>,
    pub links: Vec<RawLink>,
    pub tags: Vec<TagRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub anchor: String,
    pub range: Range<usize>,
}

/// A Link as written: where it points, before resolving it against the Wiki.
#[derive(Debug, Clone, PartialEq)]
pub struct RawLink {
    /// The Link's source text, e.g. `[[eng/rust|Rust]]`.
    pub raw: String,
    pub range: Range<usize>,
    /// `[[…]]` (resolved from the Wiki root) vs `[text](…)` (relative).
    pub wiki: bool,
    /// `![[…]]` / `![](…)`.
    pub embed: bool,
    /// The target path as written, percent-decoded, without `#heading`.
    pub target: String,
    pub heading: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TagRef {
    /// As written (a mixed-case Tag is accepted, with a warning).
    pub tag: String,
    pub source: TagSource,
    /// Byte range of the Tag (inline: including `#`). None for frontmatter.
    pub range: Option<Range<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagSource {
    Frontmatter,
    Inline,
}

impl TagSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TagSource::Frontmatter => "frontmatter",
            TagSource::Inline => "inline",
        }
    }
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_WIKILINKS
}

/// Parses one Page.
#[must_use]
pub fn parse(text: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut in_metadata = false;
    let mut in_code_block = false;
    let mut heading: Option<(u8, Range<usize>, String)> = None;
    // Text inside an autolink or email link is a URL, never a Tag.
    let mut in_url_link = 0usize;

    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::MetadataBlock(MetadataBlockKind::YamlStyle)) => in_metadata = true,
            Event::End(TagEnd::MetadataBlock(_)) => in_metadata = false,
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_) | CodeBlockKind::Indented)) => {
                in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some((level_num(level), range, String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, range, text)) = heading.take() {
                    let text = text.trim().to_string();
                    if level == 1 && out.title.is_none() {
                        out.title = Some(text.clone());
                    }
                    out.headings.push(Heading {
                        level,
                        anchor: anchor(&text),
                        text,
                        range,
                    });
                }
            }
            Event::Start(
                Tag::Link {
                    link_type,
                    dest_url,
                    ..
                }
                | Tag::Image {
                    link_type,
                    dest_url,
                    ..
                },
            ) => {
                if matches!(link_type, LinkType::Autolink | LinkType::Email) {
                    in_url_link += 1;
                    continue;
                }
                let embed = text[range.clone()].starts_with('!');
                if let Some(link) = raw_link(text, range, &dest_url, link_type, embed) {
                    out.links.push(link);
                }
            }
            Event::End(TagEnd::Link) if in_url_link > 0 => in_url_link -= 1,
            Event::Text(t) if in_metadata => {
                if let Some(fm) = parse_frontmatter(&t) {
                    out.frontmatter = Some(fm);
                }
            }
            Event::Text(t) => {
                if let Some((_, _, h)) = heading.as_mut() {
                    h.push_str(&t);
                }
                if !in_code_block && in_url_link == 0 {
                    scan_inline_tags(&t, range.start, &text[range.clone()], &mut out.tags);
                }
            }
            // Inline code counts toward a heading's text but never carries Tags.
            Event::Code(t) => {
                if let Some((_, _, h)) = heading.as_mut() {
                    h.push_str(&t);
                }
            }
            _ => {}
        }
    }

    if let Some(fm) = &out.frontmatter {
        if let Some(title) = fm.get("title").and_then(Value::as_str) {
            out.title = Some(title.to_string());
        }
        let mut fm_tags = Vec::new();
        match fm.get("tags") {
            Some(Value::String(s)) => fm_tags.push(s.clone()),
            Some(Value::Array(items)) => {
                fm_tags.extend(items.iter().filter_map(|v| v.as_str().map(str::to_string)));
            }
            _ => {}
        }
        let inline = std::mem::take(&mut out.tags);
        out.tags = fm_tags
            .into_iter()
            .filter(|t| valid_tag(t))
            .map(|tag| TagRef {
                tag,
                source: TagSource::Frontmatter,
                range: None,
            })
            .chain(inline)
            .collect();
    }
    out
}

fn level_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// GitHub-style heading anchor: lowercase, spaces to `-`, punctuation dropped.
#[must_use]
pub fn anchor(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

fn raw_link(
    text: &str,
    range: Range<usize>,
    dest: &str,
    link_type: LinkType,
    embed: bool,
) -> Option<RawLink> {
    let wiki = matches!(link_type, LinkType::WikiLink { .. });
    let dest = dest.trim();
    // External URLs and in-page anchors are not Links.
    if dest.is_empty() || dest.starts_with('#') || has_scheme(dest) {
        return None;
    }
    let (path, heading) = match dest.split_once('#') {
        Some((p, h)) => (p, Some(h.to_string())),
        None => (dest, None),
    };
    let target = if wiki {
        path.trim().to_string()
    } else {
        percent_encoding::percent_decode_str(path)
            .decode_utf8_lossy()
            .into_owned()
    };
    if target.is_empty() {
        return None;
    }
    Some(RawLink {
        raw: text[range.clone()].to_string(),
        range,
        wiki,
        embed,
        target,
        heading,
    })
}

fn has_scheme(dest: &str) -> bool {
    dest.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
    })
}

fn parse_frontmatter(yaml: &str) -> Option<Value> {
    let doc = YamlLoader::load_from_str(yaml).ok()?.into_iter().next()?;
    match yaml_to_json(&doc) {
        v @ Value::Object(_) => Some(v),
        _ => None,
    }
}

pub(crate) fn yaml_to_json(y: &Yaml) -> Value {
    match y {
        Yaml::Real(s) => s.parse::<f64>().map_or(Value::Null, Value::from),
        Yaml::Integer(i) => Value::from(*i),
        Yaml::String(s) => Value::from(s.as_str()),
        Yaml::Boolean(b) => Value::from(*b),
        Yaml::Array(items) => Value::Array(items.iter().map(yaml_to_json).collect()),
        Yaml::Hash(map) => Value::Object(
            map.iter()
                .filter_map(|(k, v)| {
                    let key = match k {
                        Yaml::String(s) => s.clone(),
                        Yaml::Integer(i) => i.to_string(),
                        Yaml::Boolean(b) => b.to_string(),
                        _ => return None,
                    };
                    Some((key, yaml_to_json(v)))
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_' || c == '/'
}

/// Tag syntax (Tag model): letters, digits, `-`, `_`, `/`; no empty segment;
/// every segment has a non-digit.
#[must_use]
pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.chars().all(is_tag_char)
        && tag
            .split('/')
            .all(|seg| !seg.is_empty() && seg.chars().any(|c| !c.is_ascii_digit()))
}

/// Finds `#tag`s in one text event. `offset` is where `text` starts in the
/// file; `source` is the event's source slice, used to map positions when the
/// event text is the source verbatim.
fn scan_inline_tags(text: &str, offset: usize, source: &str, out: &mut Vec<TagRef>) {
    let verbatim = source == text;
    for (word_start, word) in split_words(text) {
        if word.contains("://") || word.starts_with("www.") {
            continue; // URLs
        }
        let chars: Vec<(usize, char)> = word.char_indices().collect();
        let mut i = 0;
        while i < chars.len() {
            let (at, c) = chars[i];
            let boundary = i == 0 || {
                let prev = chars[i - 1].1;
                !(prev.is_alphanumeric() || prev == '_' || prev == '#' || prev == '&')
            };
            if c == '#' && boundary {
                let mut end = i + 1;
                while end < chars.len() && is_tag_char(chars[end].1) {
                    end += 1;
                }
                let from = at + 1;
                let to = chars.get(end).map_or(word.len(), |(p, _)| *p);
                let tag = word[from..to].trim_end_matches('/');
                if valid_tag(tag) {
                    let start = word_start + at;
                    out.push(TagRef {
                        tag: tag.to_string(),
                        source: TagSource::Inline,
                        range: verbatim.then(|| offset + start..offset + start + 1 + tag.len()),
                    });
                }
                i = end;
            } else {
                i += 1;
            }
        }
    }
}

fn split_words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split(char::is_whitespace)
        .scan(0, |pos, word| {
            let start = *pos;
            *pos += word.len() + 1;
            Some((start, word))
        })
        .filter(|(_, w)| !w.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(text: &str) -> Vec<String> {
        parse(text).tags.into_iter().map(|t| t.tag).collect()
    }

    #[test]
    fn links_of_every_kind() {
        let text = "See [[eng/rust|Rust]], [[eng/rust#Pinning]], [notes](../notes%20x.md#top), \
                    ![[eng/diagram.png]], ![img](pic.png), [web](https://x.com/a.md), [top](#here), <https://auto.link>.";
        let links = parse(text).links;
        let summary: Vec<_> = links
            .iter()
            .map(|l| (l.target.as_str(), l.wiki, l.embed, l.heading.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                ("eng/rust", true, false, None),
                ("eng/rust", true, false, Some("Pinning")),
                ("../notes x.md", false, false, Some("top")),
                ("eng/diagram.png", true, true, None),
                ("pic.png", false, true, None),
            ]
        );
        assert_eq!(&text[links[0].range.clone()], "[[eng/rust|Rust]]");
        assert_eq!(links[0].raw, "[[eng/rust|Rust]]");
    }

    #[test]
    fn inline_tags_follow_the_tag_model() {
        // `#2026` and `#a/2026` fail "every segment has a non-digit"; `x#no` has no
        // boundary before `#`; `/` is punctuation, so `a/#yes` is a Tag.
        assert_eq!(
            tags("#lang/rust and (#async) #Mixed #2026 #a/2026 x#no #ok/ a/#yes"),
            ["lang/rust", "async", "Mixed", "ok", "yes"]
        );
        assert_eq!(
            tags("`#code` and\n\n```\n#block\n```\n\nhttps://x.com/#frag <https://y.com/#z>"),
            Vec::<String>::new()
        );
        assert_eq!(
            tags("# Heading #tagged\n\n[link #text](x.md)"),
            ["tagged", "text"]
        );
        let parsed = parse("text #lang/rust here");
        assert_eq!(parsed.tags[0].range, Some(5..15));
    }

    #[test]
    fn frontmatter_gives_title_and_tags() {
        let parsed = parse(
            "---\ntitle: Real Title\ntags: [lang/rust, \"bad tag\", project/wikirs]\norder: 20\n---\n# H1\n\nbody #inline\n",
        );
        assert_eq!(parsed.title.as_deref(), Some("Real Title"));
        assert_eq!(parsed.frontmatter.as_ref().unwrap()["order"], 20);
        let got: Vec<_> = parsed
            .tags
            .iter()
            .map(|t| (t.tag.as_str(), t.source))
            .collect();
        assert_eq!(
            got,
            [
                ("lang/rust", TagSource::Frontmatter),
                ("project/wikirs", TagSource::Frontmatter),
                ("inline", TagSource::Inline)
            ]
        );
        assert_eq!(parse("---\ntags: solo\n---\n").tags[0].tag, "solo");
        assert_eq!(parse("# First\n\n# Second").title.as_deref(), Some("First"));
    }

    #[test]
    fn headings_get_github_anchors() {
        let h = parse("## Pinning & `Unpin`, part 2\n").headings;
        assert_eq!(
            (h[0].level, h[0].text.as_str(), h[0].anchor.as_str()),
            (2, "Pinning & Unpin, part 2", "pinning--unpin-part-2")
        );
    }
}

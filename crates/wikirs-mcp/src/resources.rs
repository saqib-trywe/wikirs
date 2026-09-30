//! MCP resources: a logic-free projection of the read Operations
//! (docs/spec/interfaces.md#mcp), and the notifications `watch` drives.

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use rmcp::{
    ErrorData as McpError,
    model::{
        ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult, Resource,
        ResourceContents, ResourceTemplate,
    },
};
use serde_json::{Value, json};
use wikirs_core::{
    Error, ErrorKind, Wiki,
    watch::{EventKind, WatchEvent},
};

const PAGE: &str = "wiki://page/";
const ATTACHMENT: &str = "wiki://attachment/";
/// Resources per `resources/list` page.
const PAGE_SIZE: usize = 100;

/// What a path segment can't hold unescaped in a `{+path}` expansion (`/` stays).
const ESCAPED: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

#[must_use]
pub fn page_uri(page: &str) -> String {
    format!("{PAGE}{}", utf8_percent_encode(page, ESCAPED))
}

#[must_use]
pub fn attachment_uri(path: &str) -> String {
    format!("{ATTACHMENT}{}", utf8_percent_encode(path, ESCAPED))
}

#[derive(Debug, PartialEq)]
enum Target {
    Page(String),
    Attachment(String),
}

fn parse(uri: &str) -> Option<Target> {
    let decode = |rest: &str| {
        percent_decode_str(rest)
            .decode_utf8()
            .ok()
            .map(std::borrow::Cow::into_owned)
            .filter(|s| !s.is_empty())
    };
    if let Some(rest) = uri.strip_prefix(PAGE) {
        decode(rest).map(Target::Page)
    } else {
        uri.strip_prefix(ATTACHMENT)
            .and_then(decode)
            .map(Target::Attachment)
    }
}

/// Whether `uri` names a Page or Attachment resource (it may not exist).
#[must_use]
pub fn is_resource_uri(uri: &str) -> bool {
    parse(uri).is_some()
}

/// Runs a registry Operation, returning its `result`.
fn call(wiki: &Wiki, op: &str, input: Value) -> Result<Value, Error> {
    let op = wikirs_core::find(op).expect("a registry Operation");
    Ok(op.call(wiki, input)?["result"].take())
}

/// Resource errors (errors.md#mcp): `not_found` is resource-not-found, the
/// rest are internal errors carrying the error object.
fn to_mcp(err: &Error) -> McpError {
    let object = Some(err.to_json()["error"].take());
    match err.kind {
        ErrorKind::NotFound => McpError::resource_not_found(err.message.clone(), object),
        _ => McpError::internal_error(err.message.clone(), object),
    }
}

#[must_use]
pub fn templates() -> ListResourceTemplatesResult {
    ListResourceTemplatesResult::with_all_items(vec![
        ResourceTemplate::new(format!("{PAGE}{{+path}}"), "page")
            .with_description("A Page's raw markdown, by Page Path (`get_page`).")
            .with_mime_type("text/markdown"),
        ResourceTemplate::new(format!("{ATTACHMENT}{{+path}}"), "attachment").with_description(
            "An Attachment's bytes, by path from the Wiki root (`read_attachment`).",
        ),
    ])
}

/// One page of the listing: every Page (by path), then every Attachment.
/// The cursor is the offset into that sequence.
pub fn list(wiki: &Wiki, cursor: Option<&str>) -> Result<ListResourcesResult, McpError> {
    let offset: usize = match cursor {
        None => 0,
        Some(c) => c
            .parse()
            .map_err(|_| McpError::invalid_params(format!("bad cursor `{c}`"), None))?,
    };
    let pages = call(
        wiki,
        "list_pages",
        json!({ "limit": PAGE_SIZE, "offset": offset }),
    )
    .map_err(|e| to_mcp(&e))?;
    let total_pages = usize::try_from(pages["total"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX);
    let mut resources: Vec<Resource> = pages["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let path = p["path"].as_str().unwrap_or_default();
            Resource::new(page_uri(path), path)
                .with_title(p["title"].as_str().unwrap_or_default())
                .with_mime_type("text/markdown")
        })
        .collect();
    let mut total = total_pages;
    if resources.len() < PAGE_SIZE {
        let attachments = call(wiki, "list_attachments", json!({})).map_err(|e| to_mcp(&e))?;
        let attachments = attachments["attachments"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        total += attachments.len();
        let wanted = PAGE_SIZE - resources.len();
        resources.extend(
            attachments
                .iter()
                .skip(offset.saturating_sub(total_pages))
                .take(wanted)
                .map(|a| {
                    let path = a["path"].as_str().unwrap_or_default();
                    Resource::new(attachment_uri(path), path)
                        .with_size(a["size"].as_u64().unwrap_or(0))
                }),
        );
    } else {
        // Only Pages on this page: say there's more without listing Attachments.
        total = usize::MAX;
    }
    let next = offset + resources.len();
    let mut result = ListResourcesResult::with_all_items(resources);
    if next < total {
        result.next_cursor = Some(next.to_string());
    }
    Ok(result)
}

pub fn read(wiki: &Wiki, uri: &str) -> Result<ReadResourceResult, McpError> {
    let contents = match parse(uri) {
        Some(Target::Page(page)) => {
            let page = call(wiki, "get_page", json!({ "page": page })).map_err(|e| to_mcp(&e))?;
            ResourceContents::text(page["content"].as_str().unwrap_or_default(), uri)
                .with_mime_type("text/markdown")
        }
        Some(Target::Attachment(path)) => {
            let file =
                call(wiki, "read_attachment", json!({ "path": path })).map_err(|e| to_mcp(&e))?;
            ResourceContents::blob(file["base64"].as_str().unwrap_or_default(), uri)
        }
        None => {
            return Err(McpError::resource_not_found(
                format!("`{uri}` is not a wiki:// Page or Attachment URI"),
                None,
            ));
        }
    };
    Ok(ReadResourceResult::new(vec![contents]))
}

/// A notification for MCP clients, from `watch`.
#[derive(Debug, PartialEq, Eq)]
pub enum Notice {
    /// `notifications/resources/updated` for this URI.
    Updated(String),
    /// `notifications/resources/list_changed`: once per batch at most.
    ListChanged,
}

/// What one event calls for: its resource(s) changed, the listing changed
/// unless it was only an edit, and `index_updated` ends the batch.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Updated(String),
    ListChanged,
    BatchEnd,
}

fn steps(event: &WatchEvent) -> Vec<Step> {
    let uri: fn(&str) -> String = match event.kind {
        EventKind::IndexUpdated => return vec![Step::BatchEnd],
        EventKind::AttachmentChanged => attachment_uri,
        _ => page_uri,
    };
    let mut out: Vec<Step> = event
        .from
        .iter()
        .chain(&event.path)
        .map(|p| Step::Updated(uri(p)))
        .collect();
    if event.kind != EventKind::PageModified {
        out.push(Step::ListChanged);
    }
    out
}

/// Turns `watch` events into notices until `send` returns false or the
/// events end: every changed resource, then one `ListChanged` per batch
/// that added, removed or moved something.
pub fn relay(events: impl IntoIterator<Item = WatchEvent>, mut send: impl FnMut(Notice) -> bool) {
    let mut list_changed = false;
    for event in events {
        for step in steps(&event) {
            let notice = match step {
                Step::Updated(uri) => Notice::Updated(uri),
                Step::ListChanged => {
                    list_changed = true;
                    continue;
                }
                Step::BatchEnd if std::mem::take(&mut list_changed) => Notice::ListChanged,
                Step::BatchEnd => continue,
            };
            if !send(notice) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_escape_what_a_path_expansion_cannot_hold() {
        let uri = page_uri("eng/rust notes/50% #1?");
        assert_eq!(uri, "wiki://page/eng/rust%20notes/50%25%20%231%3F");
        assert_eq!(
            parse(&uri),
            Some(Target::Page("eng/rust notes/50% #1?".into()))
        );
        assert_eq!(
            parse("wiki://attachment/eng/d%C3%A9.png"),
            Some(Target::Attachment("eng/dé.png".into()))
        );
        for bad in [
            "wiki://page/",
            "wiki://tag/x",
            "file:///x",
            "wiki://page/%FF",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn attachments_page_on_after_the_pages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wiki");
        std::fs::create_dir_all(root.join("a")).unwrap();
        for page in ["a", "b", "c"] {
            std::fs::write(root.join(format!("{page}.md")), "x").unwrap();
        }
        for i in 0..150 {
            std::fs::write(root.join(format!("a/{i:03}.bin")), "x").unwrap();
        }
        let wiki = Wiki::open_isolated(&root, dir.path().join("base")).unwrap();
        let mut uris = Vec::new();
        let mut cursor = None;
        loop {
            let page = list(&wiki, cursor.as_deref()).unwrap();
            uris.extend(page.resources.into_iter().map(|r| r.uri.clone()));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(uris.len(), 153);
        assert_eq!(uris[3], "wiki://attachment/a/000.bin");
        assert_eq!(
            uris[100], "wiki://attachment/a/097.bin",
            "the second page resumes"
        );
        assert_eq!(uris[152], "wiki://attachment/a/149.bin");
        assert!(list(&wiki, Some("x")).is_err(), "a cursor is an offset");
    }

    #[test]
    fn events_become_one_list_changed_per_batch() {
        let event = |kind, path: Option<&str>, from: Option<&str>| WatchEvent {
            kind,
            path: path.map(str::to_string),
            from: from.map(str::to_string),
            version: None,
        };
        let done = || event(EventKind::IndexUpdated, None, None);
        let events = vec![
            event(EventKind::PageModified, Some("a"), None),
            done(),
            event(EventKind::PageCreated, Some("n"), None),
            event(EventKind::PageMoved, Some("b"), Some("a")),
            event(EventKind::AttachmentChanged, Some("a/x.png"), None),
            done(),
            event(EventKind::PageDeleted, Some("n"), None),
        ];
        let mut got = Vec::new();
        relay(events, |n| {
            got.push(n);
            true
        });
        let up = |u: &str| Notice::Updated(u.to_string());
        assert_eq!(
            got,
            [
                up("wiki://page/a"),
                up("wiki://page/n"),
                up("wiki://page/a"),
                up("wiki://page/b"),
                up("wiki://attachment/a/x.png"),
                Notice::ListChanged,
                up("wiki://page/n"),
            ],
            "edits alone don't change the listing; an unfinished batch waits"
        );

        let mut sent = 0;
        relay(
            vec![event(EventKind::PageMoved, Some("b"), Some("a"))],
            |_| {
                sent += 1;
                false
            },
        );
        assert_eq!(sent, 1, "stops once the client is gone");
    }
}

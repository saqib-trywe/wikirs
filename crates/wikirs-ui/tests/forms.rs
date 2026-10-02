//! The palette's form model against the real registry (interfaces.md#gui-and-tui).

use serde_json::{Value, json};
use wikirs_core::{Kind, Wiki, find, registry};
use wikirs_ui::{Control, Field, Form, forms};

/// "The form renderer must support every schema shape used by any Input."
#[test]
fn every_operation_has_a_form() {
    let forms = forms().unwrap_or_else(|e| panic!("{e}"));
    let names: Vec<_> = forms.iter().map(|f| f.op).collect();
    let registry: Vec<_> = registry().iter().map(|op| op.name).collect();
    assert_eq!(names, registry);
    for form in &forms {
        // Mutations take `dry_run` (as the toggle); nothing else does.
        assert_eq!(
            form.dry_run.is_some(),
            form.kind == Kind::Mutation,
            "{}",
            form.op
        );
        assert!(form.field("dry_run").is_none(), "{}", form.op);
    }
}

#[test]
fn required_fields_come_first_in_declaration_order() {
    let form = Form::for_op("set_config").unwrap().unwrap();
    let names: Vec<_> = form.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["key", "value", "scope"]);
    let form = Form::for_op("create_page").unwrap().unwrap();
    let names: Vec<_> = form.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["path", "parent", "title", "content"]);
}

#[test]
fn controls_follow_the_schema() {
    let form = Form::for_op("list_pages").unwrap().unwrap();
    let Control::Group(filter) = &form.field("filter").unwrap().control else {
        panic!("filter: {form:?}")
    };
    assert_eq!(
        filter.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["space", "path_prefix", "tag", "exact"]
    );
    assert!(matches!(
        form.field("limit").unwrap().control,
        Control::Integer { min: Some(0), .. }
    ));
    let Control::Choice {
        choices,
        selected: None,
    } = &form.field("sort").unwrap().control
    else {
        panic!("sort")
    };
    assert_eq!(choices.len(), 3);

    let form = Form::for_op("read_attachment").unwrap().unwrap();
    let Control::Choice { choices, .. } = &form.field("as").unwrap().control else {
        panic!("as")
    };
    assert_eq!(choices[1].value, "local_path");
    assert!(
        choices[1]
            .description
            .as_deref()
            .unwrap()
            .contains("absolute path")
    );

    let form = Form::for_op("children").unwrap().unwrap();
    assert_eq!(
        form.field("depth").unwrap().control,
        Control::Integer {
            text: "1".into(),
            min: Some(0)
        }
    );
    let form = Form::for_op("set_page_meta").unwrap().unwrap();
    assert!(matches!(
        form.field("meta").unwrap().control,
        Control::Json { object: true, .. }
    ));
}

/// Inputs exercising every control, which `fill` then `input` must give back.
fn samples() -> Vec<(&'static str, Value)> {
    vec![
        (
            "list_pages",
            json!({"filter": {"tag": "lang", "exact": true}, "limit": 5, "sort": "title"}),
        ),
        ("search", json!({"text": "async", "offset": 2})),
        (
            "check",
            json!({"kinds": ["broken_link", "heading_missing"], "scope": {"space": "eng"}}),
        ),
        (
            "edit_page",
            json!({"page": "a", "edits": [{"old": "x", "new": ""}, {"old": "y", "new": "z"}], "dry_run": true}),
        ),
        (
            "set_config",
            json!({"key": "links.syntax", "value": "wiki", "scope": "machine", "dry_run": false}),
        ),
        (
            "set_config",
            json!({"key": "k", "value": "123", "scope": "wiki", "dry_run": false}),
        ),
        (
            "set_config",
            json!({"key": "k", "value": null, "scope": "wiki", "dry_run": false}),
        ),
        (
            "set_page_meta",
            json!({"page": "a", "meta": {"order": 10, "title": null}, "dry_run": false}),
        ),
        (
            "add_attachment",
            json!({"page": "a", "name": "f.txt", "source": {"base64": "aGk="}, "dry_run": true}),
        ),
        (
            "tag_page",
            json!({"page": "a", "tags": ["x", "y/z"], "dry_run": false}),
        ),
        (
            "delete_page",
            json!({"page": "a", "recursive": true, "dry_run": false}),
        ),
        ("children", json!({"parent": "eng", "depth": 3})),
        ("list_spaces", json!({})),
        ("check", json!({})),
    ]
}

#[test]
fn fill_then_input_round_trips() {
    for (op, sample) in samples() {
        let mut form = Form::for_op(op).unwrap().unwrap();
        form.fill(&sample);
        let got = form.input().unwrap_or_else(|e| panic!("{op}: {e:?}"));
        assert_eq!(got, sample, "{op}");
    }
}

#[test]
fn bad_entries_name_their_field() {
    let mut form = Form::for_op("list_pages").unwrap().unwrap();
    form.fill(&json!({"limit": 5}));
    let Control::Integer { text, .. } = &mut form.field_mut("limit").unwrap().control else {
        panic!()
    };
    *text = "five".into();
    let errors = form.input().unwrap_err();
    assert_eq!(errors[0].path, "limit");
    form.fill(&json!({"limit": -1}));
    assert_eq!(form.input().unwrap_err()[0].message, "must be at least 0");

    let mut form = Form::for_op("check").unwrap().unwrap();
    form.fill(&json!({"kinds": ["nope"]}));
    assert_eq!(form.input().unwrap_err()[0].path, "kinds");

    let mut form = Form::for_op("edit_page").unwrap().unwrap();
    form.fill(&json!({"page": "a"}));
    form.field_mut("edits").unwrap().control.push();
    // The new record's strings are required but may be empty; `edits` itself is set.
    assert_eq!(
        form.input().unwrap(),
        json!({"page": "a", "edits": [{"old": "", "new": ""}], "dry_run": false})
    );

    let mut form = Form::for_op("set_page_meta").unwrap().unwrap();
    form.fill(&json!({"page": "a", "meta": "title"}));
    assert_eq!(form.input().unwrap_err()[0].path, "meta");

    // A required choice or JSON value left empty.
    let form = Form::for_op("set_config").unwrap().unwrap();
    let paths: Vec<_> = form
        .input()
        .unwrap_err()
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(paths, ["value", "scope"]);
}

#[test]
fn nested_errors_have_paths() {
    // No real Input nests a number yet, so build one: `outer.inner` and `rows[1].n`.
    let int = |name: &str, text: &str| Field {
        name: name.into(),
        description: None,
        required: true,
        control: Control::Integer {
            text: text.into(),
            min: None,
        },
    };
    let mut form = Form::for_op("list_spaces").unwrap().unwrap();
    form.fields = vec![
        Field {
            name: "outer".into(),
            description: None,
            required: false,
            control: Control::Group(vec![int("inner", "x")]),
        },
        Field {
            name: "rows".into(),
            description: None,
            required: false,
            control: Control::Records {
                template: vec![int("n", "")],
                records: vec![vec![int("n", "1")], vec![int("n", "")]],
            },
        },
    ];
    let errors: Vec<_> = form
        .input()
        .unwrap_err()
        .into_iter()
        .map(|e| e.to_string())
        .collect();
    assert_eq!(
        errors,
        [
            "outer.inner: `x` isn't a whole number",
            "rows[1].n: required"
        ]
    );
}

/// A form's Input runs: the dry-run toggle gives a Plan, and applying it creates the Page.
#[test]
fn form_input_runs_against_a_wiki() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("wiki")).unwrap();
    let wiki = Wiki::open_isolated(dir.path().join("wiki"), dir.path().join("cache")).unwrap();
    let op = find("create_page").unwrap();
    let mut form = Form::new(&op).unwrap();
    form.fill(&json!({"path": "eng/notes", "content": "# Notes\n"}));
    form.dry_run = Some(true);
    let planned = op.call(&wiki, form.input().unwrap()).unwrap();
    assert!(
        planned["result"]["plan"]["edits"]
            .as_array()
            .is_some_and(|e| !e.is_empty())
    );
    assert!(!dir.path().join("wiki/eng/notes.md").exists());

    form.dry_run = Some(false);
    op.call(&wiki, form.input().unwrap()).unwrap();
    assert!(dir.path().join("wiki/eng/notes.md").exists());
}

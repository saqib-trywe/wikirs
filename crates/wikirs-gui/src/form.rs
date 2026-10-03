//! The palette's form (interfaces.md#gui-and-tui), over `wikirs-ui`'s form
//! model: one row per value, with a text input for text-like controls,
//! toggles, choice buttons, and records that can be added to. Running it shows
//! the outcome, and a dry run's Plan can be applied.

use gpui_kit::{
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    prelude::FluentBuilder as _,
    *,
};
use serde_json::Value;
use wikirs_ui::{Control, FieldError, Form, Step, outcome::OutcomeLine};

/// A form being filled in, and what running it gave.
pub struct FormPanel {
    pub form: Form,
    /// A text input for each text-like row, by the row's path.
    inputs: Vec<(Vec<Step>, Entity<InputState>)>,
    pub errors: Vec<FieldError>,
    pub outcome: Option<Vec<OutcomeLine>>,
    /// A dry run's Input with `dry_run` off: what Apply runs.
    pub apply: Option<Value>,
    /// Enter in any input runs the form.
    subscriptions: Vec<Subscription>,
}

impl FormPanel {
    pub fn new<V: 'static>(form: Form, window: &mut Window, cx: &mut Context<V>) -> Self {
        let mut panel = Self {
            form,
            inputs: Vec::new(),
            errors: Vec::new(),
            outcome: None,
            apply: None,
            subscriptions: Vec::new(),
        };
        panel.make_inputs(window, cx);
        // Start typing in the first field that isn't filled in yet.
        let first = panel
            .inputs
            .iter()
            .find(|(_, input)| input.read(cx).value().is_empty())
            .or(panel.inputs.first())
            .map(|(_, input)| input.clone());
        if let Some(input) = first {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        panel
    }

    /// Calls `on_enter` when Enter is pressed in any of the form's inputs.
    pub fn on_enter<V: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<V>,
        on_enter: impl Fn(&mut V, &mut Window, &mut Context<V>) + Clone + 'static,
    ) {
        self.subscriptions = self
            .inputs
            .iter()
            .map(|(_, input)| {
                let on_enter = on_enter.clone();
                cx.subscribe_in(
                    input,
                    window,
                    move |this, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            on_enter(this, window, cx);
                        }
                    },
                )
            })
            .collect();
    }

    /// One input per text-like row that hasn't got one yet (records can be added).
    fn make_inputs<V: 'static>(&mut self, window: &mut Window, cx: &mut Context<V>) {
        for row in self.form.rows() {
            if self.inputs.iter().any(|(path, _)| *path == row.path) {
                continue;
            }
            let Some((_, control)) = self.form.at(&row.path) else {
                continue;
            };
            if let Some(text) = text_of(control) {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
                self.inputs.push((row.path, input));
            }
        }
    }

    /// The text input of the top-level field called `name`.
    #[must_use]
    pub fn field_input(&self, name: &str) -> Option<Entity<InputState>> {
        let i = self.form.fields.iter().position(|f| f.name == name)?;
        self.inputs
            .iter()
            .find(|(path, _)| *path == [Step::Field(i)])
            .map(|(_, input)| input.clone())
    }

    /// The inputs' text into the form's controls.
    pub fn sync(&mut self, cx: &App) {
        for (path, input) in &self.inputs {
            let text = input.read(cx).value().to_string();
            match self.form.control_mut(path) {
                Some(
                    Control::Text(t)
                    | Control::Integer { text: t, .. }
                    | Control::Json { text: t, .. },
                ) => {
                    *t = text;
                }
                Some(Control::List { items, .. }) => {
                    *items = if text.trim().is_empty() {
                        Vec::new()
                    } else {
                        text.split(',').map(|s| s.trim().to_string()).collect()
                    };
                }
                _ => {}
            }
        }
    }

    /// The Input, or the errors (shown next to their rows).
    pub fn input(&mut self, cx: &App) -> Option<Value> {
        self.sync(cx);
        match self.form.input() {
            Ok(input) => {
                self.errors.clear();
                Some(input)
            }
            Err(errors) => {
                self.errors = errors;
                None
            }
        }
    }

    pub fn push_record<V: 'static>(
        &mut self,
        path: &[Step],
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        self.sync(cx);
        self.form.push_record(path);
        self.make_inputs(window, cx);
    }

    /// The form's rows. `on` gets every change that isn't typing: a toggle,
    /// a choice, an added record.
    pub fn rows(
        &self,
        on: impl Fn(Change, &mut Window, &mut App) + Clone + 'static,
        cx: &App,
    ) -> Vec<AnyElement> {
        let theme = cx.theme();
        let mut out = Vec::new();
        for (i, row) in self.form.rows().into_iter().enumerate() {
            let Some((field, control)) = self.form.at(&row.path) else {
                continue;
            };
            let label = format!("{}{}", row.label, if field.required { " *" } else { "" });
            let widget: AnyElement = match control {
                Control::Bool { on: checked, .. } => {
                    let (on, path) = (on.clone(), row.path.clone());
                    Button::new(("toggle", i))
                        .label(if *checked { "☑" } else { "☐" })
                        .ghost()
                        .small()
                        .on_click(move |_, window, cx| on(Change::Toggle(path.clone()), window, cx))
                        .into_any_element()
                }
                Control::Choice { choices, selected } => h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(choices.iter().enumerate().map(|(c, choice)| {
                        let (on, path) = (on.clone(), row.path.clone());
                        Button::new(("choice", i * 100 + c))
                            .label(choice.value.clone())
                            .small()
                            .when(*selected == Some(c), ButtonVariants::primary)
                            .on_click(move |_, window, cx| {
                                on(Change::Choose(path.clone(), c), window, cx);
                            })
                    }))
                    .into_any_element(),
                Control::Records { records, .. } => {
                    let (on, path) = (on.clone(), row.path.clone());
                    h_flex()
                        .gap_2()
                        .child(format!("{} item(s)", records.len()))
                        .child(Button::new(("add", i)).label("Add").small().on_click(
                            move |_, window, cx| on(Change::AddRecord(path.clone()), window, cx),
                        ))
                        .into_any_element()
                }
                Control::Group(_) => div().into_any_element(),
                _ => match self.inputs.iter().find(|(path, _)| *path == row.path) {
                    Some((_, input)) => Input::new(input).small().into_any_element(),
                    None => div().into_any_element(),
                },
            };
            let error = self.errors.iter().find(|e| e.path == row.error_path);
            out.push(
                h_flex()
                    .gap_2()
                    .pl(px(16.) * f32::from(u8::try_from(row.depth).unwrap_or(u8::MAX)))
                    .child(
                        div()
                            .w(px(140.))
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(label),
                    )
                    .child(div().flex_1().child(widget))
                    .when_some(error, |d, e| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.danger)
                                .child(e.message.clone()),
                        )
                    })
                    .into_any_element(),
            );
        }
        for error in self
            .errors
            .iter()
            .filter(|e| !self.form.rows().iter().any(|r| r.error_path == e.path))
        {
            out.push(
                div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(error.to_string())
                    .into_any_element(),
            );
        }
        out
    }
}

/// A change to the form other than typing.
#[derive(Debug, Clone)]
pub enum Change {
    Toggle(Vec<Step>),
    Choose(Vec<Step>, usize),
    AddRecord(Vec<Step>),
}

impl Change {
    pub fn apply<V: 'static>(
        self,
        panel: &mut FormPanel,
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        match self {
            Change::Toggle(path) => {
                if let Some(Control::Bool { on, .. }) = panel.form.control_mut(&path) {
                    *on = !*on;
                }
            }
            Change::Choose(path, c) => {
                if let Some(Control::Choice { selected, .. }) = panel.form.control_mut(&path) {
                    *selected = if *selected == Some(c) { None } else { Some(c) };
                }
            }
            Change::AddRecord(path) => panel.push_record(&path, window, cx),
        }
    }
}

/// The text a text-like control starts with; `None` for other controls.
fn text_of(control: &Control) -> Option<String> {
    match control {
        Control::Text(t) | Control::Integer { text: t, .. } | Control::Json { text: t, .. } => {
            Some(t.clone())
        }
        Control::List { items, .. } => Some(items.join(", ")),
        _ => None,
    }
}

/// Outcome lines, coloured by kind.
pub fn outcome_view(lines: &[OutcomeLine], cx: &App) -> AnyElement {
    use wikirs_ui::outcome::Kind;
    let theme = cx.theme();
    v_flex()
        .p_2()
        .rounded_md()
        .bg(theme.secondary)
        .font_family(theme.mono_font_family.clone())
        .text_xs()
        .children(lines.iter().enumerate().map(|(i, l)| {
            let color = match l.kind {
                Kind::Error | Kind::Delete | Kind::Removed => Some(theme.danger),
                Kind::Applied | Kind::Create | Kind::Added => Some(theme.success),
                Kind::DryRun | Kind::Warning => Some(theme.warning),
                Kind::Move => Some(theme.info),
                Kind::Modify | Kind::Plain => None,
                Kind::Muted => Some(theme.muted_foreground),
            };
            div()
                .debug_selector(move || format!("diff-line-{i}"))
                .when_some(color, gpui_kit::Styled::text_color)
                .when(
                    matches!(
                        l.kind,
                        Kind::Error | Kind::Applied | Kind::DryRun | Kind::Modify
                    ),
                    |d| d.font_weight(FontWeight::SEMIBOLD),
                )
                .child(if l.text.is_empty() {
                    " ".to_string()
                } else {
                    l.text.clone()
                })
        }))
        .into_any_element()
}

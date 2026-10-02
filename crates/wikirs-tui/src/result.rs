//! An Operation's outcome as popup lines: `wikirs-ui`'s outcome lines, styled.

use ratatui::{
    style::{Color, Style, Stylize},
    text::Line,
};
use serde_json::Value;
use wikirs_core::Error;
use wikirs_ui::outcome::{self, Kind};

/// Lines for `{ result, warnings }` or an error.
#[must_use]
pub fn lines(outcome: &Result<Value, Error>) -> Vec<Line<'static>> {
    outcome::lines(outcome)
        .into_iter()
        .map(|l| {
            let line = Line::from(l.text);
            match l.kind {
                Kind::Error => line.red().bold(),
                Kind::Applied => line.green().bold(),
                Kind::DryRun => line.yellow().bold(),
                Kind::Create => line.green(),
                Kind::Delete => line.red(),
                Kind::Move => line.cyan(),
                Kind::Modify => line.bold(),
                Kind::Removed => line.style(Style::new().fg(Color::Red)),
                Kind::Added => line.style(Style::new().fg(Color::Green)),
                Kind::Warning => line.yellow(),
                Kind::Plain => line,
                Kind::Muted => line.dim(),
            }
        })
        .collect()
}

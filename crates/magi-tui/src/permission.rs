//! What `:permission` shows: who is asked about an action no rule allows, who else has a say,
//! and on what terms. A card like the model's, and settable in the same way — the rows that can
//! be changed take `◂ ▸`, and the rest is what a person needs in order to choose.

use crate::colour;
use magi_proto::judging::{Judging, Kind, Mode};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// The rows, and which of them may be taken.
#[derive(Default)]
pub struct Rendered {
    pub rows: Vec<Line<'static>>,
    pub picks: Vec<Option<String>>,
}

impl Rendered {
    fn push(&mut self, line: Line<'static>, pick: Option<&str>) {
        self.rows.push(line);
        self.picks.push(pick.map(ToOwned::to_owned));
    }

    fn say(&mut self, text: impl Into<String>, style: Style) {
        self.push(Line::from(Span::styled(text.into(), style)), None);
    }

    fn blank(&mut self) {
        self.push(Line::default(), None);
    }

    fn section(&mut self, title: &str, width: u16) {
        self.blank();
        let rule = "- ".repeat(usize::from(width) / 2);
        self.say(rule.trim_end(), Style::default().fg(colour::border()));
        self.say(
            title.to_owned(),
            Style::default()
                .fg(colour::text())
                .add_modifier(Modifier::BOLD),
        );
        self.blank();
    }

    /// A label and what it says, the labels in one column.
    fn fact(&mut self, label: &str, value: impl Into<String>) {
        self.push(
            Line::from(vec![
                Span::styled(format!("{label:<12}"), Style::default().fg(colour::dim())),
                Span::styled(value.into(), Style::default().fg(colour::text())),
            ]),
            None,
        );
    }

    /// A row a person may step through, drawn with the arrows that say so.
    fn steppable(&mut self, label: &str, value: Vec<Span<'static>>, id: &str) {
        let mut spans = vec![
            Span::styled(format!("{label:<12}"), Style::default().fg(colour::dim())),
            Span::styled("◂ ", Style::default().fg(colour::dim())),
        ];
        spans.extend(value);
        spans.push(Span::styled(" ▸", Style::default().fg(colour::dim())));
        self.push(Line::from(spans), Some(id));
    }
}

/// How wide the band is drawn, in characters.
const SLIDER: usize = 34;

/// What each mode does, in the words the card says it in.
fn does(mode: Mode) -> &'static str {
    match mode {
        Mode::Ask => "you are asked about anything no rule allows",
        Mode::Edits => "edits in this directory go ahead; you are asked about the rest",
        Mode::Auto => "a second model decides, and you are asked when it cannot",
        Mode::Locked => "anything no rule allows is refused, not asked about",
    }
}

/// The card.
#[must_use]
pub fn view(judging: &Judging, width: u16) -> Rendered {
    let width = width.max(20);
    let mut out = Rendered::default();
    let dim = Style::default().fg(colour::dim());

    out.say(
        "Permission",
        Style::default()
            .fg(colour::text())
            .add_modifier(Modifier::BOLD),
    );
    out.say(does(judging.mode), dim);
    out.blank();
    out.steppable(
        "Mode",
        vec![Span::styled(
            judging.mode.name().to_owned(),
            Style::default().fg(colour::accent()),
        )],
        "mode",
    );

    second_model(&mut out, judging);
    band(&mut out, judging, width);
    rules(&mut out, judging, width);
    out
}

/// Who else has a say, and of what kind. A model that writes answers in prose and is taken at
/// its word; one that decides answers with a number, which is what the band is drawn across.
fn second_model(out: &mut Rendered, judging: &Judging) {
    let dim = Style::default().fg(colour::dim());
    let said = match judging.kind {
        Kind::None => Span::styled(
            "nobody — set `magi.helpers.safety` to a model".to_owned(),
            dim,
        ),
        _ => Span::styled(
            judging.model.clone().unwrap_or_default(),
            Style::default().fg(colour::text()),
        ),
    };
    out.push(
        Line::from(vec![Span::styled(format!("{:<12}", "Second"), dim), said]),
        None,
    );
    let kind = match judging.kind {
        Kind::None => "",
        Kind::Writes => "writes — asked in words, answers in prose",
        Kind::Decides => "decides — typed questions, answers with a number",
    };
    if !kind.is_empty() {
        out.push(
            Line::from(vec![
                Span::raw(" ".repeat(12)),
                Span::styled(kind.to_owned(), dim),
            ]),
            None,
        );
    }
}

/// The band, drawn as what it is: the middle of a line from certainly-not to certainly-yes, and
/// the part of it that comes back to the person.
fn band(out: &mut Rendered, judging: &Judging, width: u16) {
    let dim = Style::default().fg(colour::dim());
    out.section("How sure it has to be", width);
    if judging.kind != Kind::Decides {
        out.say("  only a model that decides says how sure it is,", dim);
        out.say("  so there is no band to draw and none to set", dim);
        return;
    }
    let (low, high) = judging.unsure;
    let at = |v: f64| ((v * SLIDER as f64).round() as usize).min(SLIDER);
    let (from, to) = (at(low), at(high));
    out.steppable(
        "Band",
        vec![
            Span::styled("█".repeat(from), Style::default().fg(colour::error())),
            Span::styled(
                "░".repeat(to.saturating_sub(from)),
                Style::default().fg(colour::warning()),
            ),
            Span::styled(
                "█".repeat(SLIDER.saturating_sub(to)),
                Style::default().fg(colour::success()),
            ),
        ],
        "band",
    );
    out.push(
        Line::from(vec![
            Span::raw(" ".repeat(14)),
            Span::styled(
                format!("refused <{low:.2}"),
                Style::default().fg(colour::error()),
            ),
            Span::styled(
                format!("   you {low:.2}–{high:.2}"),
                Style::default().fg(colour::warning()),
            ),
            Span::styled(
                format!("   allowed >{high:.2}"),
                Style::default().fg(colour::success()),
            ),
        ]),
        None,
    );
    out.blank();
    out.say(
        "  wider means more comes to you, and less is decided for you",
        dim,
    );
    if judging.judged > 0 {
        out.fact(
            "Judged",
            format!(
                "{}, refused {} of them, {} in a row",
                judging.judged, judging.refused, judging.in_a_row
            ),
        );
    }
}

/// What holds whatever the mode is, and whatever a second model says.
fn rules(out: &mut Rendered, judging: &Judging, width: u16) {
    let dim = Style::default().fg(colour::dim());
    out.section("Rules, in every mode", width);
    let mut listed = |title: &str, rules: &[String], ink: Style| {
        if rules.is_empty() {
            return;
        }
        out.say(format!("  {title}"), ink);
        for rule in rules {
            out.say(format!("    {rule}"), Style::default().fg(colour::text()));
        }
    };
    listed(
        "never, and never asked about  `magi.deny`",
        &judging.denied,
        Style::default().fg(colour::error()),
    );
    listed(
        "always you, whatever the mode  `magi.ask`",
        &judging.always_asked,
        Style::default().fg(colour::warning()),
    );
    if judging.denied.is_empty() && judging.always_asked.is_empty() {
        out.say("  none set — `magi.deny` and `magi.ask` take them", dim);
    }
    out.blank();
    out.say(
        "  the kernel jail is under all of this, whatever is set here",
        dim,
    );
}

#[cfg(test)]
mod tests;

use super::{App, Picking};
use magi_model::ThinkingLevel;

impl App {
    pub(super) fn thinking_levels(&self) -> Vec<ThinkingLevel> {
        let choice = self
            .model
            .as_ref()
            .and_then(|model| self.choices.iter().find(|choice| choice.name == model.name));
        ThinkingLevel::available(
            choice.map_or(self.model_reasons, |choice| choice.reasoning),
            choice.and_then(|choice| choice.capabilities.as_ref()),
        )
    }

    pub fn open_thinking_picker(&mut self) {
        let available = self.thinking_levels();
        let choices = ThinkingLevel::ALL
            .into_iter()
            .map(|level| {
                let ready = available.contains(&level);
                let detail = if !ready {
                    "not selectable for this model"
                } else {
                    match level {
                        ThinkingLevel::Off => {
                            "disabled where supported; otherwise the provider minimum"
                        }
                        ThinkingLevel::Minimal => "the smallest budget the model offers",
                        ThinkingLevel::Low => "a small budget",
                        ThinkingLevel::Medium => "the usual budget",
                        ThinkingLevel::High => "a large budget",
                        ThinkingLevel::XHigh => "extra high reasoning effort",
                        ThinkingLevel::Max => "the largest budget the model offers",
                    }
                };
                magi_tui::picker::Choice {
                    value: level.as_str().into(),
                    detail: detail.into(),
                    ready,
                }
            })
            .collect();
        self.overlay =
            Some(magi_tui::picker::Picker::new("Thinking", choices, Some(&self.thinking)).into());
        self.picking = Some(Picking::Thinking);
    }

    pub fn open_profile_picker(&mut self) {
        if self.profiles.is_empty() {
            self.show_notice(
                "No profiles configured. Add `magi.profiles` in init.lua or `~/.config/magi/profiles/*.md`."
                    .to_owned(),
            );
            return;
        }
        let mut choices: Vec<magi_tui::picker::Choice> = self
            .profiles
            .iter()
            .map(|(name, summary)| magi_tui::picker::Choice {
                value: name.clone(),
                detail: summary.clone(),
                ready: true,
            })
            .collect();
        choices.push(magi_tui::picker::Choice {
            value: "default".to_owned(),
            detail: "base session configuration (no active profile)".to_owned(),
            ready: true,
        });
        let current = self.profile.as_deref().unwrap_or("default");
        self.overlay =
            Some(magi_tui::picker::Picker::new("Profile", choices, Some(current)).into());
        self.picking = Some(Picking::Profile);
    }

    /// The next profile name when cycling (`alt+p` / `:profile next`), wrapping back to `"default"`.
    #[must_use]
    pub fn next_profile(&self) -> Option<String> {
        if self.profiles.is_empty() {
            return None;
        }
        match self.profile.as_deref() {
            None => self.profiles.first().map(|(n, _)| n.clone()),
            Some(current) => {
                let idx = self.profiles.iter().position(|(n, _)| n == current);
                match idx.and_then(|i| self.profiles.get(i + 1)) {
                    Some((n, _)) => Some(n.clone()),
                    None => Some("default".to_owned()),
                }
            }
        }
    }

    pub(super) fn thinking_step(&self, forward: bool, wrap: bool) -> Option<&'static str> {
        let levels = self.thinking_levels();
        let current = levels
            .iter()
            .position(|level| level.as_str() == self.thinking);
        let next = match current {
            None => {
                if forward {
                    levels.first()
                } else {
                    levels.last()
                }
            }
            Some(at) if forward => levels
                .get(at + 1)
                .or_else(|| wrap.then(|| levels.first()).flatten()),
            Some(at) => at
                .checked_sub(1)
                .and_then(|at| levels.get(at))
                .or_else(|| wrap.then(|| levels.last()).flatten()),
        }?;
        (next.as_str() != self.thinking).then(|| next.as_str())
    }

    pub(super) fn refresh_thinking_controls(&mut self) {
        self.model_reasons = self.model.as_ref().is_some_and(|model| {
            self.choices
                .iter()
                .any(|choice| choice.name == model.name && choice.reasoning)
        });
        if self.picking == Some(Picking::Thinking) && self.overlay.is_some() {
            self.open_thinking_picker();
        }
        if self.pane_titled("model") {
            self.show_model();
        }
    }
}

#[cfg(test)]
mod tests;

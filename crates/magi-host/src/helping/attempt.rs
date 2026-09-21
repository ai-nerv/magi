//! Attempt-local helper output and request-wide reported usage.

use magi_core::{Turn, TurnState};
use magi_model::{Delta, StopReason, Usage};

#[derive(Default)]
pub(super) struct Attempt {
    turn: Turn,
    spent: Usage,
}

impl Attempt {
    pub(super) fn apply(&mut self, delta: Delta) {
        self.turn.apply(delta);
    }

    pub(super) fn retry(&mut self) {
        self.spent = self.usage();
        self.turn = Turn::new();
    }

    pub(super) fn usage(&self) -> Usage {
        let latest = self.turn.usage();
        Usage {
            input: self.spent.input.saturating_add(latest.input),
            output: self.spent.output.saturating_add(latest.output),
            cache_read: self.spent.cache_read.saturating_add(latest.cache_read),
            cache_write: self.spent.cache_write.saturating_add(latest.cache_write),
            cost_micros: self.spent.cost_micros.saturating_add(latest.cost_micros),
        }
    }

    pub(super) fn text(&self, structured: bool) -> Result<String, String> {
        if !matches!(
            self.turn.state(),
            TurnState::ToolsPending
                | TurnState::Finished(StopReason::EndTurn | StopReason::ToolUse)
        ) {
            return Err(format!(
                "helper response did not complete: {:?}",
                self.turn.state()
            ));
        }
        let text = match self.turn.attempted_calls() {
            [] => self.turn.text(),
            [call] => &call.arguments,
            _ => return Err("helper answered with multiple tool calls".into()),
        }
        .trim();
        if text.is_empty() {
            return Err(format!(
                "helper answered nothing ({} thinking chars)",
                self.turn.thinking().chars().count()
            ));
        }
        if structured || !self.turn.attempted_calls().is_empty() {
            serde_json::from_str::<serde_json::Value>(text)
                .map_err(|_| "helper did not answer with one complete JSON value".to_owned())?;
        }
        Ok(text.to_owned())
    }
}

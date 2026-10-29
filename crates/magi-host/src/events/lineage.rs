use super::{Correlation, Id};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    pub run_id: Option<Id>,
    pub agent_id: Option<Id>,
    pub turn_id: Option<Id>,
    pub ask_id: Option<Id>,
}

impl Lineage {
    pub fn root(run: Id, agent: Id) -> Self {
        Self {
            run_id: Some(run),
            agent_id: Some(agent),
            ..Self::default()
        }
    }

    pub(crate) fn valid(self) -> bool {
        match (self.run_id, self.agent_id) {
            (Some(_), Some(_)) => true,
            (None, None) => self.turn_id.is_none() && self.ask_id.is_none(),
            _ => false,
        }
    }

    pub(crate) fn apply(self, mut correlation: Correlation) -> Correlation {
        correlation.run_id = self.run_id.or(correlation.run_id);
        correlation.agent_id = self.agent_id.or(correlation.agent_id);
        correlation.turn_id = self.turn_id.or(correlation.turn_id);
        correlation.ask_id = self.ask_id.or(correlation.ask_id);
        correlation
    }

    pub fn accepts(self, correlation: Correlation) -> bool {
        [
            (self.run_id, correlation.run_id),
            (self.agent_id, correlation.agent_id),
            (self.turn_id, correlation.turn_id),
            (self.ask_id, correlation.ask_id),
        ]
        .into_iter()
        .all(|(bound, actual)| bound.is_none() || bound == actual)
    }
}

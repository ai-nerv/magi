//! Where axon meets the agent layer.
//!
//! Everything about naming, finding, reaching and refusing other instances lives in
//! [`axon_agent`], which knows nothing about axon and is meant to leave the workspace. This
//! module is the seam: the handful of places where a thing that crate produces has to become a
//! thing axon understands.
//!
//! It is deliberately thin, and thinness is the measure. Three adaptations, and no logic:
//!
//! - a [`verbs::Answer`] becomes an [`axon_tools::Output`]
//! - a descriptor becomes something implementing [`axon_tools::Tool`]
//! - an [`App`](crate::app::App) becomes a [`verbs::Standing`]
//!
//! Anything that starts to look like a *decision* here belongs on the other side of the seam.
//! The day this file is deciding something is the day the split stopped being real.

pub mod peer;

// Re-exported rather than reached for, so `crate::instance::` is the one place axon names the
// agent layer. When the crate leaves, this list is the whole of what has to be repointed.
pub use axon_agent::directory::{
    ID, PROJECT, ROLE, TOOL, announce, children, forget, host_at, inbox_of, listening,
    listening_at, mine, parent, socket, token,
};
pub use axon_agent::{Identity, answering, asking, policy, serving, wire};

use axon_agent::verbs;
use axon_tools::{Cancel, Ops, Output, Tool};
use serde_json::Value;

/// The `agent` tool, as axon's registry needs it.
///
/// The vocabulary, the schema and every verb's behaviour come from [`axon_agent::verbs`]. What
/// is here is the shape a tool has *in axon* — a trait with four methods — and nothing else.
/// That is the whole reason the crate hands its surface over as data: a tool trait is a harness
/// idea, and a crate that implemented one could not be lifted out of the harness.
pub struct Agent {
    /// What this session knows about itself and its neighbours.
    pub standing: verbs::Standing,
}

impl Tool for Agent {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        // Leaked from the descriptor once and kept, because `description` hands back a borrow
        // and the descriptor is built fresh. It is one string for the life of the process.
        static SAID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        SAID.get_or_init(|| {
            verbs::described()["description"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
    }

    fn parameters(&self) -> Value {
        verbs::parameters()
    }

    fn run(&self, arguments: &Value, _ops: &dyn Ops, _cancel: &dyn Cancel) -> Output {
        let answer = verbs::answer(arguments, &self.standing);
        if answer.failed {
            Output::error(answer.said)
        } else {
            Output::ok(answer.said)
        }
    }
}

/// What the model is told about the instances a prompt named.
///
/// The scan is axon's — a prompt, a cursor and a table of sigils are all things only a harness
/// has — and what is known about a name is the agent layer's. So this hands one to the other.
#[must_use]
pub fn briefing(text: &str, standing: &verbs::Standing) -> String {
    let named = axon_tui::trigger::named(text, axon_tui::trigger::Trigger::Instance);
    axon_agent::briefing::about(&named, standing)
}

/// The seam is thin, and stays thin.
#[cfg(test)]
mod tests {
    use super::*;

    fn standing() -> verbs::Standing {
        verbs::Standing {
            me: "axon/main/alpha-rho".to_owned(),
            ..verbs::Standing::default()
        }
    }

    #[test]
    fn the_tool_axon_registers_is_the_one_the_layer_describes() {
        // Two copies of nineteen verb descriptions would disagree the first time one was
        // edited. There is one, and this is the wire it comes over.
        let agent = Agent {
            standing: standing(),
        };
        let described = verbs::described();
        assert_eq!(agent.name(), described["name"].as_str().expect("a name"));
        assert_eq!(
            agent.description(),
            described["description"].as_str().expect("a description")
        );
        assert_eq!(agent.parameters(), described["parameters"]);
    }

    #[test]
    fn a_refusal_arrives_as_a_failed_result_rather_than_as_prose() {
        // The one thing the adaptation has to get right. A refusal that came back as a
        // successful result would read to the model as "that worked", and it would carry on.
        let agent = Agent {
            standing: standing(),
        };
        let out = agent.run(
            &serde_json::json!({"verb": "nonsense"}),
            &axon_tools::ops::Real::new(std::path::PathBuf::from(".")),
            &axon_tools::Uncancelled,
        );
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("nonsense"), "{}", out.content);
    }

    #[test]
    fn a_prompt_naming_nobody_is_briefed_about_nothing() {
        assert!(briefing("fix the parser", &standing()).is_empty());
    }

    #[test]
    fn a_prompt_naming_somebody_is_briefed_about_them() {
        // The scan is axon's and the facts are the layer's; this is the one place they meet.
        let said = briefing("ask $beta-nu about it", &standing());
        assert!(said.contains("axon/main/beta-nu"), "{said}");
    }
}

//! Asking the model what it is going to need, before it starts.
//!
//! **Why ask at all.** Permission prompts arrive one at a time, in the middle of work, and each
//! one is a decision made with no idea how many more are coming. Answering "yes" four times to
//! four narrow questions is a worse outcome than answering once to an accurate description of
//! the whole job — and the only party that knows the shape of the job before it starts is the
//! model.
//!
//! So it is asked, in a [schema](axon_provider::api::Schema) rather than in prose: what verbs,
//! on what, and why. The answer is a *proposal*, not a decision. It is turned into the same
//! prompt every other request goes through, and the person can grant all of it, some of it, or
//! none — the model has no more authority over the ledger than it had before.
//!
//! **What it is not.** It is not a security boundary. A model that under-declares still hits the
//! per-action gate on everything it did not mention, which is exactly what that gate is for.
//! This buys fewer interruptions, not more trust.

use axon_proto::permit::{Grant, Scope};
use axon_provider::api::Schema;

/// What the model is asked, as a message of its own.
///
/// The schema says what shape to answer in; it does not say what the question is. A conversation
/// handed to a provider with a schema and no question ends on whatever was said last — usually
/// the model's own answer — and a model asked to continue its own turn says nothing at all,
/// which arrives here as an empty body and an answer that will not parse.
///
/// `here` is named in it because a scope has to be a thing the ledger can hold. Asked without
/// it, the answer came back as "project root" three times over, and the person was offered "any
/// `project root` command" — a sentence that means nothing and grants nothing.
#[must_use]
pub fn question(here: &std::path::Path) -> String {
    format!(
        "Before going further: what permissions does the work ahead need? Answer with the needs \
         themselves — the verb, what it applies to, and one clause saying why. Ask for the least \
         that would let the work proceed. The working directory is {}. Each scope is one path or \
         one program name and nothing else — a full path such as {} for read and write, and the \
         word you would type for run: `cargo`, `git`.",
        here.display(),
        here.display()
    )
}

/// The shape the model is asked to answer in.
///
/// Deliberately small. A model asked for an elaborate object fills it in elaborately, and every
/// field is another thing a person has to read before deciding.
#[must_use]
pub fn schema() -> Schema {
    Schema {
        name: "permissions".to_owned(),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "needs": {
                    "type": "array",
                    "description": "Only what this task actually requires. Fewer is better.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "verb": {
                                "type": "string",
                                "enum": ["read", "write", "run", "reach"],
                            },
                            "scope": {
                                "type": "string",
                                "description":
                                    "One path, one program name, or one hostname, on its own: \
                                     `/home/you/project`, `cargo`, `api.example.com`. Not a \
                                     phrase — whatever is written here is read back to the \
                                     person as the thing being granted.",
                            },
                            "why": {
                                "type": "string",
                                "description": "One short clause. The person reads this.",
                            },
                        },
                        "required": ["verb", "scope", "why"],
                        "additionalProperties": false,
                    },
                },
            },
            "required": ["needs"],
            "additionalProperties": false,
        }),
    }
}

/// What the model asked for, in the form it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Need {
    /// One of `read`, `write`, `run`, `reach`.
    pub verb: String,
    /// A directory, a program, or a host.
    pub scope: String,
    /// Why, in the model's words, for the person deciding.
    pub why: String,
}

impl Need {
    /// The grant this would become, if allowed.
    ///
    /// A `run` need becomes a program, everything else a directory-or-host. A verb this does not
    /// recognise becomes nothing: a model that invents one has not asked for anything, and
    /// guessing what it meant would put a sentence in front of somebody that does not match what
    /// would actually be granted.
    #[must_use]
    pub fn grant(&self) -> Option<Grant> {
        if self.scope.trim().is_empty() {
            return None;
        }
        let scope = match self.verb.as_str() {
            "run" => Scope::Program {
                program: self.scope.clone(),
            },
            "read" | "write" | "reach" => Scope::Directory {
                path: self.scope.clone(),
            },
            _ => return None,
        };
        Some(Grant {
            verb: self.verb.clone(),
            scope,
        })
    }
}

/// Read the model's answer, dropping anything malformed.
///
/// Lenient about the envelope and strict about each entry: a schema-honouring provider returns
/// exactly this shape, and one that does not has produced something nobody should act on.
#[must_use]
pub fn read(value: &serde_json::Value) -> Vec<Need> {
    let Some(needs) = value.get("needs").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    needs
        .iter()
        .filter_map(|need| {
            Some(Need {
                verb: need.get("verb")?.as_str()?.to_owned(),
                scope: need.get("scope")?.as_str()?.to_owned(),
                why: need
                    .get("why")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_names_the_directory_the_answer_is_about() {
        // Without it the answer came back as "project root" for all three needs, and the person
        // was offered "any `project root` command".
        let asked = question(std::path::Path::new("/home/you/work"));
        assert!(asked.contains("/home/you/work"), "{asked}");
    }

    #[test]
    fn the_schema_names_the_verbs_the_ledger_understands() {
        // A model offered a verb the ledger cannot store would be asked to declare something
        // that could never be granted.
        let schema = schema();
        let verbs = &schema.schema["properties"]["needs"]["items"]["properties"]["verb"]["enum"];
        assert_eq!(verbs, &serde_json::json!(["read", "write", "run", "reach"]));
    }

    #[test]
    fn a_declared_need_becomes_the_grant_it_describes() {
        let need = Need {
            verb: "run".into(),
            scope: "git".into(),
            why: "to read the history".into(),
        };
        assert_eq!(
            need.grant(),
            Some(Grant {
                verb: "run".into(),
                scope: Scope::Program {
                    program: "git".into()
                },
            })
        );
    }

    #[test]
    fn a_path_need_becomes_a_directory() {
        let need = Need {
            verb: "write".into(),
            scope: "/home/x/work".into(),
            why: "to edit the source".into(),
        };
        assert!(matches!(
            need.grant(),
            Some(Grant {
                scope: Scope::Directory { .. },
                ..
            })
        ));
    }

    #[test]
    fn an_invented_verb_grants_nothing() {
        // Guessing what it meant would show somebody a sentence that does not match what would
        // actually be granted.
        let need = Need {
            verb: "delete".into(),
            scope: "/".into(),
            why: "".into(),
        };
        assert_eq!(need.grant(), None);
    }

    #[test]
    fn an_empty_scope_grants_nothing() {
        let need = Need {
            verb: "read".into(),
            scope: "   ".into(),
            why: "".into(),
        };
        assert_eq!(need.grant(), None);
    }

    #[test]
    fn a_well_formed_answer_is_read() {
        let value = serde_json::json!({
            "needs": [
                { "verb": "read", "scope": "/home/x/work", "why": "to see the code" },
                { "verb": "run", "scope": "cargo", "why": "to run the tests" },
            ]
        });
        let needs = read(&value);
        assert_eq!(needs.len(), 2);
        assert_eq!(needs[1].scope, "cargo");
        assert_eq!(needs[0].why, "to see the code");
    }

    #[test]
    fn a_malformed_entry_is_dropped_rather_than_guessed_at() {
        let value = serde_json::json!({
            "needs": [
                { "verb": "read" },
                { "verb": "run", "scope": "git", "why": "history" },
            ]
        });
        assert_eq!(read(&value).len(), 1);
    }

    #[test]
    fn an_answer_of_the_wrong_shape_asks_for_nothing() {
        assert!(read(&serde_json::json!({ "oops": true })).is_empty());
    }
}

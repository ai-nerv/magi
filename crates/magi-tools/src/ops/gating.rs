//! What the permission gate does when it is asked, and what it does when it is adopted.
//!
//! Split out under THE RULE; the ops they exercise are next door.

// Brought in here so the test modules below can keep saying `use super::*` and mean what they
// meant when they lived in `ops.rs` -- a private import is visible to descendant modules.
use super::*;

#[cfg(test)]
mod gate_tests {
    use super::*;
    use magi_model::scratch::Scratch;
    use magi_proto::permit::{Action, Decision, Lifetime, Scope};
    use std::sync::Arc;

    /// An approver that answers from a script and records what it was asked.
    struct Scripted {
        answers: std::sync::Mutex<Vec<Decision>>,
        asked: std::sync::Mutex<Vec<Action>>,
    }

    impl Scripted {
        fn new(answers: Vec<Decision>) -> Arc<Self> {
            Arc::new(Self {
                answers: std::sync::Mutex::new(answers),
                asked: std::sync::Mutex::new(Vec::new()),
            })
        }
        fn asked(&self) -> Vec<Action> {
            self.asked.lock().map(|a| a.clone()).unwrap_or_default()
        }
    }

    impl crate::approve::Approver for Scripted {
        fn ask(&self, _tool: &str, action: &Action) -> Decision {
            if let Ok(mut asked) = self.asked.lock() {
                asked.push(action.clone());
            }
            self.answers
                .lock()
                .ok()
                .and_then(|mut a| {
                    if a.is_empty() {
                        None
                    } else {
                        Some(a.remove(0))
                    }
                })
                .unwrap_or(Decision::Deny)
        }
    }

    fn scratch(name: &str) -> Scratch {
        Scratch::new("magi-gate", name)
    }

    #[test]
    fn an_ungated_ops_asks_nobody() {
        // Every `Ops` but `Real` is a test double, and one that had to be taught about
        // permissions would make every tool test a permissions test.
        let dir = scratch("ungated");
        let ops = Real::new(dir.to_path_buf());
        assert!(ops.allow("t", &Action::Read { path: "/x".into() }).is_ok());
    }

    #[test]
    fn a_gated_ops_asks_and_a_refusal_reaches_the_model() {
        let dir = scratch("refused");
        let approver = Scripted::new(vec![Decision::Deny]);
        let ops = Real::gated(
            dir.to_path_buf(),
            crate::permit::Ledger::new(),
            approver.clone(),
        );
        let why = ops
            .allow(
                "t",
                &Action::Read {
                    path: "/etc/shadow".into(),
                },
            )
            .expect_err("refused");
        assert!(why.contains("not permitted"), "{why}");
        assert!(
            why.contains("/etc/shadow"),
            "it says what was refused: {why}"
        );
        assert_eq!(approver.asked().len(), 1);
    }

    #[test]
    fn a_directory_answer_means_the_next_file_is_not_asked_about() {
        // The difference between a permission prompt and a nuisance.
        let dir = scratch("once-only");
        let approver = Scripted::new(vec![Decision::Allow {
            scope: Scope::Directory {
                path: "/home/x/work".into(),
            },
            lifetime: Lifetime::Session,
        }]);
        let ops = Real::gated(
            dir.to_path_buf(),
            crate::permit::Ledger::new(),
            approver.clone(),
        );
        for file in ["a.rs", "b.rs", "c.rs"] {
            ops.allow(
                "t",
                &Action::Read {
                    path: format!("/home/x/work/{file}"),
                },
            )
            .expect("allowed");
        }
        assert_eq!(approver.asked().len(), 1, "asked once, not three times");
    }

    #[test]
    fn a_gated_session_is_still_confined() {
        // `magi.confine = true` used to be dropped whenever there was somebody to ask, so it
        // held only for headless runs. Asserted with an approver that would allow anything: if
        // the wall works, the question is never reached, and an allowing approver proves that
        // more sharply than a refusing one — a refusal would pass either way.
        let dir = scratch("confined-gate");
        std::fs::create_dir_all(dir.join("inside")).expect("mkdir");
        std::fs::create_dir_all(dir.join("outside")).expect("mkdir");
        // The file has to *exist*, or the read fails because it is missing and the test passes
        // whether or not the wall is there. Confinement must be the only thing in the way.
        std::fs::write(dir.join("outside/secret"), "a key").expect("write");

        let approver = Scripted::new(vec![Decision::Allow {
            scope: Scope::Anything,
            lifetime: Lifetime::Session,
        }]);
        let ops = Real::gated(
            dir.join("inside"),
            crate::permit::Ledger::new(),
            approver.clone(),
        )
        .confining(true);

        // Reachable without the wall: the same path, read by an ops that is gated and not
        // confined, comes back with the contents.
        let open = Real::gated(
            dir.join("inside"),
            crate::permit::Ledger::new(),
            approver.clone(),
        );
        assert_eq!(
            open.read(Path::new("../outside/secret")).as_deref(),
            Ok("a key"),
            "the file is readable when nothing is confining"
        );

        assert!(
            ops.read(Path::new("../outside/secret")).is_err(),
            "confinement holds even with an approver that allows everything"
        );
    }

    #[test]
    fn a_grant_on_a_directory_does_not_cover_a_path_that_climbs_out_of_it() {
        // The escape this closes. `..` is resolved before the question is asked, so the subject
        // the person sees and the file that opens are one string. Asked with the raw join, the
        // grant on `work` covered `work/sub/../../secret/id_rsa` — it starts with the root — and
        // the read went through without a second question.
        let dir = scratch("climbing");
        let root = dir.join("work");
        std::fs::create_dir_all(root.join("sub")).expect("mkdir");
        std::fs::create_dir_all(dir.join("secret")).expect("mkdir");
        std::fs::write(dir.join("secret/id_rsa"), "key").expect("write");

        let approver = Scripted::new(vec![
            Decision::Allow {
                scope: Scope::Directory {
                    path: root.display().to_string(),
                },
                lifetime: Lifetime::Session,
            },
            Decision::Deny,
        ]);
        let ops = Real::gated(
            dir.to_path_buf(),
            crate::permit::Ledger::new(),
            approver.clone(),
        );
        ops.allow(
            "t",
            &Action::Read {
                path: ops.resolved(Path::new("work/a.txt")).display().to_string(),
            },
        )
        .expect("allowed inside the grant");

        // Climbing out is a second question, and this approver denies it.
        let out = ops.resolved(Path::new("work/sub/../../secret/id_rsa"));
        assert_eq!(out, dir.join("secret/id_rsa"), "the subject is normalised");
        assert!(
            ops.allow(
                "t",
                &Action::Read {
                    path: out.display().to_string(),
                },
            )
            .is_err(),
            "a grant on `work` must not cover a path that leaves it"
        );
    }

    #[test]
    fn a_grant_for_one_directory_does_not_cover_another() {
        let dir = scratch("elsewhere");
        let approver = Scripted::new(vec![
            Decision::Allow {
                scope: Scope::Directory {
                    path: "/home/x/work".into(),
                },
                lifetime: Lifetime::Session,
            },
            Decision::Deny,
        ]);
        let ops = Real::gated(
            dir.to_path_buf(),
            crate::permit::Ledger::new(),
            approver.clone(),
        );
        ops.allow(
            "t",
            &Action::Read {
                path: "/home/x/work/a".into(),
            },
        )
        .expect("allowed");
        assert!(
            ops.allow(
                "t",
                &Action::Read {
                    path: "/home/x/secrets/a".into()
                }
            )
            .is_err(),
            "a second directory is a second question"
        );
        assert_eq!(approver.asked().len(), 2);
    }

    #[test]
    fn what_was_granted_can_be_written_down() {
        let dir = scratch("grants");
        let approver = Scripted::new(vec![Decision::Allow {
            scope: Scope::Program {
                program: "git".into(),
            },
            lifetime: Lifetime::Always,
        }]);
        let ops = Real::gated(dir.to_path_buf(), crate::permit::Ledger::new(), approver);
        ops.allow(
            "t",
            &Action::Run {
                command: "git status".into(),
                program: "git".into(),
            },
        )
        .expect("allowed");
        assert_eq!(ops.grants().len(), 1);
    }
}

/// Grants a parent lends change what a child may actually do.
#[cfg(test)]
mod taking_on {
    use super::*;
    use magi_proto::permit::{Action, Decision, Grant, Scope};

    fn run(command: &str) -> Action {
        Action::Run {
            command: command.to_owned(),
            program: command.split_whitespace().next().unwrap_or("").to_owned(),
        }
    }

    /// An approver that refuses everything, so anything allowed came from the ledger.
    struct Refuses;
    impl crate::approve::Approver for Refuses {
        fn ask(&self, _tool: &str, _action: &Action) -> Decision {
            Decision::Deny
        }
    }

    #[test]
    fn what_a_parent_lends_is_what_the_child_may_do() {
        // The end of the chain. Everything before this — the prompt, the pipe, the socket — is
        // plumbing for exactly this effect, and without it a child would be told it had been
        // adopted and then be refused every command anyway.
        let ops = Real::gated(
            std::env::temp_dir(),
            crate::permit::Ledger::new(),
            std::sync::Arc::new(Refuses),
        );
        assert!(
            ops.allow("shell", &run("git status")).is_err(),
            "it should start with nothing"
        );

        ops.take_on(vec![Grant {
            verb: "run".to_owned(),
            scope: Scope::Program {
                program: "git".to_owned(),
            },
        }]);

        assert!(
            ops.allow("shell", &run("git status")).is_ok(),
            "the lent grant did not reach the ledger"
        );
        // And no further than what was lent. A child gets what its parent holds and nothing more.
        assert!(
            ops.allow("shell", &run("rm -rf /")).is_err(),
            "it took on more than it was lent"
        );
    }

    #[test]
    fn a_session_that_gates_nothing_is_unchanged_by_being_lent_something() {
        // An ungated session already allows everything; writing a rule into it would decide
        // nothing and suggest it had.
        let ops = Real::new(std::env::temp_dir());
        ops.take_on(vec![Grant {
            verb: "run".to_owned(),
            scope: Scope::Anything,
        }]);
        assert!(ops.allow("shell", &run("anything")).is_ok());
    }
}

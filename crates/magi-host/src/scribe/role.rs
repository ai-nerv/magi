use super::ROLE;

#[test]
fn every_verb_scribe_calls_is_one_the_role_names() {
    // The list and the call sites are two copies of one fact. This is the cheap half of holding
    // them together; `gate-role.sh` holds the other end against `ROLES.md`.
    let source = include_str!("../scribe.rs");
    for verb in ["observe", "amend", "replay", "sessions", "recall", "model"] {
        assert!(
            source.contains(&format!("call(\"{verb}\"")) || ROLE.contains(&verb),
            "`{verb}` is called but the role does not name it"
        );
    }
}

#[test]
fn the_role_names_no_verb_balthasar_alone_offers() {
    // `utility`, `context`, `trace` and `status` are balthasar's own. A role contract that
    // named them would make a second implementation owe verbs magi never calls.
    for theirs in ["utility", "context", "trace", "status", "ingest"] {
        assert!(
            !ROLE.contains(&theirs),
            "`{theirs}` is one implementation's, not the role's"
        );
    }
}

use super::*;

fn current() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("KEY".into(), "old:tail".into()),
        ("HOME".into(), "/home/test".into()),
    ])
}

#[test]
fn shell_values_expand_without_splitting_or_execution() {
    let (out, skipped) = exports(
        r#"export KEY=new_val:$KEY NEXT="${KEY}:end" LITERAL='$KEY' EMPTY= URL=https://host/a?x=1"#,
        &current(),
        true,
    )
    .expect("assignments");
    assert_eq!(out["KEY"], "new_val:old:tail");
    assert_eq!(out["NEXT"], "old:tail:end");
    assert_eq!(out["LITERAL"], "$KEY");
    assert_eq!(out["EMPTY"], "");
    assert_eq!(out["URL"], "https://host/a?x=1");
    assert_eq!(skipped, 0);
}

#[test]
fn source_only_reads_standalone_exports_not_control_flow_or_heredocs() {
    let script = "echo ignored\nplain=ignored\nexport KEY=first\n\
        f() { export HIDDEN=function; }\nif true; then export HIDDEN=conditional; fi\n\
        false && export HIDDEN=and\nexport HIDDEN=pipeline | cat\n\
        cat <<'EOF'\nexport HIDDEN=heredoc\nEOF\n\
        export DYNAMIC=$(touch /tmp/magi-must-not-execute)\n\
        export KEY=second:$KEY # comment\nexport FINAL=";
    let (out, skipped) = exports(script, &current(), false).expect("script");
    assert_eq!(out["KEY"], "second:first");
    assert_eq!(out["FINAL"], "");
    assert!(!out.contains_key("HIDDEN"));
    assert!(!out.contains_key("DYNAMIC"));
    assert_eq!(skipped, 1);
}

#[test]
fn env_rejects_unsafe_or_invalid_inputs_atomically_without_echoing_values() {
    for bad in [
        "export OK=sentinel; echo sentinel",
        "export OK=sentinel && true",
        "export OK=sentinel &",
        "export OK=sentinel >file",
        "export OK=sentinel BAD=$(echo sentinel)",
        "export OK=sentinel BAD=`echo sentinel`",
        "export OK=sentinel BAD=${KEY:-sentinel}",
        "export OK=sentinel BAD=(sentinel)",
        "export OK=sentinel BAD=$((1+2))",
        "export OK=sentinel BAD=<(echo sentinel)",
        "export OK=sentinel BAD='unterminated",
        "export -n OK=sentinel",
        "export OK=sentinel BAD-NAME=sentinel",
    ] {
        let error = exports(bad, &current(), true).expect_err("refuse shell execution");
        assert!(!error.contains("sentinel"), "{error}");
    }
}

#[test]
fn paths_and_escaping_follow_shell_quoting() {
    assert_eq!(
        path("~/some/file", &current()).expect("path"),
        "/home/test/some/file"
    );
    assert_eq!(
        path("\"$HOME/weird file\"", &current()).expect("path"),
        "/home/test/weird file"
    );
    assert!(path("a b", &current()).is_err());
    let (out, _) = exports(
        "export A=hello\\ world B=\"\\$KEY\\q\" C=$UNSET D=one\\\ntwo",
        &current(),
        true,
    )
    .expect("quotes");
    assert_eq!(out["A"], "hello world");
    assert_eq!(out["B"], "$KEY\\q");
    assert_eq!(out["C"], "");
    assert_eq!(out["D"], "onetwo");
}

#[test]
fn requests_redact_secrets_in_debug_output() {
    let command = magi_proto::UiCommand::Environment(magi_proto::Environment {
        source: false,
        input: "KEY=sentinel".into(),
    });
    assert!(!format!("{command:?}").contains("sentinel"));
}

#[test]
fn environment_expansion_is_bounded_before_accumulating_many_values() {
    let current = BTreeMap::from([("LARGE".into(), "x".repeat(LIMIT / 2))]);
    assert!(exports("export A=$LARGE B=$LARGE", &current, true).is_err());
    let (out, skipped) = exports(
        "export A=$LARGE\nexport B=$LARGE\nexport C=small",
        &current,
        false,
    )
    .expect("bounded source");
    assert_eq!(skipped, 1);
    assert!(!out.contains_key("B"));
    assert_eq!(out["C"], "small");
}

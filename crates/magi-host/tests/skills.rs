use magi_host::skills::{
    FILE_MOST, Failure, METADATA_MOST, invocation, metadata, parse, valid_name,
};

const SOURCE: &str = "---\nname: review\ndescription: Review changes\n---\nRead only.\n";

fn source(description: &str, body: &str) -> String {
    format!("---\nname: review\ndescription: {description}\n---\n{body}")
}

#[test]
fn exact_body_and_independent_source_digest() {
    let document = parse("review", SOURCE.as_bytes()).expect("workflow fixture");
    assert_eq!(document.name, "review");
    assert_eq!(document.description, "Review changes");
    assert_eq!(document.body, "Read only.\n");
    assert_eq!(
        document.source_digest,
        "8707b745571ed633664cb84525f173b2e82138dde583814eb8cbca02f26f60b5"
    );
}

#[test]
fn lazy_metadata_needs_no_body_and_cannot_accept_body_bytes() {
    let header = "---\nname: review\ndescription: Révision\n---\n";
    let summary = metadata("review", header.as_bytes()).expect("workflow fixture");
    assert_eq!(summary.name, "review");
    assert_eq!(summary.description, "Révision");
    assert_eq!(
        metadata("review", SOURCE.as_bytes()),
        Err(Failure::Metadata)
    );
    assert_eq!(
        metadata("review", &vec![b'x'; METADATA_MOST + 1]),
        Err(Failure::MetadataOversized)
    );
    assert_eq!(metadata("other", header.as_bytes()), Err(Failure::Name));
    assert_eq!(metadata("review", b"\xff"), Err(Failure::Encoding));
}

#[test]
fn metadata_and_full_parser_apply_identical_scalar_rules() {
    for value in [
        "text",
        "'literal: # value'",
        "!!tag",
        "&anchor",
        "|",
        "\"\\u0000\"",
        "",
    ] {
        let header = source(value, "");
        let full = source(value, "body");
        match (
            metadata("review", header.as_bytes()),
            parse("review", full.as_bytes()),
        ) {
            (Ok(summary), Ok(document)) => {
                assert_eq!(summary.name, document.name);
                assert_eq!(summary.description, document.description);
            }
            (Err(a), Err(b)) => assert_eq!(a, b),
            pair => panic!("metadata/full parser disagreed: {pair:?}"),
        }
    }
}

#[test]
fn identifier_boundaries_and_path_rejection() {
    for name in ["a", "0", "review-2", "x--y", &"a".repeat(64)] {
        assert!(valid_name(name), "{name:?}");
    }
    for name in [
        "",
        "-a",
        "a-",
        ".",
        "..",
        "../a",
        "a/b",
        "a\\b",
        "A",
        "é",
        "ａ",
        "a.b",
        "a_b",
        "a b",
        "a\0b",
        &"a".repeat(65),
    ] {
        assert!(!valid_name(name), "{name:?}");
    }
}

#[test]
fn directory_and_metadata_name_must_agree() {
    assert_eq!(parse("other", SOURCE.as_bytes()), Err(Failure::Name));
    assert_eq!(parse("../review", SOURCE.as_bytes()), Err(Failure::Name));
    for name in ["Review", "../review", "réview", ""] {
        let input = SOURCE.replace("name: review", &format!("name: {name}"));
        assert!(parse("review", input.as_bytes()).is_err());
    }
}

#[test]
fn unicode_description_and_literal_body() {
    let body = "\n  $(touch /tmp/not-run)\n`command` ${SECRET} {{template}}\n---\n";
    let document =
        parse("review", source("Révision 日本語 🦀", body).as_bytes()).expect("workflow fixture");
    assert_eq!(document.description, "Révision 日本語 🦀");
    assert_eq!(document.body, body);
}

#[test]
fn scalar_quotes_are_decoded_without_interpretation() {
    for (input, output) in [
        (r#""Quoted \"review\"""#, "Quoted \"review\""),
        ("'It''s literal'", "It's literal"),
        ("'!!tag &anchor *alias'", "!!tag &anchor *alias"),
        ("'a: b # comment'", "a: b # comment"),
        (r#""\u65e5\u672c""#, "日本"),
        ("$(echo SECRET)", "$(echo SECRET)"),
    ] {
        let document = parse("review", source(input, "body").as_bytes()).expect("workflow fixture");
        assert_eq!(document.description, output);
    }
}

#[test]
fn unsupported_yaml_constructs_fail_closed() {
    for value in [
        "!!python/object/apply:os.system [id]",
        "&anchor value",
        "*anchor",
        "[a, b]",
        "{a: b}",
        "|",
        ">",
        "%TAG",
        "@value",
        "`command`",
        "#comment",
        "? key",
        "- item",
        "a: b",
        "a # comment",
        "'unclosed",
        "'can't'",
        "\"unclosed",
        "\"text\" trailing",
        "\"line\\nnext\"",
        "\"\\u0000\"",
        "",
        "''",
        "\" \"",
    ] {
        assert_eq!(
            parse("review", source(value, "body").as_bytes()),
            Err(Failure::Metadata),
            "{value:?}"
        );
    }
}

#[test]
fn duplicate_unknown_nested_and_missing_keys_are_rejected() {
    for metadata in [
        "name: review\nname: review\ndescription: text",
        "name: review\ndescription: text\ndescription: text",
        "name: review\ndescription: text\nrun: command",
        "name: review",
        "description: text",
        "Name: review\ndescription: text",
        " name: review\ndescription: text",
        "name: review\n  description: text",
        "name: review\ndescription: text\n continuation",
        "name: review\ndescription text",
    ] {
        let input = format!("---\n{metadata}\n---\nbody");
        assert_eq!(parse("review", input.as_bytes()), Err(Failure::Metadata));
    }
}

#[test]
fn delimiters_are_explicit_not_found_inside_body() {
    for input in [
        "",
        "body",
        "\u{feff}---\nname: review\ndescription: text\n---\nbody",
        " ---\nname: review\ndescription: text\n---\nbody",
        "---\nname: review\ndescription: text",
        "---\n---\nbody",
        "---\nname: review\ndescription: text\n...\nbody",
    ] {
        assert_eq!(parse("review", input.as_bytes()), Err(Failure::Metadata));
    }
}

#[test]
fn comments_and_blank_metadata_lines_are_supported() {
    let input = "---\n# literal metadata\n\nname: review\ndescription: text\n---\nbody";
    assert_eq!(
        parse("review", input.as_bytes())
            .expect("workflow fixture")
            .body,
        "body"
    );
}

#[test]
fn crlf_body_bytes_are_preserved() {
    let input = SOURCE.replace('\n', "\r\n");
    let document = parse("review", input.as_bytes()).expect("workflow fixture");
    assert_eq!(document.body, "Read only.\r\n");
    assert_ne!(
        document.source_digest,
        parse("review", SOURCE.as_bytes())
            .expect("workflow fixture")
            .source_digest
    );
}

#[test]
fn invalid_utf8_is_not_lossily_replaced() {
    let mut input = SOURCE.as_bytes().to_vec();
    input.push(0xff);
    assert_eq!(parse("review", &input), Err(Failure::Encoding));
}

#[test]
fn empty_and_nul_body_are_rejected() {
    for body in ["", " \n\t\r\n", "text\0tail"] {
        assert_eq!(
            parse("review", source("text", body).as_bytes()),
            Err(Failure::Body)
        );
    }
}

#[test]
fn complete_file_limit_is_inclusive_and_byte_based() {
    let header = source("text", "");
    let mut input = header.clone();
    input.push_str(&"x".repeat(FILE_MOST - header.len()));
    assert_eq!(input.len(), FILE_MOST);
    assert!(parse("review", input.as_bytes()).is_ok());
    input.push('x');
    assert_eq!(parse("review", input.as_bytes()), Err(Failure::Oversized));
    let input = source("text", &"🦀".repeat(FILE_MOST / 4));
    assert_eq!(parse("review", input.as_bytes()), Err(Failure::Oversized));
}

#[test]
fn metadata_limit_includes_both_delimiters() {
    let overhead = source("", "").len();
    let input = source(&"x".repeat(METADATA_MOST - overhead), "body");
    assert_eq!(input.len(), METADATA_MOST + 4);
    assert!(parse("review", input.as_bytes()).is_ok());
    let input = source(&"x".repeat(METADATA_MOST - overhead + 1), "body");
    assert_eq!(
        parse("review", input.as_bytes()),
        Err(Failure::MetadataOversized)
    );
}

#[test]
fn changed_metadata_or_body_changes_digest() {
    let before = parse("review", SOURCE.as_bytes()).expect("workflow fixture");
    for changed in [
        SOURCE.replace("changes", "code"),
        SOURCE.replace("Read", "Inspect"),
    ] {
        let after = parse("review", changed.as_bytes()).expect("workflow fixture");
        assert_ne!(before.source_digest, after.source_digest);
    }
    assert_eq!(before.body, "Read only.\n");
}

#[test]
fn only_explicit_skill_command_is_interpreted() {
    for text in [
        "",
        "/review",
        "/skills review",
        "/skillish",
        " /skill review",
        "say /skill review",
        "/skill/review",
    ] {
        assert_eq!(invocation(text), Ok(None), "{text:?}");
    }
    let selected = invocation("/skill review")
        .expect("workflow fixture")
        .expect("workflow fixture");
    assert_eq!(selected.name, "review");
    assert_eq!(selected.arguments, "");
}

#[test]
fn malformed_explicit_invocation_is_visible() {
    for text in [
        "/skill",
        "/skill ",
        "/skill ../review",
        "/skill Review",
        "/skill réview",
    ] {
        assert_eq!(invocation(text), Err(Failure::Invocation), "{text:?}");
    }
}

#[test]
fn arguments_preserve_shell_punctuation_spaces_and_newlines() {
    let arguments = "  $(touch /tmp/not-run); $SECRET 'quoted' \"double\"\nnext\n";
    let command = format!("/skill review {arguments}");
    let selected = invocation(&command)
        .expect("workflow fixture")
        .expect("workflow fixture");
    assert_eq!(selected.name, "review");
    assert_eq!(selected.arguments, arguments);
    let selected = invocation("/skill\treview\t日本語")
        .expect("workflow fixture")
        .expect("workflow fixture");
    assert_eq!(selected.arguments, "日本語");
}

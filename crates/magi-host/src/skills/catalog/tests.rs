use super::*;
use crate::skills::{Failure, ReadFailure};

fn roots() -> Roots {
    Roots::new(
        Path::new("/workspace/nested"),
        None,
        Path::new("/xdg/config"),
    )
    .expect("roots")
}

fn listing(names: &[&str]) -> wire::Listing {
    wire::Listing {
        entries: names
            .iter()
            .map(|name| wire::Candidate {
                name: (*name).into(),
                metadata: Ok(format!(
                    "---\nname: {name}\ndescription: description\n---\n"
                )),
            })
            .collect(),
        invalid_names: 0,
    }
}

fn empty() -> Catalog {
    Catalog {
        entries: BTreeMap::new(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn roots_use_launch_or_explicit_workspace_never_ancestor_search() {
    let roots = roots();
    assert_eq!(
        roots
            .sources()
            .iter()
            .map(|s| s.root.clone())
            .collect::<Vec<_>>(),
        [
            "/workspace/nested/.magi/skills",
            "/workspace/nested/.agents/skills",
            "/xdg/config/magi/skills"
        ]
        .map(PathBuf::from)
    );
    for (explicit, expected) in [
        ("/other/worktree", "/other/worktree/.magi/skills"),
        ("subdir", "/workspace/subdir/.magi/skills"),
    ] {
        let selected = Roots::new(
            Path::new("/workspace"),
            Some(Path::new(explicit)),
            Path::new("/xdg"),
        )
        .expect("explicit");
        assert_eq!(
            selected.sources().first().expect("project").root,
            PathBuf::from(expected)
        );
    }
    assert!(
        Roots::new(
            Path::new("/work"),
            Some(Path::new("../parent")),
            Path::new("/xdg")
        )
        .is_err()
    );
    assert!(Roots::new(Path::new("relative"), None, Path::new("/xdg")).is_err());
    assert!(Roots::new(Path::new("/work"), None, Path::new("relative")).is_err());
}

#[test]
fn first_match_wins_with_sorted_entries_and_duplicate_diagnostics() {
    let roots = roots();
    let mut catalog = empty();
    for (source, names) in roots.sources().iter().zip([
        vec!["review", "z"],
        vec!["a", "review"],
        vec!["a", "review", "user"],
    ]) {
        catalog.merge(source, listing(&names));
    }
    assert_eq!(
        catalog
            .entries()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "review", "user", "z"]
    );
    assert_eq!(
        catalog.entries.get("review").expect("review").source.class,
        SourceClass::Project
    );
    assert_eq!(
        catalog.entries.get("a").expect("a").source.class,
        SourceClass::Agents
    );
    assert_eq!(
        catalog.entries.get("user").expect("user").source.class,
        SourceClass::User
    );
    assert_eq!(catalog.diagnostics.len(), 3);
    assert!(
        catalog
            .diagnostics
            .iter()
            .all(|item| matches!(item, Diagnostic::Duplicate { .. }))
    );
}

#[test]
fn malformed_first_match_does_not_silently_fall_back() {
    let roots = roots();
    for metadata in [Ok("not metadata".into()), Err(ReadFailure::UnsafePath)] {
        let mut catalog = empty();
        let mut first = listing(&["review"]);
        first.entries.first_mut().expect("entry").metadata = metadata;
        for (index, source) in roots.sources().iter().enumerate() {
            catalog.merge(
                source,
                if index == 0 {
                    wire::Listing {
                        entries: std::mem::take(&mut first.entries),
                        invalid_names: 2,
                    }
                } else {
                    listing(&["review"])
                },
            );
        }
        assert!(
            catalog
                .entries
                .get("review")
                .expect("entry")
                .metadata
                .is_err()
        );
        assert_eq!(
            catalog.entries.get("review").expect("entry").source.class,
            SourceClass::Project
        );
        assert_eq!(catalog.diagnostics.len(), 3);
    }
}

#[test]
fn complete_metadata_parser_applies_to_each_candidate() {
    let mut catalog = empty();
    let mut list = listing(&["review"]);
    list.entries.first_mut().expect("candidate").metadata =
        Ok("---\nname: other\ndescription: body\n---\n".into());
    catalog.merge(roots().sources().first().expect("project"), list);
    assert_eq!(
        catalog.entries.get("review").expect("entry").metadata,
        Err(Error::Document(Failure::Name))
    );
}

#[test]
fn all_three_full_roots_remain_bounded() {
    let mut catalog = empty();
    for (index, source) in roots().sources().iter().enumerate() {
        let names: Vec<_> = (0..256).map(|n| format!("r{index}-{n:03}")).collect();
        catalog.merge(
            source,
            listing(&names.iter().map(String::as_str).collect::<Vec<_>>()),
        );
    }
    assert_eq!(catalog.entries().count(), 768);
    assert!(catalog.diagnostics().is_empty());
}

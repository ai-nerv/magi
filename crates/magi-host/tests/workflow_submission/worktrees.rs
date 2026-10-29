use super::*;

fn git(dir: &Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new(utility("git"))
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(arguments)
        .env_clear()
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .current_dir(dir)
        .output()
        .expect("private git command");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git output")
}

async fn linked(explicit: bool) {
    let name = if explicit { "git-root" } else { "git-cwd" };
    let Some(fixture) = Fixture::configured(name, false, |catalog| {
        let dir = &catalog.cwd;
        git(dir, &["init", "--bare", "--template=", "repository"]);
        git(
            dir,
            &[
                "--git-dir=repository",
                "worktree",
                "add",
                "--orphan",
                "-b",
                "fixture",
                "tree",
            ],
        );
        let tree = dir.join("tree");
        assert!(tree.join(".git").is_file());
        let registered = git(
            dir,
            &["--git-dir=repository", "worktree", "list", "--porcelain"],
        );
        assert!(registered.contains(&format!("worktree {}", tree.display())));
        assert!(registered.contains("branch refs/heads/fixture"));
        assert!(git(dir, &["--git-dir=repository", "rev-list", "--all"]).is_empty());
        std::fs::create_dir_all(tree.join("nested")).expect("nested cwd");
        assert_eq!(
            git(dir, &["-C", "tree/nested", "rev-parse", "--show-toplevel"]).trim(),
            tree.to_str().expect("tree")
        );
        catalog.workflow_workspace = explicit.then_some(tree.clone());
        catalog.cwd = tree.join("nested");
    })
    .await
    else {
        return;
    };
    let source = |name: &str| {
        format!("---\nname: {name}\ndescription: Worktree fixture\n---\n{name} body\n")
    };
    for (root, name) in [
        ("tree/.magi/skills", "worktree"),
        ("tree/nested/.magi/skills", "nested"),
        ("repository/.magi/skills", "gitdata"),
        (".magi/skills", "ancestor"),
        ("config/magi/skills", "personal"),
    ] {
        put(
            &fixture.dir.join(root).join(name).join("SKILL.md"),
            &source(name),
        );
    }
    let selected = if explicit { "worktree" } else { "nested" };
    let expected_root = fixture.dir.join(if explicit {
        "tree/.magi/skills"
    } else {
        "tree/nested/.magi/skills"
    });
    let (mut reader, mut writer, _) = fixture.attach().await;
    writer
        .write(&UiCommand::RefreshWorkflows)
        .await
        .expect("refresh");
    let listed = event(&mut reader, |event| {
        matches!(event, HarnessEvent::WorkflowsListed { .. })
    })
    .await;
    let HarnessEvent::WorkflowsListed {
        entries,
        warnings,
        error,
    } = listed
    else {
        unreachable!()
    };
    assert!(error.is_none(), "{error:?}");
    assert!(warnings.is_empty(), "{warnings:?}");
    let mut names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
    names.sort_unstable();
    let mut expected = ["personal", selected];
    expected.sort_unstable();
    assert_eq!(names, expected);
    assert!(entries.iter().all(|entry| entry.error.is_none()));
    assert!(fixture.replay().await.is_empty());
    prompt(&mut writer, &format!("/skill {selected} literal")).await;
    let admitted = event(&mut reader, |event| {
        matches!(
            event,
            HarnessEvent::WorkflowMessage { .. } | HarnessEvent::Refused { .. }
        )
    })
    .await;
    let HarnessEvent::WorkflowMessage { workflow, .. } = admitted else {
        panic!("unexpected admission: {admitted:?}");
    };
    assert_eq!(workflow.root, expected_root.to_str().expect("root"));
    assert_eq!(workflow.body, format!("{selected} body\n"));
    assert_eq!(workflow.arguments, "literal");
    assert_eq!(
        workflow.digest,
        format!("{:x}", Sha256::digest(source(selected)))
    );
    event(&mut reader, |event| {
        matches!(
            event,
            HarnessEvent::StatusChanged {
                status: AgentStatus::Idle,
                ..
            }
        )
    })
    .await;
    std::fs::remove_dir_all(&expected_root).expect("remove selected source");
    let (_, _, snapshot) = fixture.attach().await;
    assert_eq!(fixture.replay().await, snapshot);
    assert_eq!(
        snapshot
            .iter()
            .filter(|entry| matches!(entry, Entry::Workflow { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn explicit_root_selects_real_linked_worktree_without_scanning_git_metadata() {
    linked(true).await;
}

#[tokio::test]
async fn nested_launch_in_real_worktree_does_not_implicitly_select_ancestor_skills() {
    linked(false).await;
}

use super::*;

fn events(root: &Path) -> Vec<Value> {
    serde_json::from_slice(
        &std::fs::read(root.join("dispatch-counts.json"))
            .expect("actual bounded dispatch counters"),
    )
    .expect("fixture counters")
}

fn mutations(root: &Path) -> usize {
    events(root)
        .iter()
        .filter(|event| event["mutation_id"].is_string())
        .count()
}

fn dropped(root: &Path, outcome: &str) {
    let events = events(root);
    let event = events.last().expect("actual completed mutation");
    assert_eq!(event["dropped"], true, "{event}");
    assert_eq!(event["outcome"], outcome, "{event}");
    let id = event["mutation_id"]
        .as_str()
        .expect("actual logical identity");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["mutation_id"] == id)
            .count(),
        1,
        "logical mutation replayed"
    );
    assert!(!root.join("drop-next").exists());
}

fn arm(root: &Path) {
    assert!(!root.join("drop-next").exists());
    std::fs::write(
        root.join("drop-next"),
        "drop only after actual completed reply",
    )
    .expect("loss barrier");
}

fn unknown(result: &magi_proto::ToolResult) {
    assert!(
        result.is_error
            && result
                .output
                .starts_with("Native file mutation outcome is unknown"),
        "{}",
        result.output
    );
}

async fn blocked(
    root: &Path,
    parent: &Mutex<Session>,
    registry: &Registry,
    ops: &dyn magi_tools::Ops,
    path: &str,
) {
    let before = mutations(root);
    let result = call(
        parent,
        registry,
        ops,
        &root.join("work"),
        "store",
        json!({"path":path,"contents":"unsafe replay\n"}),
    )
    .await;
    assert!(
        result.is_error && result.output.contains("reread"),
        "{}",
        result.output
    );
    assert_eq!(
        mutations(root),
        before,
        "blocked mutation reached actual tool process"
    );
}

pub(super) async fn verify(
    root: &Path,
    parent: &Mutex<Session>,
    sibling: &Mutex<Session>,
    registry: &Registry,
    ops: &dyn magi_tools::Ops,
) {
    let cwd = root.join("work");
    arm(root);
    let result = call(
        parent,
        registry,
        ops,
        &cwd,
        "store",
        json!({"path":"lost-create","contents":"A\n"}),
    )
    .await;
    unknown(&result);
    assert_eq!(
        std::fs::read_to_string(cwd.join("lost-create")).expect("committed creation"),
        "A\n"
    );
    dropped(root, "committed");
    blocked(root, parent, registry, ops, "lost-create").await;
    let sibling_read = call(
        sibling,
        registry,
        ops,
        &cwd,
        "look",
        json!({"path":"lost-create"}),
    )
    .await;
    assert!(!sibling_read.is_error, "{}", sibling_read.output);
    blocked(root, parent, registry, ops, "lost-create").await;
    let missing = call(
        parent,
        registry,
        ops,
        &cwd,
        "look",
        json!({"path":"not-the-pinned-target"}),
    )
    .await;
    assert!(missing.is_error, "ordinary missing read succeeded");
    blocked(root, parent, registry, ops, "lost-create").await;
    let own = call(
        parent,
        registry,
        ops,
        &cwd,
        "look",
        json!({"path":"lost-create"}),
    )
    .await;
    assert!(
        !own.is_error && own.output.starts_with('A'),
        "{}",
        own.output
    );
    let replace = call(
        parent,
        registry,
        ops,
        &cwd,
        "store",
        json!({"path":"lost-create","contents":"B\n"}),
    )
    .await;
    assert!(!replace.is_error, "{}", replace.output);
    assert_eq!(
        std::fs::read_to_string(cwd.join("lost-create")).expect("observed replacement"),
        "B\n"
    );

    arm(root);
    let edit = call(
        parent,
        registry,
        ops,
        &cwd,
        "change",
        json!({"path":"lost-create","old":"B","new":"BB"}),
    )
    .await;
    unknown(&edit);
    assert_eq!(
        std::fs::read_to_string(cwd.join("lost-create")).expect("one non-idempotent edit"),
        "BB\n"
    );
    dropped(root, "committed");
    blocked(root, parent, registry, ops, "lost-create").await;
    let own = call(
        parent,
        registry,
        ops,
        &cwd,
        "look",
        json!({"path":"lost-create"}),
    )
    .await;
    assert!(!own.is_error, "{}", own.output);
    let edit = call(
        parent,
        registry,
        ops,
        &cwd,
        "change",
        json!({"path":"lost-create","old":"BB","new":"RECOVERED"}),
    )
    .await;
    assert!(!edit.is_error, "{}", edit.output);
    assert_eq!(
        std::fs::read_to_string(cwd.join("lost-create")).expect("recovered actual edit"),
        "RECOVERED\n"
    );

    for (path, interloper) in [("lost-absent", false), ("lost-absent-race", true)] {
        arm(root);
        let lost = call(
            parent,
            registry,
            ops,
            &cwd,
            "store",
            json!({"path":path,"contents":"invalid\u{0}"}),
        )
        .await;
        unknown(&lost);
        assert!(!cwd.join(path).exists());
        dropped(root, "refused");
        blocked(root, parent, registry, ops, path).await;
        let read = call(parent, registry, ops, &cwd, "look", json!({"path":path})).await;
        assert!(
            !read.is_error && read.output.contains("is absent"),
            "{}",
            read.output
        );
        let before = mutations(root);
        let edit = call(
            parent,
            registry,
            ops,
            &cwd,
            "change",
            json!({"path":path,"old":"x","new":"y"}),
        )
        .await;
        assert!(
            edit.is_error && edit.output.contains("delivered observation"),
            "absence authorized overwrite"
        );
        assert_eq!(mutations(root), before);
        if interloper {
            std::fs::write(cwd.join(path), "external creator\n").expect("competing creator");
        }
        let create = call(
            parent,
            registry,
            ops,
            &cwd,
            "store",
            json!({"path":path,"contents":"created once\n"}),
        )
        .await;
        assert_eq!(create.is_error, interloper, "{}", create.output);
        assert_eq!(
            std::fs::read_to_string(cwd.join(path)).expect("final creation bytes"),
            if interloper {
                "external creator\n"
            } else {
                "created once\n"
            }
        );
    }
    let events = events(root);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["dropped"] == true)
            .count(),
        4
    );
    let mut ids = std::collections::BTreeSet::new();
    for event in events
        .iter()
        .filter(|event| event["mutation_id"].is_string())
    {
        assert!(
            ids.insert(event["mutation_id"].as_str().expect("mutation id")),
            "duplicate actual mutation: {event}"
        );
        let keys: std::collections::BTreeSet<_> = event
            .as_object()
            .expect("event")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from(["mutation_id", "outcome", "dropped"])
        );
    }
    let doors = std::fs::read_to_string(cwd.join("doors")).expect("actual invocation doors");
    if std::env::var("MAGI_TOOLS_DOOR").as_deref() == Ok("library") {
        assert_eq!(
            doors.lines().filter(|line| *line == "serve").count(),
            1,
            "{doors}"
        );
        assert!(
            !doors.lines().any(|line| line == "run"),
            "sent socket mutation fell back to command: {doors}"
        );
        let actual: Value = serde_json::from_slice(
            &std::fs::read(root.join("actual-announcement.json")).expect("actual daemon admission"),
        )
        .expect("typed admission");
        assert_eq!(actual["event"], "listening");
        assert_eq!(actual["file_admission"], 1);
    } else {
        assert_eq!(
            doors.lines().filter(|line| *line == "run").count(),
            events.len(),
            "command dispatched outside actual event accounting"
        );
        assert!(!doors.lines().any(|line| line == "serve"), "{doors}");
    }
}

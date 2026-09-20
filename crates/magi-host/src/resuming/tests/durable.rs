use super::*;
use magi_testkit::{Mind, memory::Serving};

#[tokio::test]
async fn late_helper_is_durable_only_in_a_before_b_is_published() {
    let dir = Scratch::new("magi-resume", "late-helper");
    let Some(memory) = Serving::start(&dir, "late-helper").await else {
        return;
    };
    let mut family = Family::dial(memory.socket()).await.expect("dial");
    for id in ["A", "B"] {
        let raw = magi_journal::Record::Entry {
            cursor: Cursor(1),
            entry: user(id),
        };
        family.call("observe", vec![json!(id), json!({"cursor":1,"run":id,"raw":raw,"role":"user","kind":"user","text":format!("Only {id} uses Rust.")})]).await.expect("seed");
    }
    let jobs = family
        .call(
            "jobs",
            vec![json!("A"), json!({"helpers":["notes","curate"]})],
        )
        .await
        .expect("jobs");
    let job: crate::helping::Job = serde_json::from_value(
        jobs.into_iter()
            .find(|job| job["kind"] == "extract")
            .expect("extraction job"),
    )
    .expect("job");
    let job_id = job.id.clone();
    let scribe = Arc::new(Mutex::new(Some(scribe::Scribe::over(
        family,
        Some(memory.socket().into()),
        &SessionId::new("A"),
    ))));
    let session = Arc::new(Mutex::new(Session::recorded(
        SessionId::new("A"),
        vec![user("A")],
    )));
    let mind = Mind::answering("resume-late-helper", &json!({"ops":[{"op":"add","title":"Only A","text":"Only A uses Rust.","description":"A language","pinned":false}]}).to_string());
    let mut backend = crate::turn::Backend {
        tools: Vec::new(),
        clients: Vec::new(),
        tooling: Default::default(),
        cwd: dir.to_path_buf(),
        model: "fake/one".into(),
        mind: mind.program().display().to_string(),
        wants: Default::default(),
        context_window: Some(200_000),
        max_output: None,
        system: None,
        confine: false,
        isolate: false,
        grants: Vec::new(),
        environ: Default::default(),
        helpers: Default::default(),
        deciders: Vec::new(),
    };
    backend
        .helpers
        .roles
        .insert("memory".into(), "fake/one".into());
    let old = session.lock().await.helpers();
    let events = session.lock().await.publisher();
    let (release, wait) = tokio::sync::oneshot::channel();
    let open = Arc::clone(&scribe);
    old.spawn(async move {
        wait.await.map_err(|why| why.to_string())?;
        crate::helping::work(&[job], &backend, &open, &events, &Default::default()).await
    })
    .expect("helper");
    let (switching, open) = (Arc::clone(&session), Arc::clone(&scribe));
    let switched =
        tokio::spawn(async move { resume(&switching, &RwLock::new(None), &open, "B").await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !session.lock().await.busy() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("transition");
    assert!(!switched.is_finished());
    assert_eq!(session.lock().await.id().as_str(), "A");
    release.send(()).expect("release helper");
    tokio::time::timeout(std::time::Duration::from_secs(20), switched)
        .await
        .expect("bounded transition")
        .expect("task")
        .expect("resume");
    assert_eq!(session.lock().await.id().as_str(), "B");
    assert_eq!(mind.asks().len(), 1);
    assert!(old.spawn(async { Ok(()) }).is_err());
    drop(scribe);
    drop(memory);
    let memory = Serving::start(&dir, "late-helper").await.expect("reopen");
    let mut family = Family::dial(memory.socket())
        .await
        .expect("independent dial");
    let a = family
        .call("notes", vec![json!("A")])
        .await
        .expect("A notes");
    assert_eq!(
        a[0]["deferred"].as_array().expect("A deferred").len(),
        1,
        "{a:?}"
    );
    assert_eq!(a[0]["deferred"][0]["title"], "Only A");
    let a_jobs = family
        .call("jobs", vec![json!("A"), json!({"inspect":true})])
        .await
        .expect("A jobs");
    let b_jobs = family
        .call("jobs", vec![json!("B"), json!({"inspect":true})])
        .await
        .expect("B jobs");
    let completed = a_jobs[0]["jobs"]
        .as_array()
        .expect("A job list")
        .iter()
        .find(|job| job["id"] == job_id)
        .expect("A owns helper");
    assert_eq!(completed["state"], "done");
    assert!(b_jobs[0]["jobs"].as_array().expect("B job list").is_empty());
    let changes = family
        .call("changes", vec![json!("A"), json!({})])
        .await
        .expect("changes");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["by"], job_id);
}

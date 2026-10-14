//! Host probe transport and unavailable-host behavior.

use super::*;
use magi_model::scratch::Scratch;
use magi_proto::{AgentStatus, ModelInfo};

#[tokio::test]
async fn probe_reads_details_without_attaching() {
    let dir = Scratch::new("magi-cli", "agent-probe");
    let socket = dir.join("host");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let (read, write) = stream.into_split();
        assert_eq!(
            FrameReader::new(read)
                .read::<UiCommand>()
                .await
                .expect("command"),
            UiCommand::Inspect
        );
        let details = AgentDetails {
            model: Some(ModelInfo {
                name: "provider/active-model".into(),
                context_window: 128000,
            }),
            thinking: "high".into(),
            status: AgentStatus::Idle,
            approvals: 1,
            questions: 0,
            context: None,
            cost_micros: 0,
        };
        FrameWriter::new(write)
            .write(&details)
            .await
            .expect("details");
    });
    let details = probe(&socket).await.expect("sample");
    assert_eq!(details.model.expect("model").name, "provider/active-model");
    assert_eq!(details.approvals, 1);
    task.await.expect("server");
}

#[tokio::test]
async fn absent_or_older_hosts_are_unavailable() {
    let dir = Scratch::new("magi-cli", "older-host");
    assert!(probe(&dir.join("absent")).await.is_none());
    let socket = dir.join("host");
    let listener = magi_ipc::bind(&socket).await.expect("bind");
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        let (read, _) = stream.into_split();
        let _: UiCommand = FrameReader::new(read).read().await.expect("command");
    });
    assert!(probe(&socket).await.is_none());
    task.await.expect("server");
}

#[tokio::test]
async fn closed_tree_does_not_start_a_probe() {
    let mut app = App::new();
    app.reachable.push(crate::melchior::Peer {
        id: "agent".into(),
        ui: Some("/missing".into()),
        ..Default::default()
    });
    let mut poller = Poller::new();
    poller.pump(&mut app);
    assert!(poller.task.is_none());
    assert!(app.agent_details.is_empty());
}

#[tokio::test]
async fn stale_socket_results_are_rejected_and_roster_refresh_keeps_current_samples() {
    let mut app = App::new();
    app.reachable.push(crate::melchior::Peer {
        id: "agent".into(),
        ui: Some("/new".into()),
        ..Default::default()
    });
    app.show_agents();
    let mut poller = Poller::new();
    poller.next = Instant::now() + Duration::from_secs(60);
    poller
        .send
        .send(vec![("agent".into(), "/old".into(), None)])
        .await
        .expect("batch");
    poller.pump(&mut app);
    assert!(app.agent_details.is_empty());
    poller
        .send
        .send(vec![("agent".into(), "/new".into(), None)])
        .await
        .expect("batch");
    poller.pump(&mut app);
    assert_eq!(app.agent_details.len(), 1);
    app.reachable = app.reachable.clone();
    poller.pump(&mut app);
    assert_eq!(app.agent_details.len(), 1);
    app.reachable.clear();
    poller.pump(&mut app);
    assert!(app.agent_details.is_empty());
}

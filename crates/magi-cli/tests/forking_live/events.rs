use super::*;
use serde_json::Value;
use std::{
    fs,
    io::Read,
    os::unix::{fs::FileTypeExt, net::UnixStream},
};

fn inspect(mut stream: UnixStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read deadline");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("write deadline");
    magi_ipc::blocking::FrameWriter::new(&mut stream)
        .write_blocking(&UiCommand::Inspect)
        .expect("inspect");
    magi_ipc::blocking::FrameReader::new(stream.take(16388))
        .read_blocking()
        .expect("details")
}

fn socket_for(root: &Path, pid: u32) -> Option<PathBuf> {
    let mut pending = vec![root.to_owned()];
    for _ in 0..256 {
        let path = pending.pop()?;
        let metadata = fs::symlink_metadata(&path).ok()?;
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(path)
                    .ok()?
                    .filter_map(Result::ok)
                    .map(|e| e.path()),
            );
        } else if metadata.file_type().is_socket()
            && let Ok(stream) = UnixStream::connect(&path)
            && rustix::net::sockopt::get_socket_peercred(&stream)
                .is_ok_and(|peer| peer.pid.as_raw_nonzero().get() as u32 == pid)
        {
            return Some(path);
        }
    }
    None
}

fn instance(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

#[test]
fn actual_spawn_keeps_root_collection_after_fork_helper_exits() {
    run_tree(false);
}

#[test]
fn actual_nested_spawn_uses_original_root_and_immediate_parent_lineage() {
    run_tree(true);
}

fn run_tree(nested: bool) {
    let _alone = alone();
    let siblings: Option<Vec<_>> = [
        "MELCHIOR_TEST_BINARY",
        "BALTHASAR_TEST_BINARY",
        "CASPER_TEST_BINARY",
    ]
    .into_iter()
    .map(|name| std::env::var_os(name).map(PathBuf::from))
    .collect();
    let Some(siblings) = siblings else {
        magi_testkit::live::unavailable(
            "selected native siblings are required for child collection",
        );
        return;
    };
    let binary = fs::canonicalize(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")))
        .expect("selected Magi");
    let dir = workspace("eventkin", &siblings[0]);
    let init = dir.join("c/magi/init.lua");
    let mut config = fs::read_to_string(&init).expect("config");
    config.push_str(&format!(
        "\nmagi.events = true\nmagi.memory = {:?}\nmagi.tools = {:?}\n",
        siblings[1].to_str().expect("memory"),
        siblings[2].to_str().expect("tools")
    ));
    if nested {
        config.push_str("\nmagi.agents.reviewer.delegate = true\n");
    }
    fs::write(init, config).expect("trusted local configuration");
    let mut parent = Session::start(&dir, "root", FORK);
    assert_eq!(
        fs::read_link(format!("/proc/{}/exe", parent.process.id())).expect("root image"),
        binary
    );
    let mut screen = parent.screen();
    let child = screen.forked();
    until("the spawned native process", || {
        forked_by(parent.process.id()).is_some()
    });
    let pid = forked_by(parent.process.id()).expect("child pid");
    let identity = instance(pid).expect("child instance");
    assert_eq!(
        fs::read_link(format!("/proc/{pid}/exe")).expect("child image"),
        binary
    );
    until("the child's own host socket", || {
        socket_for(&dir.join("r"), pid).is_some()
    });
    let socket = socket_for(&dir.join("r"), pid).expect("child socket");
    let root = inspect(UnixStream::connect(&parent.socket).expect("root socket"));
    let descendant = inspect(UnixStream::connect(&socket).expect("child socket"));
    assert_eq!(root["events"]["state"], "collecting");
    assert_eq!(descendant["events"]["state"], "forwarding");
    let root_lineage = &root["events"]["lineage"];
    let lineage = &descendant["events"]["lineage"];
    let ancestor = &descendant["events"]["parent"];
    assert!(lineage["run_id"].as_str().is_some_and(|id| id.len() == 32));
    assert_eq!(lineage["run_id"], root_lineage["run_id"]);
    assert_ne!(lineage["agent_id"], root_lineage["agent_id"]);
    assert_eq!(ancestor["agent_id"], root_lineage["agent_id"]);
    assert_eq!(ancestor["run_id"], root_lineage["run_id"]);
    assert!(
        ancestor["turn_id"]
            .as_str()
            .is_some_and(|id| id.len() == 32)
    );
    assert!(lineage["turn_id"].is_null() && lineage["ask_id"].is_null());
    until("root collection of the child and its native memory", || {
        let state = inspect(UnixStream::connect(&parent.socket).expect("root socket"));
        assert_eq!(state["events"]["summary"]["counts"]["invalid"], 0);
        state["events"]["summary"]["remote_producers"]
            .as_u64()
            .is_some_and(|n| n >= 3)
    });
    if nested {
        exercise_nested(
            &dir,
            &parent.socket,
            &socket,
            pid,
            &binary,
            lineage,
            ancestor,
        );
    }
    let before = inspect(UnixStream::connect(&parent.socket).expect("root socket"))["events"]["summary"]["retired"]["producers"].as_u64().expect("retirements");
    let stopped = screen.stop(&dir, &child);
    assert!(!stopped.is_error, "{}", stopped.output);
    until("exact native child exit", || {
        instance(pid).as_ref() != Some(&identity)
    });
    until("root retirement after actual child exit", || {
        let state = inspect(UnixStream::connect(&parent.socket).expect("root socket"));
        assert_eq!(state["events"]["summary"]["counts"]["invalid"], 0);
        state["events"]["summary"]["retired"]["producers"]
            .as_u64()
            .is_some_and(|n| n >= before + 2)
    });
    parent.end();
}

fn exercise_nested(
    dir: &Path,
    root: &Path,
    child: &Path,
    parent_pid: u32,
    binary: &Path,
    parent_lineage: &Value,
    first_parent: &Value,
) {
    let mut screen = Screen::open(child);
    screen.submit(FORK);
    let grandchild = screen.forked();
    until("the nested native process", || {
        forked_by(parent_pid).is_some()
    });
    let pid = forked_by(parent_pid).expect("grandchild pid");
    let identity = instance(pid).expect("grandchild instance");
    assert_eq!(
        fs::read_link(format!("/proc/{pid}/exe")).expect("grandchild image"),
        binary
    );
    until("the nested host socket", || {
        socket_for(&dir.join("r"), pid).is_some()
    });
    let socket = socket_for(&dir.join("r"), pid).expect("grandchild socket");
    let state = inspect(UnixStream::connect(&socket).expect("grandchild inspect"));
    assert_eq!(state["events"]["state"], "forwarding");
    let lineage = &state["events"]["lineage"];
    let parent = &state["events"]["parent"];
    assert_eq!(lineage["run_id"], parent_lineage["run_id"]);
    assert_ne!(lineage["agent_id"], parent_lineage["agent_id"]);
    assert_ne!(lineage["agent_id"], first_parent["agent_id"]);
    assert_eq!(parent["agent_id"], parent_lineage["agent_id"]);
    assert_eq!(parent["run_id"], parent_lineage["run_id"]);
    assert!(parent["turn_id"].as_str().is_some_and(|id| id.len() == 32));
    assert_ne!(parent["turn_id"], first_parent["turn_id"]);
    assert!(lineage["turn_id"].is_null() && lineage["ask_id"].is_null());
    until(
        "original root collection of nested agent and memory",
        || {
            let state = inspect(UnixStream::connect(root).expect("root inspect"));
            assert_eq!(state["events"]["summary"]["counts"]["invalid"], 0);
            assert_eq!(state["events"]["summary"]["counts"]["refused_producers"], 0);
            state["events"]["summary"]["remote_producers"]
                .as_u64()
                .is_some_and(|n| n >= 5)
        },
    );
    let before = inspect(UnixStream::connect(root).expect("root inspect"))["events"]["summary"]["retired"]["producers"].as_u64().expect("retirements");
    let stopped = screen.stop(dir, &grandchild);
    assert!(!stopped.is_error, "{}", stopped.output);
    until("exact nested native exit", || {
        instance(pid).as_ref() != Some(&identity)
    });
    until("root retirement of nested agent and memory", || {
        let state = inspect(UnixStream::connect(root).expect("root inspect"));
        assert_eq!(state["events"]["summary"]["counts"]["invalid"], 0);
        state["events"]["summary"]["retired"]["producers"]
            .as_u64()
            .is_some_and(|n| n >= before + 2)
    });
}

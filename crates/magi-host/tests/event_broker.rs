#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{broker, events::collection::runtime};
use magi_model::{Context, Delta};
use magi_proto::ask::Wants;
use magi_testkit::Scratch;
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("native broker deadline");
}

fn child(binary: &Path) -> Option<u32> {
    fs::read_dir("/proc")
        .expect("processes")
        .filter_map(Result::ok)
        .find_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let status = fs::read_to_string(entry.path().join("status")).ok()?;
            let parent = status
                .lines()
                .find_map(|line| line.strip_prefix("PPid:\t"))?;
            (parent.trim().parse::<u32>().ok()? == std::process::id()
                && fs::read_link(entry.path().join("exe")).ok()?.as_path() == binary)
                .then_some(pid)
        })
}

async fn scenario(binary: &Path, enabled: bool, cancel: bool, legacy: bool) {
    let root = Scratch::new(
        "event-broker",
        if cancel {
            "cancel"
        } else if enabled {
            "enabled"
        } else {
            "disabled"
        },
    );
    for name in [
        "home",
        "config/melchior",
        "runtime",
        "tmp",
        "state",
        "data",
        "cache",
    ] {
        fs::create_dir_all(root.join(name)).expect("directory");
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o700))
            .expect("private directory");
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("synthetic provider");
    fs::write(root.join("config/melchior/providers.lua"), format!(r#"melchior.provider("synthetic", {{ name="Synthetic", api="openai-completions", base_url="http://{}", auth={{kind="none"}}, models={{{{id="test",name="Synthetic",context_window=8192,max_tokens=128}}}} }})"#, listener.local_addr().expect("provider address"))).expect("catalog");
    let environ: BTreeMap<String, String> = [
        ("HOME", "home"),
        ("MELCHIOR_CONFIG", "config/melchior"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_CACHE_HOME", "cache"),
        ("TMPDIR", "tmp"),
    ]
    .into_iter()
    .map(|(key, path)| (key.into(), root.join(path).to_string_lossy().into_owned()))
    .collect();
    let program = if legacy {
        let wrapper = root.join("legacy");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\n[ \"$1\" = event-capabilities ] && exit 1\nexec '{}' \"$@\"\n",
                binary.display()
            ),
        )
        .expect("legacy wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700))
            .expect("legacy executable");
        wrapper.to_string_lossy().into_owned()
    } else {
        binary.to_string_lossy().into_owned()
    };
    let task = tokio::spawn(async move {
        let mut text = String::new();
        let result = broker::ask_using(
            (&program, &environ),
            "synthetic/test",
            &Context::default(),
            &Wants::default(),
            |delta| {
                if let Delta::Text(part) = delta {
                    text.push_str(&part);
                }
            },
            |_| {},
        )
        .await;
        (result, text)
    });
    let (mut http, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("provider deadline")
        .expect("HTTP");
    let pid = child(binary).expect("selected native provider child");
    let argv = fs::read(format!("/proc/{pid}/cmdline")).expect("native argv");
    assert_eq!(
        argv.split(|byte| *byte == 0)
            .any(|arg| arg == b"--event-bootstrap"),
        enabled && !legacy
    );
    let mut header = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !header.ends_with(b"\r\n\r\n") {
            assert!(header.len() < 65536);
            header.push(http.read_u8().await.expect("header"));
        }
        let header = String::from_utf8(header).expect("header text");
        let len: usize = header
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|len| len.trim().parse().expect("length"))
            })
            .expect("content length");
        assert!(len < 65536);
        http.read_exact(&mut vec![0; len]).await.expect("body");
    })
    .await
    .expect("request deadline");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !task.is_finished(),
        "provider ended with its negotiation worker"
    );
    if enabled && !legacy {
        until(|| {
            runtime::status()
                .summary
                .is_some_and(|summary| summary.remote_producers == 1 && summary.counts.accepted > 0)
        })
        .await;
    }
    if cancel {
        task.abort();
        assert!(task.await.expect_err("cancelled ask").is_cancelled());
    } else {
        let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"synthetic-result\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        http.write_all(headers.as_bytes())
            .await
            .expect("response header");
        http.write_all(body).await.expect("response");
        http.shutdown().await.expect("HTTP EOF");
        let (result, text) = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("ask deadline")
            .expect("ask task");
        result.expect("native answer");
        assert_eq!(text, "synthetic-result");
    }
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "provider was not reaped before return"
    );
    if enabled && !legacy {
        until(|| {
            runtime::status()
                .summary
                .is_some_and(|summary| summary.remote_producers == 0)
        })
        .await;
        assert_eq!(
            runtime::status().summary.expect("summary").counts.invalid,
            0
        );
    }
}

#[test]
fn normal_broker_streams_and_reaps_selected_native_providers() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .thread_keep_alive(Duration::from_millis(10))
        .build()
        .expect("async runtime")
        .block_on(verify());
}

async fn verify() {
    let Some(selected) = std::env::var_os("MELCHIOR_TEST_BINARY") else {
        magi_testkit::live::unavailable("MELCHIOR_TEST_BINARY is not set");
        return;
    };
    let binary = fs::canonicalize(selected).expect("selected binary");
    scenario(&binary, false, false, false).await;
    assert_eq!(runtime::status().state, runtime::StatusKind::Disabled);
    let scope = runtime::Scope::start().expect("collector");
    scenario(&binary, true, false, false).await;
    scenario(&binary, true, true, false).await;
    scenario(&binary, true, false, true).await;
    cancelled_negotiation(&binary).await;
    assert_eq!(runtime::status().unobserved_launches, 1);
    let summary = runtime::status().summary.expect("retired providers");
    assert_eq!(summary.retired.producers, 2);
    assert!(summary.retired.unknown_loss_producers >= 1);
    drop(scope);
    assert!(runtime::status().summary.expect("closed collector").closed);
}

async fn cancelled_negotiation(binary: &Path) {
    let root = Scratch::new("event-broker", "negotiating");
    let wrapper = root.join("probe");
    let marker = root.join("pid");
    let launched = root.join("launched");
    fs::write(&wrapper, format!("#!/bin/sh\nif [ \"$1\" = event-capabilities ]; then\n printf '%s' $$ > '{}'\n sleep 0.2\n exec '{}' event-capabilities\nfi\nprintf launched > '{}'\nexec '{}' \"$@\"\n", marker.display(), binary.display(), launched.display(), binary.display())).expect("slow negotiation wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).expect("probe executable");
    let before = runtime::status()
        .summary
        .expect("collector")
        .retired
        .producers;
    let task = tokio::spawn(async move {
        broker::ask_through(
            wrapper.to_str().expect("program"),
            "synthetic/test",
            &Context::default(),
            &Wants::default(),
            |_| {},
            |_| {},
        )
        .await
    });
    until(|| fs::read_to_string(&marker).is_ok_and(|pid| pid.parse::<u32>().is_ok())).await;
    let pid = fs::read_to_string(&marker).expect("probe pid");
    task.abort();
    assert!(
        task.await
            .expect_err("cancelled negotiation")
            .is_cancelled()
    );
    until(|| !Path::new(&format!("/proc/{pid}")).exists()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !launched.exists(),
        "cancelled negotiation launched a provider"
    );
    let summary = runtime::status().summary.expect("collector");
    assert_eq!(summary.remote_producers, 0);
    assert_eq!(summary.retired.producers, before);
}

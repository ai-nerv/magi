#![allow(clippy::expect_used, clippy::indexing_slicing)]

use magi_host::{catalog::Catalog, session::Session};
use magi_ipc::{FrameReader, FrameWriter};
use magi_model::scratch::Scratch;
use magi_proto::{
    HarnessEvent, SessionId, UiCommand,
    authentication::{Reply, Request},
};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

type Reader = FrameReader<OwnedReadHalf>;
type Writer = FrameWriter<OwnedWriteHalf>;

async fn connect(path: &Path, controls: bool) -> (Reader, Writer) {
    let stream = magi_ipc::connect(path).await.expect("connect");
    let (read, write) = stream.into_split();
    let (mut reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
    writer
        .write(&UiCommand::AttachLatest {
            draws: false,
            key: controls.then(|| magi_ipc::read_key(path)).flatten(),
        })
        .await
        .expect("attach");
    event(&mut reader, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    (reader, writer)
}

async fn event(reader: &mut Reader, predicate: impl Fn(&HarnessEvent) -> bool) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = reader.read().await.expect("event");
            if predicate(&event) {
                return event;
            }
        }
    })
    .await
    .expect("bounded event")
}

async fn auth(reader: &mut Reader, writer: &mut Writer, request: u64, action: Request) -> Reply {
    writer
        .write(&UiCommand::Authenticate { request, action })
        .await
        .expect("auth request");
    match event(reader, |e| matches!(e, HarnessEvent::Authentication { request: id, reply: Reply::Finished { .. } } if *id == request)).await {
        HarnessEvent::Authentication { reply, .. } => reply,
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn authentication_is_controller_only_not_broadcast_or_recorded() {
    let dir = Scratch::new("magi-login", "private");
    let program = dir.join("mind");
    std::fs::write(
        &program,
        r#"#!/bin/sh
[ "$1" = --probe ] && exit 0
if [ "$1" = models ]; then printf '%s\n' '{"ok":true,"result":[]}'; exit; fi
[ "$*" = 'auth --stdio' ] || exit 4
[ "$SESSION_AUTH_ENV" = fixture ] || exit 5
IFS= read -r request
printf '%s\n' '{"event":"finished","providers":[],"error":null}'
"#,
    )
    .expect("fixture");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).expect("executable");
    magi_model::scratch::executable_ready(&program);
    let mut catalog = Catalog::empty();
    catalog.mind = program.to_string_lossy().into_owned();
    catalog
        .environ
        .insert("SESSION_AUTH_ENV".into(), "fixture".into());
    let path = dir.join("s.sock");
    let memory = magi_testkit::memory::Failing::start(&dir.join("m.sock"))
        .await
        .expect("memory");
    let task = tokio::spawn(magi_host::serve_on(
        magi_ipc::bind(&path).await.expect("bind"),
        Session::recorded(SessionId::new("auth"), Vec::new()),
        None,
        catalog,
        Some(memory.socket().to_owned()),
    ));
    let (mut viewer, mut viewer_write) = connect(&path, false).await;
    viewer_write
        .write(&UiCommand::Authenticate {
            request: 1,
            action: Request::List,
        })
        .await
        .expect("watcher auth");
    event(&mut viewer, |e| {
        matches!(
            e,
            HarnessEvent::Authentication {
                reply: Reply::Finished { error: Some(_), .. },
                ..
            }
        )
    })
    .await;
    let (mut owner, mut owner_write) = connect(&path, true).await;
    let result = auth(
        &mut owner,
        &mut owner_write,
        2,
        Request::SetKey {
            provider: "fixture".into(),
            key: "private-login-sentinel".into(),
        },
    )
    .await;
    assert!(matches!(result, Reply::Finished { error: None, .. }));
    assert!(!format!("{result:?}").contains("private-login-sentinel"));
    let observer = tokio::time::timeout(Duration::from_millis(150), async {
        loop {
            assert!(!matches!(
                viewer.read::<HarnessEvent>().await.expect("observer event"),
                HarnessEvent::Authentication { .. }
            ));
        }
    })
    .await;
    assert!(observer.is_err());
    let stream = magi_ipc::connect(&path).await.expect("snapshot connection");
    let (r, w) = stream.into_split();
    let mut reader = FrameReader::new(r);
    let mut writer = FrameWriter::new(w);
    writer
        .write(&UiCommand::AttachLatest {
            draws: false,
            key: None,
        })
        .await
        .expect("snapshot request");
    let snapshot = event(&mut reader, |e| {
        matches!(e, HarnessEvent::SessionSnapshot { .. })
    })
    .await;
    assert!(
        matches!(&snapshot, HarnessEvent::SessionSnapshot { entries, .. } if entries.is_empty())
    );
    assert!(!format!("{snapshot:?}").contains("private-login-sentinel"));
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn native_melchior_login_updates_model_readiness_and_removes_saved_key() {
    let Some(binary) = std::env::var_os("MELCHIOR_TEST_BINARY") else {
        magi_testkit::live::unavailable("MELCHIOR_TEST_BINARY is not set for native login");
        return;
    };
    let binary = std::fs::canonicalize(binary).expect("selected Melchior");
    let dir = Scratch::new("magi-login", "native");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .expect("private fixture");
    std::fs::write(dir.join("providers.lua"), r#"
        for _, id in ipairs({"openrouter", "ollama"}) do
            melchior.provider(id, {name=id,api="openai-completions",auth={kind="none"}})
        end
        melchior.provider("login-fixture", {name="Login fixture",api="openai-completions",base_url="https://fixture.invalid",
            auth={kind="api-key",vars={"LOGIN_FIXTURE_KEY"}},models={{id="m",name="Test",context_window=8192,max_tokens=128}}})
    "#).expect("config");
    let mut catalog = Catalog::empty();
    catalog.mind = binary.to_string_lossy().into_owned();
    for (key, path) in [
        ("HOME", dir.to_path_buf()),
        ("MELCHIOR_CONFIG", dir.to_path_buf()),
        ("XDG_DATA_HOME", dir.join("data")),
        ("XDG_CONFIG_HOME", dir.join("config")),
        ("XDG_CACHE_HOME", dir.join("cache")),
    ] {
        catalog
            .environ
            .insert(key.into(), path.to_string_lossy().into_owned());
    }
    catalog
        .environ
        .insert("LOGIN_FIXTURE_KEY".into(), String::new());
    let memory = magi_testkit::memory::Failing::start(&dir.join("m.sock"))
        .await
        .expect("memory");
    let path = dir.join("s.sock");
    let task = tokio::spawn(magi_host::serve_on(
        magi_ipc::bind(&path).await.expect("bind"),
        Session::recorded(SessionId::new("native-auth"), Vec::new()),
        None,
        catalog,
        Some(memory.socket().to_owned()),
    ));
    let (mut reader, mut writer) = connect(&path, true).await;
    let result = auth(
        &mut reader,
        &mut writer,
        1,
        Request::SetKey {
            provider: "login-fixture".into(),
            key: "native-private-sentinel".into(),
        },
    )
    .await;
    assert!(
        matches!(&result, Reply::Finished { error:None, providers } if providers.iter().any(|p| p.id == "login-fixture" && p.saved && p.source == "saved")),
        "{result:?}"
    );
    let refreshed = event(&mut reader, |e| {
        matches!(e, HarnessEvent::ModelsRefreshed { .. })
    })
    .await;
    assert!(
        matches!(refreshed, HarnessEvent::ModelsRefreshed { choices, warning:None } if choices.iter().any(|c| c.name == "login-fixture/m" && c.requirement.is_empty()))
    );
    let result = auth(
        &mut reader,
        &mut writer,
        2,
        Request::Remove {
            provider: "login-fixture".into(),
        },
    )
    .await;
    assert!(matches!(result, Reply::Finished { error: None, .. }));
    let refreshed = event(&mut reader, |e| {
        matches!(e, HarnessEvent::ModelsRefreshed { .. })
    })
    .await;
    assert!(
        matches!(refreshed, HarnessEvent::ModelsRefreshed { choices, .. } if choices.iter().any(|c| c.name == "login-fixture/m" && !c.requirement.is_empty()))
    );
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn native_browser_login_cancels_and_reaps_the_selected_melchior_image() {
    native_login_cancellation(false).await;
}

#[tokio::test]
async fn native_copilot_device_login_is_private_and_cancellation_reaps_selected_image() {
    native_login_cancellation(true).await;
}

async fn native_login_cancellation(device: bool) {
    let Some(binary) = std::env::var_os("MELCHIOR_TEST_BINARY") else {
        magi_testkit::live::unavailable("MELCHIOR_TEST_BINARY is not set for browser login");
        return;
    };
    let binary = std::fs::canonicalize(binary).expect("selected Melchior");
    let dir = Scratch::new("magi-login", "native-browser");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .expect("private fixture");
    std::fs::write(dir.join("providers.lua"), r#"
        for _, id in ipairs({"openrouter", "ollama"}) do
            melchior.provider(id, {name=id,api="openai-completions",auth={kind="none"}})
        end
        melchior.provider("login-fixture", {name="OAuth fixture",api="openai-completions",base_url="https://fixture.invalid",
            auth={kind="oauth",service="Fixture",authorize_url="https://fixture.invalid/authorize",token_url="https://fixture.invalid/token",client_id="test-client"}})
    "#).expect("config");
    let server = if device {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("device fixture");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let mut config = std::fs::read_to_string(dir.join("providers.lua")).expect("config");
        config.push_str(&format!(r#"
            melchior.provider("login-fixture", {{name="Device fixture",api="openai-completions",base_url={base:?},
                auth={{kind="github-copilot",client_id="fixture",device_url="{base}/device",oauth_url="{base}/oauth",token_url="{base}/token",verification_url="{base}/verify"}}}})
        "#));
        std::fs::write(dir.join("providers.lua"), config).expect("device config");
        Some(tokio::spawn(async move {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .expect("device deadline")
                .expect("device connection");
            let mut bytes = [0; 8192];
            let n = socket.read(&mut bytes).await.expect("request");
            assert!(String::from_utf8_lossy(&bytes[..n]).starts_with("POST /device "));
            let body = serde_json::json!({"device_code":"private-device-secret", "user_code":"TEST-CODE", "verification_uri":format!("{base}/verify"), "expires_in":900, "interval":60}).to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .expect("reply");
        }))
    } else {
        None
    };
    let program = dir.join("mind");
    std::fs::write(&program, "#!/bin/sh\n[ \"$1\" = --probe ] && exit 0\necho $$ > \"$AUTH_PID\"\nexec \"$AUTH_SELECTED\" \"$@\"\n").expect("wrapper");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).expect("mode");
    magi_model::scratch::executable_ready(&program);
    let mut catalog = Catalog::empty();
    catalog.mind = program.to_string_lossy().into_owned();
    let pidfile = dir.join("pid");
    for (key, path) in [
        ("HOME", dir.to_path_buf()),
        ("MELCHIOR_CONFIG", dir.to_path_buf()),
        ("XDG_DATA_HOME", dir.join("data")),
        ("XDG_CONFIG_HOME", dir.join("config")),
        ("XDG_CACHE_HOME", dir.join("cache")),
        ("AUTH_PID", pidfile.clone()),
        ("AUTH_SELECTED", binary.clone()),
    ] {
        catalog
            .environ
            .insert(key.into(), path.to_string_lossy().into_owned());
    }
    let memory = magi_testkit::memory::Failing::start(&dir.join("m.sock"))
        .await
        .expect("memory");
    let path = dir.join("s.sock");
    let task = tokio::spawn(magi_host::serve_on(
        magi_ipc::bind(&path).await.expect("bind"),
        Session::recorded(SessionId::new("native-browser"), Vec::new()),
        None,
        catalog,
        Some(memory.socket().to_owned()),
    ));
    let (mut reader, mut writer) = connect(&path, true).await;
    let (mut viewer, _viewer_writer) = connect(&path, false).await;
    writer
        .write(&UiCommand::Authenticate {
            request: 1,
            action: Request::Login {
                provider: "login-fixture".into(),
            },
        })
        .await
        .expect("login");
    let browser = event(&mut reader, |e| {
        matches!(
            e,
            HarnessEvent::Authentication {
                reply: Reply::Browser { .. } | Reply::Device { .. },
                ..
            }
        )
    })
    .await;
    if device {
        assert!(
            matches!(browser, HarnessEvent::Authentication { reply:Reply::Device { code, expires_in:900, .. }, .. } if code == "TEST-CODE")
        );
    } else {
        assert!(
            matches!(browser, HarnessEvent::Authentication { reply:Reply::Browser { url }, .. } if url.starts_with("https://fixture.invalid/authorize?"))
        );
    }
    if let Some(server) = server {
        server.await.expect("device server");
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                assert!(!matches!(
                    viewer.read::<HarnessEvent>().await.expect("viewer"),
                    HarnessEvent::Authentication { .. }
                ));
            }
        })
        .await
        .is_err()
    );
    let pid: u32 = std::fs::read_to_string(&pidfile)
        .expect("pid")
        .trim()
        .parse()
        .expect("pid number");
    assert_eq!(
        std::fs::read_link(format!("/proc/{pid}/exe")).expect("running image"),
        binary
    );
    writer
        .write(&UiCommand::CancelAuthentication { request: 1 })
        .await
        .expect("cancel");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::path::Path::new(&format!("/proc/{pid}")).exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "Melchior was not reaped"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let store =
        std::fs::read_to_string(dir.join("data/magi/credentials.json")).expect("attempt store");
    assert!(!store.contains("access"));
    task.abort();
    let _ = task.await;
}

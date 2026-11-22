#![allow(clippy::expect_used, clippy::indexing_slicing)]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, GenericImageView, ImageFormat, Rgb, RgbImage};
use magi_ipc::{FrameReader, FrameWriter};
use magi_model::{Image, scratch::Scratch};
use magi_proto::{Entry, HarnessEvent, UiCommand};
use magi_testkit::{
    Mind,
    mind::{call_lines, stop_line, text_line},
};
use serde_json::{Value, json};
use std::{
    io::Cursor,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

#[path = "image_history/racing.rs"]
mod racing;

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = rustix::process::kill_process_group(
                rustix::process::Pid::from_child(&self.0),
                rustix::process::Signal::Kill,
            );
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

struct Fixture {
    child: Process,
    read: FrameReader<OwnedReadHalf>,
    write: FrameWriter<OwnedWriteHalf>,
    mind: Mind,
    root: Scratch,
}

fn picture(width: u32, height: u32) -> Image {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([200, 30, 60])))
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("picture");
    Image {
        data: STANDARD.encode(bytes.into_inner()),
        media_type: "image/png".into(),
    }
}

fn card(id: &str, edge: u64, max_bytes: u64, vision: bool) -> Value {
    json!({"id":id,"provider":"fake","name":id,"api":"anthropic-messages","ready":true,
        "vision":vision,"context_window":200000,"max_output":4096,"capabilities":{"input":["text","image"],
        "input_limits":{"images":{"resize":{"max_width":edge,"max_height":edge,"max_bytes":max_bytes}}}}})
}

fn cards(root: &Path, edge: u64, max_bytes: u64) {
    std::fs::write(root.join("cards.json"), json!({"ok":true,"n":3,"refreshed":true,"failed":[],"result":[
        card("fake/one", edge, max_bytes, true),card("fake/tight", 5, 1000, true),card("fake/text", 5, 1, false)
    ]}).to_string()).expect("catalog fixture");
}

fn selected(variable: &str) -> Option<PathBuf> {
    let Some(path) = std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
    else {
        magi_testkit::live::unavailable(&format!("image-history native proof needs {variable}"));
        return None;
    };
    Some(std::fs::canonicalize(path).expect("selected artifact"))
}

fn script(path: &Path, source: &str) {
    std::fs::write(path, source).expect("script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("executable");
    magi_model::scratch::executable_ready(path);
}

async fn next(reader: &mut FrameReader<OwnedReadHalf>) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(35), reader.read())
        .await
        .expect("event deadline")
        .expect("event")
}

async fn attach(
    root: &Path,
) -> (
    FrameReader<OwnedReadHalf>,
    FrameWriter<OwnedWriteHalf>,
    HarnessEvent,
) {
    let socket = root.join("r/host.sock");
    let connection = magi_ipc::connect(&socket).await.expect("host socket");
    let (read, write) = connection.into_split();
    let mut reader = FrameReader::new(read);
    let mut writer = FrameWriter::new(write);
    writer
        .write(&UiCommand::AttachLatest {
            draws: false,
            key: magi_ipc::read_key(&socket),
        })
        .await
        .expect("attach");
    let snapshot = next(&mut reader).await;
    assert!(
        matches!(snapshot, HarnessEvent::SessionSnapshot { .. }),
        "{snapshot:?}"
    );
    (reader, writer, snapshot)
}

async fn launch(root: &Path, resume: Option<&str>) -> Process {
    let binary = std::fs::canonicalize(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")))
        .expect("Magi executable");
    let mut command = Command::new(&binary);
    command
        .args(["--headless", "--socket"])
        .arg(root.join("r/host.sock"));
    if let Some(id) = resume {
        command.args(["--resume-run", id]);
    }
    let mut child = Process(
        command
            .env_clear()
            .current_dir(root.join("p"))
            .env(
                "PATH",
                std::env::join_paths(std::iter::once(root.join("bin")).chain(
                    std::env::split_paths(&std::env::var_os("PATH").expect("fixture utilities")),
                ))
                .expect("fixture PATH"),
            )
            .env("HOME", root.join("h"))
            .env("XDG_CONFIG_HOME", root.join("c"))
            .env("XDG_DATA_HOME", root.join("d"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_STATE_HOME", root.join("s"))
            .env("XDG_RUNTIME_DIR", root.join("r"))
            .env("TMPDIR", root.join("tmp"))
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(root.join("out")).expect("stdout"))
            .stderr(std::fs::File::create(root.join("err")).expect("stderr"))
            .spawn()
            .expect("native Magi"),
    );
    tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            assert!(
                child.0.try_wait().expect("status").is_none(),
                "native startup: {}",
                std::fs::read_to_string(root.join("err")).unwrap_or_default()
            );
            if root.join("r/host.sock").exists()
                && magi_ipc::read_key(&root.join("r/host.sock")).is_some()
                && tokio::net::UnixStream::connect(root.join("r/host.sock"))
                    .await
                    .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("native host readiness");
    assert_eq!(
        std::fs::read_link(format!("/proc/{}/exe", child.0.id())).expect("running binary"),
        binary
    );
    child
}

impl Fixture {
    async fn start(name: &str, max_bytes: u64) -> Option<Self> {
        Self::configured(name, max_bytes, None).await
    }

    async fn configured(name: &str, max_bytes: u64, repeated: Option<usize>) -> Option<Self> {
        Self::configured_image(name, max_bytes, repeated, picture(320, 160)).await
    }

    async fn configured_image(
        name: &str,
        max_bytes: u64,
        repeated: Option<usize>,
        pixels: Image,
    ) -> Option<Self> {
        let memory = selected("BALTHASAR_TEST_BINARY")?;
        let tools = selected("CASPER_TEST_BINARY")?;
        let tools_config = selected("CASPER_TEST_TOOLS")?;
        let root = Scratch::new("imh", name).settling();
        for dir in [
            "r", "c/magi", "c/casper", "d", "s", "h", "cache", "tmp", "p/.git", "bin",
        ] {
            let _ = root.private(dir);
        }
        std::fs::copy(tools_config, root.join("c/casper/tools.lua")).expect("tools configuration");
        let launcher = std::env::split_paths(&std::env::var_os("PATH").expect("fixture PATH"))
            .map(|path| path.join("bwrap"))
            .find(|path| path.is_file())
            .expect("bubblewrap prerequisite");
        std::fs::copy(launcher, root.join("bin/bwrap")).expect("private launcher copy");
        std::fs::set_permissions(
            root.join("bin/bwrap"),
            std::fs::Permissions::from_mode(0o555),
        )
        .expect("read-only launcher");
        let mut first = Vec::new();
        for index in 0..repeated.unwrap_or(1) {
            first.extend(
                call_lines(&format!("image-{index}"), "paint", "{}")
                    .into_iter()
                    .take(2),
            );
        }
        first.push(magi_testkit::mind::stopped_line("tool_use"));
        let following = [text_line("image recorded"), stop_line()];
        let first = first.iter().map(String::as_str).collect::<Vec<_>>();
        let following = following.iter().map(String::as_str).collect::<Vec<_>>();
        let turns: Vec<&[&str]> = if repeated.is_some() {
            vec![&first]
        } else {
            vec![&first, &following]
        };
        let mind = Mind::turns(name, &turns);
        cards(&root, 20, max_bytes);
        let wrapper = root.join("mind");
        script(
            &wrapper,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = models ]; then exec cat '{}'; fi\nexec '{}' \"$@\"\n",
                root.join("cards.json").display(),
                mind.program().display()
            ),
        );
        std::fs::write(root.join("c/magi/tools.lua"), format!(
            "magi.tool('paint', {{description='Return pixels',parameters={{type='object'}},transport={{kind='lua'}},run=function() return {{content='pixels',images={{{{data='{}',media_type='image/png'}}}}}} end}})\n", pixels.data)).expect("image tool");
        std::fs::write(root.join("c/magi/init.lua"), format!(
            "magi.model='fake/one'\nmagi.melchior={:?}\nmagi.memory={:?}\nmagi.tools={:?}\nmagi.events=false\nmagi.helpers={{memory=false}}\nmagi.may_spawn=false\nmagi.load('tools.lua')\n",
            wrapper.to_str().expect("model"), memory.to_str().expect("memory"), tools.to_str().expect("tools"))).expect("machine config");
        let child = launch(&root, None).await;
        let (read, write, _) = attach(&root).await;
        Some(Self {
            root,
            mind,
            child,
            read,
            write,
        })
    }

    async fn prompt(&mut self, request: u64, text: &str, images: Vec<Image>) -> Result<(), String> {
        self.write
            .write(&UiCommand::SubmitTracked {
                request,
                text: text.into(),
                aside: String::new(),
                images,
            })
            .await
            .expect("prompt");
        tokio::time::timeout(Duration::from_secs(35), async {
            let mut started = None;
            loop {
                match next(&mut self.read).await {
                    HarnessEvent::TurnStarted {
                        owner,
                        request: Some(id),
                    } if id == request => started = Some(owner),
                    HarnessEvent::TurnFinished { owner } if Some(owner) == started => return Ok(()),
                    HarnessEvent::PromptRefused {
                        request: id,
                        message,
                    } if id == request => return Err(message),
                    _ => {}
                }
            }
        })
        .await
        .expect("prompt completion")
    }

    async fn snapshot(&self) -> (magi_proto::SessionId, Vec<Entry>) {
        let (_, _, snapshot) = attach(&self.root).await;
        let HarnessEvent::SessionSnapshot {
            session, entries, ..
        } = snapshot
        else {
            panic!("snapshot")
        };
        (session, entries)
    }

    async fn choose(&mut self, name: &str) {
        self.write
            .write(&UiCommand::SelectModel {
                request: 91,
                name: name.into(),
            })
            .await
            .expect("select");
        loop {
            if let HarnessEvent::ModelSelected { request: 91, error } = next(&mut self.read).await {
                assert_eq!(error, None);
                break;
            }
        }
    }

    async fn stop(&mut self) {
        rustix::process::kill_process(
            rustix::process::Pid::from_child(&self.child.0),
            rustix::process::Signal::Int,
        )
        .expect("stop native host");
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Some(status) = self.child.0.try_wait().expect("status") {
                    assert!(status.success(), "{status}");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("native host reaped");
    }

    async fn resume(&mut self, id: &str) {
        self.stop().await;
        self.child = launch(&self.root, Some(id)).await;
        let (read, write, snapshot) = attach(&self.root).await;
        assert!(
            matches!(snapshot, HarnessEvent::SessionSnapshot {session, ..} if session.as_str() == id)
        );
        self.read = read;
        self.write = write;
    }
}

fn saved_images(entries: &[Entry]) -> Vec<Image> {
    entries
        .iter()
        .flat_map(|entry| match entry {
            Entry::User { images, .. } | Entry::Workflow { images, .. } => images.as_slice(),
            Entry::Tool {
                result: Some(result),
                ..
            } => &result.images,
            _ => &[],
        })
        .cloned()
        .collect()
}

fn sent_images(ask: &str) -> Vec<Image> {
    let value: Value = serde_json::from_str(ask).expect("captured request");
    let context: magi_model::Context =
        serde_json::from_value(value["context"].clone()).expect("context");
    let mut images = Vec::new();
    for content in context
        .messages
        .into_iter()
        .flat_map(|message| message.content)
    {
        match content {
            magi_model::Content::Image { data, media_type } => {
                images.push(Image { data, media_type })
            }
            magi_model::Content::ToolResult { images: tool, .. } => images.extend(tool),
            _ => {}
        }
    }
    images
}

#[tokio::test]
async fn native_images_are_normalized_before_history_and_preserved_across_refresh_switch_and_resume()
 {
    let Some(mut fixture) = Fixture::start("image-history", 1000).await else {
        return;
    };
    fixture
        .prompt(1, "look", vec![picture(400, 200)])
        .await
        .expect("admitted");
    let (session, entries) = fixture.snapshot().await;
    let images = saved_images(&entries);
    assert_eq!(images.len(), 2, "user and tool images: {entries:?}");
    for image in &images {
        assert!(image.data.len() <= 1000);
        let pixels = image::load_from_memory(&STANDARD.decode(&image.data).expect("base64"))
            .expect("pixels");
        assert_eq!(pixels.dimensions(), (20, 10));
        assert!(pixels.get_pixel(10, 5).0[0] > 190);
    }
    assert_eq!(fixture.mind.asked(), 2);
    assert_eq!(sent_images(&fixture.mind.asks()[1]), images);
    fixture.prompt(2, "again", vec![]).await.expect("followup");
    assert_eq!(sent_images(&fixture.mind.asks()[2]), images);
    fixture.resume(session.as_str()).await;
    assert_eq!(saved_images(&fixture.snapshot().await.1), images);
    fixture
        .prompt(3, "after resume", vec![])
        .await
        .expect("resume followup");
    assert_eq!(sent_images(&fixture.mind.asks()[3]), images);
    cards(&fixture.root, 10, 1000);
    fixture
        .write
        .write(&UiCommand::RefreshModels)
        .await
        .expect("refresh");
    loop {
        if let HarnessEvent::ModelsRefreshed { warning, .. } = next(&mut fixture.read).await {
            assert_eq!(warning, None);
            break;
        }
    }
    let asked = fixture.mind.asked();
    fixture
        .prompt(4, "stricter refresh", vec![])
        .await
        .expect("turn admitted");
    assert_eq!(
        fixture.mind.asked(),
        asked,
        "incompatible history must not dispatch"
    );
    let refreshed = fixture.snapshot().await.1;
    assert_eq!(saved_images(&refreshed), images);
    assert!(refreshed.iter().any(|entry| matches!(entry, Entry::Assistant { error:Some(error), .. } if error.contains("historical image"))));
    fixture.choose("fake/tight").await;
    fixture
        .prompt(5, "stricter model", vec![])
        .await
        .expect("turn admitted");
    assert_eq!(fixture.mind.asked(), asked);
    assert_eq!(saved_images(&fixture.snapshot().await.1), images);
    fixture.choose("fake/text").await;
    fixture
        .prompt(6, "text-only model", vec![])
        .await
        .expect("text-only turn");
    assert_eq!(fixture.mind.asked(), asked + 1);
    assert_eq!(saved_images(&fixture.snapshot().await.1), images);
    fixture.stop().await;
}

#[tokio::test]
async fn native_invalid_new_image_is_refused_before_admission_or_provider_dispatch() {
    let Some(mut fixture) = Fixture::start("image-history-invalid", 1000).await else {
        return;
    };
    let error = fixture
        .prompt(
            7,
            "private prompt",
            vec![Image {
                data: "AAAA".into(),
                media_type: "image/png".into(),
            }],
        )
        .await
        .expect_err("invalid pixels");
    assert!(!error.contains("private prompt"));
    assert_eq!(fixture.mind.asked(), 0);
    assert!(fixture.snapshot().await.1.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn native_unrepresentable_tool_image_is_a_recorded_tool_failure_without_partial_images() {
    let Some(mut fixture) = Fixture::start("image-history-tool-error", 1).await else {
        return;
    };
    fixture.prompt(8, "draw", vec![]).await.expect("turn");
    let entries = fixture.snapshot().await.1;
    let tool = entries
        .iter()
        .find_map(|entry| {
            if let Entry::Tool {
                result: Some(result),
                ..
            } = entry
            {
                Some(result)
            } else {
                None
            }
        })
        .expect("tool result");
    assert!(tool.is_error && tool.images.is_empty());
    assert!(tool.output.contains("Image attachments omitted"));
    assert!(tool.output.starts_with("pixels"));
    assert_eq!(fixture.mind.asked(), 2);
    assert!(sent_images(&fixture.mind.asks()[1]).is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn native_image_failures_stop_repeated_calls_across_rounds_and_within_duplicate_batches() {
    for (name, calls, asks) in [("image-repeat", 1, 4), ("image-duplicates", 4, 1)] {
        let Some(mut fixture) = Fixture::configured(name, 1, Some(calls)).await else {
            return;
        };
        fixture.prompt(9, "draw", vec![]).await.expect("turn");
        let entries = fixture.snapshot().await.1;
        let results: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool {
                    result: Some(result),
                    ..
                } => Some(result),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 4, "{entries:?}");
        for result in &results[..2] {
            assert!(result.is_error && result.images.is_empty());
            assert!(
                result.output.starts_with("pixels")
                    && result.output.contains("Image attachments omitted")
            );
        }
        assert!(
            results[2]
                .output
                .starts_with("Repeated-call guard: not executed")
        );
        assert!(
            results[3]
                .output
                .starts_with("Repeated-call guard: stopped")
        );
        assert_eq!(fixture.mind.asked(), asks);
        assert!(entries.iter().any(|entry| matches!(entry, Entry::Assistant {error: Some(error), ..} if error.starts_with("Repeated-call guard: stopped"))));
        fixture.stop().await;
    }
}

#[tokio::test]
async fn native_workflow_images_are_frozen_after_normalization_and_survive_resume() {
    let Some(mut fixture) = Fixture::start("image-workflow", 1000).await else {
        return;
    };
    let workflow = fixture.root.private("p/.magi/skills/inspect");
    std::fs::write(
        workflow.join("SKILL.md"),
        "---\nname: inspect\ndescription: Inspect pixels\n---\nDescribe the image.\n",
    )
    .expect("workflow");
    fixture
        .prompt(10, "/skill inspect fixture", vec![picture(400, 200)])
        .await
        .expect("workflow admitted");
    let (id, entries) = fixture.snapshot().await;
    let saved = entries
        .iter()
        .find(|entry| matches!(entry, Entry::Workflow { .. }))
        .expect("workflow entry")
        .clone();
    let Entry::Workflow {
        images,
        workflow: snapshot,
        ..
    } = &saved
    else {
        panic!("workflow")
    };
    assert_eq!(snapshot.name, "inspect");
    assert_eq!(snapshot.arguments, "fixture");
    assert_eq!(images.len(), 1);
    assert_eq!(
        image::load_from_memory(&STANDARD.decode(&images[0].data).expect("base64"))
            .expect("pixels")
            .dimensions(),
        (20, 10)
    );
    let all_images = saved_images(&entries);
    assert_eq!(all_images.len(), 2);
    assert_eq!(sent_images(&fixture.mind.asks()[1]), all_images);
    std::fs::write(workflow.join("SKILL.md"), "deleted source content").expect("change source");
    fixture.resume(id.as_str()).await;
    assert!(fixture.snapshot().await.1.contains(&saved));
    fixture.prompt(11, "again", vec![]).await.expect("followup");
    assert_eq!(sent_images(&fixture.mind.asks()[2]), all_images);
    fixture.stop().await;
}

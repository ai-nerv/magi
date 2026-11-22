#![allow(clippy::expect_used, clippy::indexing_slicing)]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{DynamicImage, GenericImageView, ImageFormat, Rgb, RgbImage};
use magi_model::{Image, scratch::Scratch};
use serde_json::{Value, json};
use std::io::{Cursor, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch(dir: &Scratch) -> Worker {
    launch_with(dir, false)
}

fn launch_with(dir: &Scratch, inherited: bool) -> Worker {
    let config = dir.private("config");
    std::fs::create_dir_all(config.join("magi")).expect("config directory");
    std::fs::write(
        config.join("magi/init.lua"),
        "error('worker must not load config')",
    )
    .expect("poison config");
    let binary = magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi"));
    let mut command = if inherited {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "ulimit -t 1 && ulimit -v 131072 && exec \"$@\"",
                "fixture",
            ])
            .arg(binary);
        command
    } else {
        Command::new(binary)
    };
    Worker(
        command
            .arg(magi_host::images::WORKER_ARG)
            .env_clear()
            .env("HOME", &**dir)
            .env("XDG_CONFIG_HOME", config)
            .env("XDG_DATA_HOME", dir.private("data"))
            .env("XDG_CACHE_HOME", dir.private("cache"))
            .env("XDG_RUNTIME_DIR", dir.private("run"))
            .env("PATH", dir.private("empty-path"))
            .current_dir(&**dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("native worker"),
    )
}

fn response(mut worker: Worker, input: &[u8]) -> Value {
    let mut stdin = worker.0.stdin.take().expect("stdin");
    let input = input.to_vec();
    let writing = std::thread::spawn(move || stdin.write_all(&input));
    let stdout = worker.0.stdout.take().expect("stdout");
    let reading = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .expect("reply");
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut expired = false;
    let status = loop {
        if let Some(status) = worker.0.try_wait().expect("child status") {
            break status;
        }
        if Instant::now() >= deadline {
            expired = true;
            worker.0.kill().expect("terminate hung worker");
            break worker.0.wait().expect("reap hung worker");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = reading.join().expect("reader joined");
    let write = writing.join().expect("writer joined");
    assert!(!expired, "worker exceeded wall deadline");
    let mut stderr = String::new();
    worker
        .0
        .stderr
        .take()
        .expect("stderr")
        .take(4096)
        .read_to_string(&mut stderr)
        .expect("stderr text");
    assert!(status.success(), "status {status}; stderr {stderr}");
    assert!(
        stderr.is_empty(),
        "worker exposed data or loaded configuration: {stderr}"
    );
    assert!(write.is_ok() || stdout.windows(6).any(|part| part == b"failed"));
    serde_json::from_slice(&stdout).expect("typed reply")
}

fn original() -> Image {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(RgbImage::from_pixel(400, 200, Rgb([200, 50, 25])))
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("fixture PNG");
    Image {
        data: STANDARD.encode(bytes.into_inner()),
        media_type: "image/png".into(),
    }
}

fn call(dir: &Scratch, mode: &str, image: &Image, profile: Value) -> Value {
    response(
        launch(dir),
        &serde_json::to_vec(&json!({
            "version":1,"mode":mode,"profile":profile,"images":[image]
        }))
        .expect("request"),
    )
}

#[test]
fn native_worker_resizes_pixels_and_replay_preserves_bytes() {
    let dir = Scratch::new("magi-image-worker", "pixels");
    let original = original();
    let fits = call(&dir, "normalize", &original, json!({}));
    assert_eq!(
        fits["images"][0],
        serde_json::to_value(&original).expect("image")
    );
    let profile = json!({"max_width":20,"max_height":15,"max_bytes":1000});
    let normalized = call(&dir, "normalize", &original, profile.clone());
    assert_eq!(normalized["event"], "ok");
    let image: Image =
        serde_json::from_value(normalized["images"][0].clone()).expect("normalized image");
    assert!(image.data.len() <= 1000);
    let pixels = image::load_from_memory(&STANDARD.decode(&image.data).expect("base64"))
        .expect("decode pixels");
    assert_eq!(pixels.dimensions(), (20, 10));
    assert!(pixels.get_pixel(10, 5).0[0] > 190);
    let replay = call(&dir, "validate", &image, profile.clone());
    assert_eq!(replay, normalized);
    let refused = call(&dir, "validate", &original, profile);
    assert_eq!(
        refused,
        json!({"event":"failed","version":1,"error":"history"})
    );
}

#[test]
fn native_worker_applies_hard_resource_limits_before_reading_input() {
    let dir = Scratch::new("magi-image-worker", "limits");
    let worker = launch(&dir);
    let path = format!("/proc/{}/limits", worker.0.id());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let limits = std::fs::read_to_string(&path).expect("worker proc limits");
        let expected = [
            ("Max address space", "268435456"),
            ("Max cpu time", "4"),
            ("Max core file size", "0"),
            ("Max file size", "0"),
        ];
        if expected.iter().all(|(name, cap)| {
            limits
                .lines()
                .find(|line| line.starts_with(name))
                .is_some_and(|line| {
                    line.split_whitespace()
                        .rev()
                        .skip(1)
                        .take(2)
                        .all(|value| value == *cap)
                })
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "worker did not establish resource limits: {limits}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        response(
            worker,
            b"{\"version\":1,\"mode\":\"normalize\",\"profile\":{},\"images\":[]}"
        ),
        json!({"event":"ok","version":1,"images":[]})
    );
}

#[test]
fn native_worker_never_raises_stricter_inherited_limits() {
    let dir = Scratch::new("magi-image-worker", "inherited-limits");
    let worker = launch_with(&dir, true);
    let path = format!("/proc/{}/limits", worker.0.id());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let limits = std::fs::read_to_string(&path).expect("worker limits");
        if [
            ("Max address space", "134217728"),
            ("Max cpu time", "1"),
            ("Max file size", "0"),
        ]
        .iter()
        .all(|(name, cap)| {
            limits
                .lines()
                .find(|line| line.starts_with(name))
                .is_some_and(|line| {
                    line.split_whitespace()
                        .rev()
                        .skip(1)
                        .take(2)
                        .all(|value| value == *cap)
                })
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "inherited limits changed: {limits}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        response(
            worker,
            b"{\"version\":1,\"mode\":\"normalize\",\"profile\":{},\"images\":[]}"
        ),
        json!({"event":"ok","version":1,"images":[]})
    );
}

#[test]
fn native_worker_refuses_invalid_protocols_and_input_floods_without_echoing() {
    let dir = Scratch::new("magi-image-worker", "invalid");
    for input in [
        b"private-input-not-json".to_vec(),
        br#"{"version":2,"mode":"normalize","profile":{},"images":[]}"#.to_vec(),
        br#"{"version":1,"mode":"normalize","profile":{"max_width":0},"images":[]}"#.to_vec(),
        br#"{"version":1,"mode":"normalize","profile":{},"images":[],"extra":"private"}"#.to_vec(),
    ] {
        assert_eq!(
            response(launch(&dir), &input),
            json!({"event":"failed","version":1,"error":"invalid"})
        );
    }
    assert_eq!(
        response(launch(&dir), &vec![b'x'; 1024 * 1024 + 1]),
        json!({"event":"failed","version":1,"error":"resource"})
    );
}

#[test]
fn native_worker_dispatch_rejects_extra_arguments() {
    let dir = Scratch::new("magi-image-worker", "args");
    let output = Command::new(magi_testkit::live::binary(env!("CARGO_BIN_EXE_magi")))
        .env_clear()
        .current_dir(&*dir)
        .args([magi_host::images::WORKER_ARG, "private-extra"])
        .output()
        .expect("worker dispatch");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).expect("error");
    assert!(error.contains("image worker takes only a bounded stdin request"));
    assert!(!error.contains("private-extra"));
}

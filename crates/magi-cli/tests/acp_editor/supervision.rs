use anyhow::{Context, Result, bail, ensure};
use std::{io::Write, path::Path, process::ExitStatus, time::Duration};
use tokio::{io::AsyncReadExt, process::Child};

pub const LIMIT: usize = 1_048_576;

async fn capture(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    path: &Path,
    limit: usize,
) -> Result<()> {
    let mut file = std::fs::File::create(path)?;
    let mut remaining = limit;
    let mut bytes = [0; 8192];
    loop {
        let capacity = bytes.len().min(remaining.saturating_add(1));
        let count = reader.read(&mut bytes[..capacity]).await?;
        if count == 0 {
            return Ok(());
        }
        file.write_all(&bytes[..count.min(remaining)])?;
        ensure!(
            count <= remaining,
            "{} exceeded {limit} bytes",
            path.display()
        );
        remaining -= count;
    }
}

pub fn preserve(root: &Path, evidence: &Path, name: &str) -> Result<()> {
    use std::io::Read;
    let source = root.join(name);
    if !source.exists() {
        return Ok(());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(source)?
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    std::fs::write(evidence.join(name), &bytes[..bytes.len().min(LIMIT)])?;
    ensure!(bytes.len() <= LIMIT, "{name} exceeded evidence limit");
    Ok(())
}

pub async fn run(
    child: &mut Child,
    evidence: &Path,
    deadline: Duration,
    steps: impl Future<Output = Result<()>>,
) -> Result<ExitStatus> {
    let input = child.stdin.take();
    let stdout = child.stdout.take().context("editor stdout")?;
    let stderr = child.stderr.take().context("editor stderr")?;
    let out = evidence.join("editor.stdout");
    let err = evidence.join("editor.stderr");
    let result = tokio::time::timeout(deadline, async {
        tokio::try_join!(
            async {
                tokio::select! {
                    biased;
                    result = steps => result?,
                    status = child.wait() => bail!("editor exited before scenario completed: {:?}", status?),
                }
                Ok(child.wait().await?)
            },
            capture(stdout, &out, LIMIT),
            capture(stderr, &err, LIMIT),
        )
    })
    .await
    .context("editor overall deadline")
    .and_then(|result| result);
    if result.is_err() {
        let _ = child.kill().await;
        child.wait().await.context("reap failed editor")?;
    }
    drop(input);
    result.map(|(status, (), ())| status)
}

#[tokio::test]
async fn capture_accepts_exact_limit_and_eof() {
    let root = magi_model::scratch::Scratch::new("editor-supervision", "exact");
    capture(&b"abcd"[..], &root.join("out"), 4)
        .await
        .expect("capture");
    assert_eq!(std::fs::read(root.join("out")).expect("output"), b"abcd");
}

#[tokio::test]
async fn capture_refuses_overflow_without_waiting_for_eof() {
    use tokio::io::AsyncWriteExt;
    let root = magi_model::scratch::Scratch::new("editor-supervision", "overflow");
    let (mut writer, reader) = tokio::io::duplex(16);
    writer.write_all(b"abcde").await.expect("write");
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        capture(reader, &root.join("out"), 4),
    )
    .await
    .expect("no EOF required");
    assert!(
        result
            .expect_err("overflow")
            .to_string()
            .contains("exceeded")
    );
    assert_eq!(std::fs::read(root.join("out")).expect("output"), b"abcd");
    drop(writer);
}

#[test]
fn preserve_refuses_oversized_artifacts_but_retains_bounded_prefix() {
    let root = magi_model::scratch::Scratch::new("editor-supervision", "artifact");
    let evidence = root.join("evidence");
    std::fs::create_dir(&evidence).expect("evidence");
    std::fs::write(root.join("large"), vec![b'x'; LIMIT + 1]).expect("artifact");
    assert!(preserve(&root, &evidence, "large").is_err());
    assert_eq!(
        std::fs::metadata(evidence.join("large"))
            .expect("prefix")
            .len(),
        LIMIT as u64
    );
}

#[tokio::test]
async fn supervisor_deadline_kills_and_reaps_a_stalled_editor() {
    let root = magi_model::scratch::Scratch::new("editor-supervision", "deadline");
    let mut child = tokio::process::Command::new(super::utility("sh"))
        .args(["-c", "printf diagnostic >&2; while :; do :; done"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("stalled editor");
    let result = run(
        &mut child,
        &root,
        Duration::from_secs(1),
        std::future::pending(),
    )
    .await;
    assert!(
        result
            .expect_err("deadline")
            .to_string()
            .contains("deadline")
    );
    assert!(child.try_wait().expect("reaped").is_some());
    assert_eq!(
        std::fs::read(root.join("editor.stderr")).expect("retained stderr"),
        b"diagnostic"
    );
}

#[tokio::test]
async fn supervisor_reports_early_exit_without_waiting_for_markers() {
    let root = magi_model::scratch::Scratch::new("editor-supervision", "early-exit");
    let mut child = tokio::process::Command::new(super::utility("sh"))
        .args(["-c", "exit 7"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("exiting editor");
    let result = run(
        &mut child,
        &root,
        Duration::from_secs(5),
        std::future::pending(),
    )
    .await;
    assert!(
        result
            .expect_err("early exit")
            .to_string()
            .contains("before scenario completed")
    );
    assert_eq!(
        child.try_wait().expect("reaped").expect("exit").code(),
        Some(7)
    );
}

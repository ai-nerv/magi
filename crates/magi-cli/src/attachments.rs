//! User-selected raster files attached to a prompt through Casper's image reader.

use anyhow::{Context as _, Result};
use magi_proto::UiCommand;
use magi_tools::supplier::Tooling;

/// The program images are read through: the session's tools program, resolved to one absolute
/// path and held to its pin as the session's own tools are, checked once and run by that path.
#[derive(Default)]
pub(crate) struct Reader {
    tooling: Tooling,
    checked: std::sync::OnceLock<Result<String, String>>,
}

impl Reader {
    pub(crate) fn of(tooling: Tooling) -> Self {
        Self {
            tooling,
            checked: std::sync::OnceLock::new(),
        }
    }

    fn program(&self) -> Result<&str> {
        match self
            .checked
            .get_or_init(|| magi_tools::supplier::verified(&self.tooling))
        {
            Ok(program) => Ok(program),
            Err(why) => anyhow::bail!("Image attachments refused: {why}"),
        }
    }
}

pub(crate) async fn prepare(command: UiCommand, reader: &Reader) -> Result<UiCommand> {
    let UiCommand::SubmitPrompt {
        text,
        aside,
        mut images,
    } = command
    else {
        return Ok(command);
    };
    for path in references(&text)? {
        if images.len() >= 4 {
            anyhow::bail!("At most four image attachments per prompt.");
        }
        images.push(read(reader.program()?, &path).await?);
    }
    Ok(UiCommand::SubmitPrompt {
        text,
        aside,
        images,
    })
}

fn references(text: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut chars = text.chars().peekable();
    let mut boundary = true;
    let mut paths = Vec::new();
    while let Some(c) = chars.next() {
        if c != '@' || !boundary {
            boundary = c.is_whitespace();
            continue;
        }
        let quote = chars.peek().copied().filter(|c| *c == '\'' || *c == '"');
        if quote.is_some() {
            chars.next();
        }
        let mut path = String::new();
        let mut closed = quote.is_none();
        while let Some(c) = chars.next() {
            if Some(c) == quote {
                closed = true;
                break;
            }
            if quote.is_none() && c.is_whitespace() {
                break;
            }
            if c == '\\' {
                if let Some(next) = chars.next() {
                    path.push(next);
                }
            } else {
                path.push(c);
            }
        }
        boundary = quote.is_none();
        if quote.is_none() {
            path = path
                .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}'])
                .to_owned();
        }
        let extension = std::path::Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
        ) {
            continue;
        }
        if !closed {
            anyhow::bail!("Unclosed quote in image attachment: {path}");
        }
        let path = if let Some(rest) = path.strip_prefix("~/") {
            std::path::PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
                .join(rest)
        } else {
            path.into()
        };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

async fn read(program: &str, path: &std::path::Path) -> Result<magi_model::Image> {
    use tokio::io::AsyncReadExt as _;
    let mut child = tokio::process::Command::new(program)
        .arg("image")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("Image attachments require Casper's image reader")?;
    let operation = async {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .context("no image reader output")?
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() > 256 * 1024 {
            anyhow::bail!("Image reader exceeded its output limit.");
        }
        let status = child.wait().await?;
        anyhow::ensure!(
            status.success(),
            "Image reader failed for {}",
            path.display()
        );
        let reply: serde_json::Value = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            reply.get("ok") == Some(&serde_json::Value::Bool(true)),
            "Cannot attach {}: {}",
            path.display(),
            reply
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("image read refused")
        );
        serde_json::from_value(
            reply
                .pointer("/result/0")
                .context("missing image result")?
                .clone(),
        )
        .with_context(|| format!("{} is not a supported raster image", path.display()))
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), operation)
        .await
        .context("Image reader timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_references_support_quotes_spaces_and_deduplication() {
        let refs = references(r#"compare @a.png, @"two words.JPG" @a.png. @dir\ with\ spaces/image.webp @README.md user@host.png"#).expect("references");
        assert_eq!(
            refs,
            ["a.png", "two words.JPG", "dir with spaces/image.webp"].map(std::path::PathBuf::from)
        );
        assert!(references("look @\"unclosed.png").is_err());
        assert!(
            references("ordinary text and name@example.png")
                .expect("text")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn commands_without_image_references_are_unchanged() {
        let command = UiCommand::SubmitPrompt {
            text: "hello".into(),
            aside: "context".into(),
            images: Vec::new(),
        };
        let reader = Reader::default();
        assert_eq!(
            prepare(command.clone(), &reader).await.expect("prompt"),
            command
        );
        assert_eq!(
            prepare(UiCommand::Interrupt, &reader)
                .await
                .expect("command"),
            UiCommand::Interrupt
        );
    }

    /// A tools program in `dir` that answers `image` with one picture and says how it was run.
    fn image_reader(dir: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let program = dir.join("reader");
        std::fs::write(
            &program,
            "#!/bin/sh\nprintf '%s' \"$0\" > \"$(dirname \"$0\")/ran\"\n\
             printf '{\"ok\":true,\"result\":[{\"data\":\"iVBORw0K\",\"media_type\":\"image/png\"}]}'\n",
        )
        .expect("reader");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).expect("mode");
        while let Err(why) = std::process::Command::new(&program).status() {
            assert_eq!(why.raw_os_error(), Some(26), "{why}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = std::fs::remove_file(dir.join("ran"));
        program
    }

    fn prompt(dir: &std::path::Path) -> UiCommand {
        UiCommand::SubmitPrompt {
            text: format!("look at @{}", dir.join("shot.png").display()),
            aside: String::new(),
            images: Vec::new(),
        }
    }

    #[tokio::test]
    async fn an_image_is_read_by_the_tools_program_at_its_resolved_absolute_path() {
        let dir = magi_model::scratch::Scratch::new("magi-attach", "located");
        let program = image_reader(&dir);
        // Named through a link, as `PATH` might name it: run by the one path the link resolves to.
        let linked = dir.join("linked");
        std::os::unix::fs::symlink(&program, &linked).expect("link");
        let reader = Reader::of(Tooling {
            program: linked.display().to_string(),
            ..Tooling::default()
        });
        let UiCommand::SubmitPrompt { images, .. } =
            prepare(prompt(&dir), &reader).await.expect("attached")
        else {
            panic!("a prompt");
        };
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "image/png");
        let ran = std::fs::read_to_string(dir.join("ran")).expect("ran");
        assert_eq!(
            std::path::Path::new(&ran),
            program.canonicalize().expect("real")
        );
    }

    #[tokio::test]
    async fn a_tools_program_that_is_not_the_pinned_one_reads_no_image() {
        let dir = magi_model::scratch::Scratch::new("magi-attach", "unpinned");
        let program = image_reader(&dir);
        let reader = Reader::of(Tooling {
            program: program.display().to_string(),
            pin: Some("0".repeat(64)),
            ..Tooling::default()
        });
        let why = prepare(prompt(&dir), &reader).await.expect_err("refused");
        assert!(format!("{why:#}").contains("pinned"), "{why:#}");
        assert!(!dir.join("ran").exists(), "the unpinned program was run");
    }
}

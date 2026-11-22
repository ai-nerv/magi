use super::{INLINE_BYTES, validate};
use magi_model::{Content, Context, Image, ImageResize};
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};

const KEPT: usize = 512;

#[derive(Default)]
pub(crate) struct History {
    checked: HashSet<[u8; 32]>,
    order: VecDeque<[u8; 32]>,
}

impl History {
    pub(crate) async fn check(
        &mut self,
        context: &Context,
        profile: &ImageResize,
    ) -> Result<(), String> {
        let mut pending = Vec::new();
        for part in context.messages.iter().flat_map(|message| &message.content) {
            match part {
                Content::Image { data, media_type } => {
                    self.queue(data, media_type, profile, &mut pending).await?
                }
                Content::ToolResult { images, .. } => {
                    magi_model::validate_images(images)?;
                    for image in images {
                        self.queue(&image.data, &image.media_type, profile, &mut pending)
                            .await?;
                    }
                }
                _ => {}
            }
        }
        self.batch(&mut pending, profile).await
    }

    async fn queue(
        &mut self,
        data: &str,
        media_type: &str,
        profile: &ImageResize,
        pending: &mut Vec<Image>,
    ) -> Result<(), String> {
        if !matches!(
            media_type,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp"
        ) {
            return Err(
                "Historical image has an unsupported media type; history was not changed.".into(),
            );
        }
        if data.len() > INLINE_BYTES.div_ceil(3) * 4 {
            return Err(
                "Historical image exceeds the inline byte limit; history was not changed.".into(),
            );
        }
        if !self.checked.contains(&key(data, media_type, profile)) {
            let image = Image {
                data: data.to_owned(),
                media_type: media_type.to_owned(),
            };
            magi_model::validate_images(std::slice::from_ref(&image))?;
            pending.push(image);
            if pending.len() == 4 {
                self.batch(pending, profile).await?;
            }
        }
        Ok(())
    }

    async fn batch(
        &mut self,
        pending: &mut Vec<Image>,
        profile: &ImageResize,
    ) -> Result<(), String> {
        if pending.is_empty() {
            return Ok(());
        }
        validate(pending, profile).await?;
        for image in pending.drain(..) {
            self.remember(key(&image.data, &image.media_type, profile));
        }
        Ok(())
    }

    fn remember(&mut self, key: [u8; 32]) {
        if !self.checked.insert(key) {
            return;
        }
        self.order.push_back(key);
        if self.order.len() > KEPT
            && let Some(old) = self.order.pop_front()
        {
            self.checked.remove(&old);
        }
    }
}

fn key(data: &str, media_type: &str, profile: &ImageResize) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"magi-immutable-image-v1\0");
    for value in [profile.max_width, profile.max_height, profile.max_bytes] {
        hash.update(value.map_or(0, std::num::NonZeroU64::get).to_be_bytes());
    }
    hash.update([profile.jpeg_quality.map_or(0, u8::from)]);
    hash.update((media_type.len() as u64).to_be_bytes());
    hash.update(media_type.as_bytes());
    hash.update(data.as_bytes());
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validation_cache_is_bounded_and_keys_include_bytes_type_and_every_profile_field() {
        let base = ImageResize::default();
        let first = key("AAAA", "image/png", &base);
        for profile in [
            json!({"max_width":1}),
            json!({"max_height":1}),
            json!({"max_bytes":1}),
            json!({"jpeg_quality":80}),
        ] {
            assert_ne!(
                first,
                key(
                    "AAAA",
                    "image/png",
                    &serde_json::from_value(profile).expect("profile")
                )
            );
        }
        assert_ne!(first, key("AAAB", "image/png", &base));
        assert_ne!(first, key("AAAA", "image/jpeg", &base));
        let mut cache = History::default();
        cache.remember(first);
        cache.remember(first);
        assert_eq!(cache.order.len(), 1);
        for index in 0..KEPT {
            cache.remember(key(&index.to_string(), "image/png", &base));
        }
        assert_eq!(cache.order.len(), KEPT);
        assert_eq!(cache.checked.len(), KEPT);
        assert!(!cache.checked.contains(&first));
    }

    #[tokio::test]
    async fn cached_validation_preserves_context_and_does_not_traverse_tool_arguments() {
        let profile = ImageResize::default();
        let mut checked = History::default();
        checked.remember(key("AAAA", "image/png", &profile));
        let context: Context = serde_json::from_value(json!({"messages":[{"role":"user","content":[
            {"type":"image","data":"AAAA","media_type":"image/png"},
            {"type":"tool_result","id":"r","name":"read","content":"read","is_error":false,"images":[{"data":"AAAA","media_type":"image/png"}]},
            {"type":"tool_call","id":"c","name":"save","arguments":{"type":"image","data":"private-invalid","media_type":"image/png"}}
        ]}]})).expect("context");
        let before = serde_json::to_vec(&context).expect("bytes");
        checked
            .check(&context, &profile)
            .await
            .expect("already validated");
        assert_eq!(serde_json::to_vec(&context).expect("bytes"), before);
        let mut invalid = context;
        invalid.messages[0].content.push(Content::Image {
            data: "!".into(),
            media_type: "image/png".into(),
        });
        let error = checked
            .check(&invalid, &profile)
            .await
            .expect_err("uncached invalid image");
        assert!(!error.contains("private-invalid"));
    }
}

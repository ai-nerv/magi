use crate::{
    cancel::Cancel,
    catalog::Backend,
    images::{History, Profile},
    session::Session,
};
use tokio::sync::Mutex;

pub(super) fn profile(held: &Session, backend: &Backend) -> Profile {
    held.image_profile(Some((&backend.model, backend.image_resize.as_ref())))
}

pub(super) async fn history(
    session: &Mutex<Session>,
    backend: &Backend,
    context: &magi_model::Context,
    checked: &mut History,
    cancel: &Cancel,
) -> Result<Profile, crate::HostError> {
    let selected = profile(&*session.lock().await, backend);
    if let Some(resize) = &selected.resize {
        let validation = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            checked.check(context, resize),
        );
        tokio::select! {
            biased;
            () = cancel.requested() => {},
            result = validation => result.map_err(|_| crate::HostError::Refused("Historical image validation timed out; history was not changed.".into()))?
                .map_err(crate::HostError::Refused)?,
        }
    }
    Ok(selected)
}

pub(super) async fn output(
    session: &Mutex<Session>,
    backend: &Backend,
    output: &mut magi_tools::Output,
    cancel: &Cancel,
) -> Option<Profile> {
    if output.images.is_empty() {
        return None;
    }
    let selected = profile(&*session.lock().await, backend);
    if let Some(resize) = &selected.resize {
        let images = std::mem::take(&mut output.images);
        let result = tokio::select! {
            biased;
            () = cancel.requested() => Err("Image preparation was cancelled.".into()),
            result = crate::images::normalize(images, resize) => result,
        };
        match result {
            Ok(images) => output.images = images,
            Err(why) => omit(output, &why),
        }
    }
    Some(selected)
}

pub(super) fn fence(output: &mut magi_tools::Output, prepared: Option<Profile>, current: &Profile) {
    if !output.images.is_empty() && prepared.as_ref().is_some_and(|profile| profile != current) {
        omit(
            output,
            "Model image limits changed while preparing the tool result.",
        );
    }
}

fn omit(output: &mut magi_tools::Output, reason: &str) {
    output.images.clear();
    output.is_error = true;
    output
        .content
        .push_str(&format!("\n[Image attachments omitted: {reason}]"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn backend() -> Backend {
        let mut catalog = crate::catalog::Catalog::empty();
        catalog.cards = vec![
            serde_json::from_value(json!({"id":"fake/one","provider":"fake","name":"one",
            "api":"anthropic-messages","ready":true,"vision":true,"capabilities":{"input":[],
            "input_limits":{"images":{"resize":{"max_width":20}}}}}))
            .expect("card"),
        ];
        catalog.backend("fake/one").expect("backend")
    }

    fn result() -> magi_tools::Output {
        let mut result = magi_tools::Output::ok("pixels");
        result.images = vec![magi_model::Image {
            data: "AAAA".into(),
            media_type: "image/png".into(),
        }];
        result
    }

    #[tokio::test]
    async fn cancelled_image_work_clears_all_tool_images_and_preserves_other_output() {
        let session = Mutex::new(Session::recorded(
            magi_proto::SessionId::new("cancel"),
            vec![],
        ));
        let cancel = Cancel::default();
        cancel.request();
        let mut result = result();
        output(&session, &backend(), &mut result, &cancel)
            .await
            .expect("profile");
        assert!(result.is_error && result.images.is_empty());
        assert!(
            result
                .content
                .starts_with("pixels\n[Image attachments omitted:")
        );
        assert!(result.content.contains("cancelled"));
    }

    #[test]
    fn profile_change_fails_the_whole_image_result_without_removing_text() {
        let mut result = result();
        let old = Profile {
            model: Some("fake/one".into()),
            resize: backend().image_resize,
        };
        fence(&mut result, Some(old), &Profile::default());
        assert!(result.images.is_empty() && result.is_error);
        assert!(result.content.starts_with("pixels\n"));
    }
}

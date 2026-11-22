use super::Session;
use crate::images::Profile;
use magi_model::ImageResize;

impl Session {
    pub(crate) fn image_profile(&self, backend: Option<(&str, Option<&ImageResize>)>) -> Profile {
        let Some((model, fallback)) = backend else {
            return Profile::default();
        };
        let resize = match self.choices.iter().find(|choice| choice.name == model) {
            Some(choice) if choice.vision => choice
                .capabilities
                .as_ref()
                .and_then(|cap| cap.input_limits.as_ref())
                .and_then(|limits| limits.images.as_ref())
                .and_then(|images| images.resize.clone()),
            Some(_) => None,
            None => fallback.cloned(),
        };
        Profile {
            model: Some(model.to_owned()),
            resize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn limits(edge: u64) -> ImageResize {
        serde_json::from_value(json!({"max_width":edge})).expect("profile")
    }

    fn choice(vision: bool, profile: Option<ImageResize>) -> magi_proto::ModelChoice {
        serde_json::from_value(json!({"name":"fixture/m","context_window":1000,"vision":vision,"reasoning":false,
            "requirement":"","wants_vars":[],"capabilities":{"input":[],"input_limits":{"images":{"resize":profile}}}})).expect("choice")
    }

    #[test]
    fn fresh_choice_overrides_backend_but_missing_choice_keeps_the_captured_profile() {
        let mut session = Session::recorded(magi_proto::SessionId::new("images"), vec![]);
        let old = limits(2000);
        let backend = Some(("fixture/m", Some(&old)));
        assert_eq!(session.image_profile(backend).resize, Some(old.clone()));
        session.set_choices(vec![choice(true, Some(limits(500)))]);
        assert_eq!(session.image_profile(backend).resize, Some(limits(500)));
        session.set_choices(vec![choice(true, None)]);
        assert_eq!(session.image_profile(backend).resize, None);
        session.set_choices(vec![choice(false, Some(limits(500)))]);
        assert_eq!(session.image_profile(backend).resize, None);
        assert_eq!(session.image_profile(None), Profile::default());
    }
}

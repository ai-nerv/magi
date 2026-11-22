use super::Session;
use magi_model::RequestFrame;
use magi_proto::ask::Wants;
use std::{borrow::Cow, collections::VecDeque};

const MOST: usize = 4096;

#[derive(Default)]
pub(super) struct Fallbacks(VecDeque<[u8; 32]>);

impl Fallbacks {
    pub(super) fn observe(&mut self, mind: &str, model: &str, frame: RequestFrame) {
        if frame.version != 1 || !frame.disable_preferred_strict {
            return;
        }
        let Some(key) = crate::frames::fingerprint(&(mind, model)) else {
            return;
        };
        if self.0.contains(&key) {
            return;
        }
        if self.0.len() == MOST {
            self.0.pop_front();
        }
        self.0.push_back(key);
    }
}

impl Session {
    pub(crate) fn strict_wants<'a>(
        &self,
        mind: &str,
        model: &str,
        wants: &'a Wants,
    ) -> Cow<'a, Wants> {
        if !wants.disable_preferred_strict
            && crate::frames::fingerprint(&(mind, model))
                .is_some_and(|key| self.strict.0.contains(&key))
        {
            let mut effective = wants.clone();
            effective.disable_preferred_strict = true;
            Cow::Owned(effective)
        } else {
            Cow::Borrowed(wants)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_proto::{ModelInfo, SessionId};

    fn frame() -> RequestFrame {
        RequestFrame {
            version: 1,
            dialect: [1; 32],
            policy: [2; 32],
            omitted_images: 0,
            disable_preferred_strict: true,
        }
    }

    #[test]
    fn fallback_is_scoped_to_the_request_identity_and_session_and_is_reset_on_resume() {
        let mut session = Session::recorded(SessionId::new("one"), vec![]);
        let other = Session::recorded(SessionId::new("two"), vec![]);
        let wants = Wants::default();
        session.set_model(Some(ModelInfo {
            name: "a-different-current-model".into(),
            context_window: 4096,
        }));
        session.report_provider_frame("role", "p/m", frame());
        assert!(
            session
                .strict_wants("role", "p/m", &wants)
                .disable_preferred_strict
        );
        assert!(
            !session
                .strict_wants("role", "p/n", &wants)
                .disable_preferred_strict
        );
        assert!(
            !session
                .strict_wants("other", "p/m", &wants)
                .disable_preferred_strict
        );
        assert!(
            !other
                .strict_wants("role", "p/m", &wants)
                .disable_preferred_strict
        );
        assert!(!wants.disable_preferred_strict);
        session.resume_prepared(magi_journal::Journal::recorded(
            SessionId::new("resumed"),
            vec![],
        ));
        assert!(
            !session
                .strict_wants("role", "p/m", &wants)
                .disable_preferred_strict
        );
    }

    #[test]
    fn unknown_and_disabled_observations_do_not_learn_or_clear_fallback() {
        let mut session = Session::recorded(SessionId::new("s"), vec![]);
        let wants = Wants::default();
        for frame in [
            RequestFrame {
                version: 2,
                ..frame()
            },
            RequestFrame {
                disable_preferred_strict: false,
                ..frame()
            },
        ] {
            session.report_provider_frame("role", "p/m", frame);
        }
        assert!(
            !session
                .strict_wants("role", "p/m", &wants)
                .disable_preferred_strict
        );
        session.report_provider_frame("role", "p/m", frame());
        session.report_provider_frame(
            "role",
            "p/m",
            RequestFrame {
                disable_preferred_strict: false,
                ..frame()
            },
        );
        assert!(
            session
                .strict_wants("role", "p/m", &wants)
                .disable_preferred_strict
        );
    }

    #[test]
    fn fallback_storage_is_bounded_deduplicated_and_evicts_the_oldest_identity() {
        let mut session = Session::recorded(SessionId::new("bounded"), vec![]);
        for n in 0..MOST {
            session.report_provider_frame("role", &format!("p/{n}"), frame());
        }
        session.report_provider_frame("role", "p/0", frame());
        assert_eq!(session.strict.0.len(), MOST);
        session.report_provider_frame("role", "p/new", frame());
        assert_eq!(session.strict.0.len(), MOST);
        let wants = Wants::default();
        assert!(
            !session
                .strict_wants("role", "p/0", &wants)
                .disable_preferred_strict
        );
        assert!(
            session
                .strict_wants("role", "p/1", &wants)
                .disable_preferred_strict
        );
        assert!(
            session
                .strict_wants("role", "p/new", &wants)
                .disable_preferred_strict
        );
    }
}

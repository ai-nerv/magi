use super::Session;
use magi_proto::HarnessEvent;

impl Session {
    pub fn set_profiles(&mut self, profiles: Vec<(String, String)>) {
        self.profiles = profiles;
    }

    pub fn set_profile(&mut self, profile: Option<String>) {
        self.profile = profile;
    }

    pub fn set_base_model(&mut self, model: Option<String>) {
        self.base_model = model;
    }

    #[must_use]
    pub fn base_model(&self) -> Option<String> {
        self.base_model.clone()
    }

    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    #[must_use]
    pub fn profile_event(&self) -> Option<HarnessEvent> {
        (!self.profiles.is_empty() || self.profile.is_some()).then(|| {
            HarnessEvent::ProfileChanged {
                cursor: self.cursor(),
                profile: self.profile.clone(),
                profiles: self.profiles.clone(),
            }
        })
    }

    pub fn announce_profile(&self) {
        let _ = self.events.send(HarnessEvent::ProfileChanged {
            cursor: self.cursor(),
            profile: self.profile.clone(),
            profiles: self.profiles.clone(),
        });
    }

    pub fn set_plugin_status(&mut self, text: Option<String>) {
        if self.plugin_status == text {
            return;
        }
        self.plugin_status.clone_from(&text);
        let _ = self.events.send(HarnessEvent::PluginStatus {
            cursor: self.cursor(),
            text,
        });
    }

    #[must_use]
    pub fn plugin_status_event(&self) -> Option<HarnessEvent> {
        self.plugin_status
            .as_ref()
            .map(|text| HarnessEvent::PluginStatus {
                cursor: self.cursor(),
                text: Some(text.clone()),
            })
    }
}

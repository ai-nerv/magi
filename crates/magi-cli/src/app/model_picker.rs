//! Model picker scoping by readiness and provider, with adaptive metadata columns.

use super::{App, Picking};
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct State {
    provider: Option<String>,
    show_locked: bool,
    refreshing: bool,
}

impl App {
    pub fn open_model_picker(&mut self) {
        let already_open = self.picking == Some(Picking::Model) && self.overlay.is_some();
        if !already_open {
            self.model_picker.show_locked = false;
            self.model_picker.provider = self.initial_model_provider();
        }
        self.rebuild_model_picker(already_open);
    }

    pub fn open_model_picker_refreshing(&mut self) {
        self.model_picker.refreshing = true;
        self.open_model_picker();
    }

    pub fn open_model_picker_for(&mut self, provider: Option<String>) {
        self.model_picker.show_locked = false;
        self.model_picker.provider = provider;
        self.model_picker.refreshing = true;
        self.rebuild_model_picker(false);
    }

    pub(super) fn refresh_model_picker(&mut self) {
        self.model_picker.refreshing = false;
        if self.picking == Some(Picking::Model) && self.overlay.is_some() {
            self.rebuild_model_picker(true);
        }
    }

    pub fn step_model_provider(&mut self, forward: bool) {
        if self.picking != Some(Picking::Model) || self.overlay.is_none() {
            return;
        }
        let providers = self.visible_model_providers();
        if providers.len() <= 1 {
            return;
        }
        let mut tabs = Vec::with_capacity(providers.len() + 1);
        tabs.push(None);
        tabs.extend(providers.into_iter().map(Some));
        let current = tabs
            .iter()
            .position(|tab| *tab == self.model_picker.provider)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % tabs.len()
        } else {
            (current + tabs.len() - 1) % tabs.len()
        };
        if let Some(target) = tabs.get(next) {
            self.model_picker.provider.clone_from(target);
        }
        self.rebuild_model_picker(true);
    }

    pub fn toggle_model_locked(&mut self) {
        if self.picking != Some(Picking::Model) || self.overlay.is_none() {
            return;
        }
        let has_ready = self.choices.iter().any(|c| c.requirement.is_empty());
        let has_locked = self.choices.iter().any(|c| !c.requirement.is_empty());
        if !has_ready || !has_locked {
            return;
        }
        self.model_picker.show_locked = !self.model_picker.show_locked;
        self.rebuild_model_picker(true);
    }

    fn initial_model_provider(&self) -> Option<String> {
        let ready_providers: BTreeSet<&str> = self
            .choices
            .iter()
            .filter(|choice| choice.requirement.is_empty())
            .map(crate::models::provider_name)
            .filter(|provider| !provider.is_empty())
            .collect();
        if ready_providers.len() <= 1 {
            return None;
        }
        let active = self.model.as_ref()?.name.as_str();
        let choice = self
            .choices
            .iter()
            .find(|choice| choice.name == active && choice.requirement.is_empty())?;
        let provider = crate::models::provider_name(choice);
        (!provider.is_empty()).then(|| provider.to_owned())
    }

    fn visible_model_providers(&self) -> Vec<String> {
        let has_ready = self.choices.iter().any(|c| c.requirement.is_empty());
        let include_locked = self.model_picker.show_locked || !has_ready;
        let providers: BTreeSet<String> = self
            .choices
            .iter()
            .filter(|choice| include_locked || choice.requirement.is_empty())
            .map(|choice| crate::models::provider_name(choice).to_owned())
            .filter(|provider| !provider.is_empty())
            .collect();
        providers.into_iter().collect()
    }

    fn rebuild_model_picker(&mut self, preserve: bool) {
        let previous = preserve
            .then(|| self.overlay.as_ref().and_then(|overlay| overlay.list()))
            .flatten();
        let query = previous
            .map(|picker| picker.query().to_owned())
            .unwrap_or_default();
        let current = previous
            .and_then(|picker| picker.current().map(|choice| choice.value.clone()))
            .or_else(|| self.model.as_ref().map(|m| m.name.clone()));

        let has_ready = self.choices.iter().any(|c| c.requirement.is_empty());
        let locked_count = self
            .choices
            .iter()
            .filter(|c| !c.requirement.is_empty())
            .count();
        let include_locked = self.model_picker.show_locked || !has_ready;
        let pool: Vec<&magi_proto::ModelChoice> = self
            .choices
            .iter()
            .filter(|choice| include_locked || choice.requirement.is_empty())
            .collect();
        let providers: Vec<String> = pool
            .iter()
            .map(|choice| crate::models::provider_name(choice).to_owned())
            .filter(|provider| !provider.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        let active_provider = self
            .model_picker
            .provider
            .as_deref()
            .filter(|wanted| providers.iter().any(|provider| provider == wanted))
            .map(ToOwned::to_owned);
        if active_provider.is_none() && !self.model_picker.refreshing {
            self.model_picker.provider = None;
        }
        let scoped: Vec<&magi_proto::ModelChoice> = match active_provider.as_deref() {
            Some(provider) => pool
                .iter()
                .copied()
                .filter(|choice| crate::models::provider_name(choice) == provider)
                .collect(),
            None => pool,
        };

        let columns = crate::models::Columns::for_choices(&scoped);
        let headers = columns.headers();
        let choices: Vec<magi_tui::picker::Choice> = scoped
            .iter()
            .map(|choice| magi_tui::picker::Choice {
                value: choice.name.clone(),
                detail: crate::models::table_cells_for(choice, columns),
                ready: choice.requirement.is_empty(),
            })
            .collect();

        let title = if self.model_picker.refreshing {
            "Model — refreshing"
        } else {
            "Model"
        };
        let mut picker = magi_tui::picker::Picker::new(title, choices, current.as_deref());
        picker.table(&headers);
        picker.captions(scoped.iter().filter_map(|choice| {
            choice
                .display_name
                .as_ref()
                .map(|name| (choice.name.clone(), name.clone()))
        }));
        picker.reasons(
            scoped
                .iter()
                .filter(|choice| !choice.requirement.is_empty())
                .map(|choice| (choice.name.clone(), choice.requirement.clone())),
        );

        let mut hints = Vec::new();
        if providers.len() > 1 {
            let mut tabs = Vec::with_capacity(providers.len() + 1);
            tabs.push("all".to_owned());
            tabs.extend(providers.iter().cloned());
            let active_tab = active_provider
                .as_deref()
                .and_then(|wanted| providers.iter().position(|provider| provider == wanted))
                .map_or(0, |index| index + 1);
            picker.tabs(tabs, active_tab);
            hints.push("←→ provider".to_owned());
        }
        if has_ready && locked_count > 0 {
            if self.model_picker.show_locked {
                hints.push("^a ready only".to_owned());
            } else {
                hints.push(format!("^a +{locked_count} locked"));
            }
        }
        if !hints.is_empty() {
            picker.hint(hints.join(" · "));
        }

        for ch in query.chars() {
            picker.push(ch);
        }
        if let Some(index) = picker
            .choices
            .iter()
            .position(|choice| Some(choice.value.as_str()) == current.as_deref())
        {
            picker.selected = index;
        }
        self.overlay = Some(picker.into());
        self.picking = Some(Picking::Model);
    }
}

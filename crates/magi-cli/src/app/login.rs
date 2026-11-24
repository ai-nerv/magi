use super::App;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use magi_proto::{
    UiCommand,
    authentication::{Account, Reply, Request},
};

#[derive(Default)]
enum Page {
    #[default]
    List,
    Account,
    Key(String),
    Remove,
    Waiting(Option<String>),
}

#[derive(Default)]
pub struct Login {
    device_code: Option<String>,
    expires_in: Option<u64>,
    accounts: Vec<Account>,
    selected: String,
    page: Page,
    notice: String,
    sequence: u64,
    pending: Option<u64>,
}

impl App {
    pub fn resize_login(&mut self, rect: Option<ratatui::layout::Rect>) {
        if self.pane_titled("login") && self.pane_rect != rect {
            let previous = self.pane.clone();
            self.pane_rect = rect;
            self.show_login();
            if let (Some(previous), Some(open)) = (previous, self.pane.as_mut()) {
                if let Some(id) = previous.chosen() {
                    open.point_at(id);
                }
                open.keep_scroll(&previous);
            }
        }
    }

    pub fn open_logins(&mut self) -> Option<UiCommand> {
        if self.view_only {
            self.refuse_view_only();
            return None;
        }
        self.overlay = None;
        self.login.selected.clear();
        Some(self.login_request(Request::List))
    }

    fn login_request(&mut self, action: Request) -> UiCommand {
        self.login.device_code = None;
        self.login.expires_in = None;
        self.login.sequence += 1;
        self.login.pending = Some(self.login.sequence);
        self.login.page = Page::Waiting(None);
        self.login.notice.clear();
        self.show_login();
        UiCommand::Authenticate {
            request: self.login.sequence,
            action,
        }
    }

    pub fn show_login(&mut self) {
        use magi_tui::login::View;
        let account = self
            .login
            .accounts
            .iter()
            .find(|a| a.id == self.login.selected);
        let view = match (&self.login.page, account) {
            (Page::Account, Some(a)) => View::Account(a),
            (Page::Key(key), Some(a)) => View::Key {
                account: a,
                filled: !key.is_empty(),
            },
            (Page::Remove, Some(a)) => View::Remove(a),
            (Page::Waiting(url), _) => View::Waiting {
                url: url.as_deref(),
                code: self.login.device_code.as_deref(),
                expires_in: self.login.expires_in,
            },
            _ => View::List(&self.login.accounts),
        };
        let width = self
            .pane_rect
            .map_or(56, |r| usize::from(r.width.saturating_sub(7)));
        let active = self.model.as_ref().map(|model| model.name.as_str());
        let models: Vec<magi_tui::login::Model> = self
            .choices
            .iter()
            .map(|choice| magi_tui::login::Model {
                id: choice.name.clone(),
                provider: crate::models::provider_name(choice).to_owned(),
                context_window: choice.context_window,
                ready: choice.requirement.is_empty(),
                subscription: choice.subscription,
                unpriced: choice.pricing.is_none(),
                active: active == Some(choice.name.as_str()),
            })
            .collect();
        self.pane = Some(magi_tui::login::pane_with_models(
            view,
            &self.login.notice,
            width,
            &models,
        ));
    }

    pub fn choose_login(&mut self, choice: &str) -> Option<UiCommand> {
        if self.view_only {
            return None;
        }
        if let Some(model) = choice.strip_prefix("model:") {
            let _ = self.close_login();
            return Some(UiCommand::SetModel {
                name: model.to_owned(),
            });
        }
        if let Some(id) = choice.strip_prefix("provider:") {
            if self.login.accounts.iter().any(|a| a.id == id) {
                self.login.selected = id.into();
                self.login.page = Page::Account;
            }
        } else {
            match choice {
                "back" => {
                    self.login.page = Page::List;
                    self.login.selected.clear();
                }
                "keep" => self.login.page = Page::Account,
                "remove" => self.login.page = Page::Remove,
                "confirm-remove" if matches!(self.login.page, Page::Remove) => {
                    return Some(self.login_request(Request::Remove {
                        provider: self.login.selected.clone(),
                    }));
                }
                "models" => {
                    let provider = self.login.selected.clone();
                    let _ = self.close_login();
                    self.open_model_picker_for((!provider.is_empty()).then_some(provider));
                    return Some(UiCommand::RefreshModels);
                }
                "add" => {
                    let account = self
                        .login
                        .accounts
                        .iter()
                        .find(|a| a.id == self.login.selected && a.available)?;
                    if account.method.starts_with("oauth") {
                        return Some(self.login_request(Request::Login {
                            provider: account.id.clone(),
                        }));
                    }
                    self.login.page = Page::Key(String::new());
                }
                _ => return None,
            }
        }
        self.login.notice.clear();
        self.show_login();
        None
    }

    pub fn login_answer(&mut self, request: u64, reply: Reply) {
        if self.login.pending != Some(request) {
            return;
        }
        match reply {
            Reply::Device {
                url,
                code,
                expires_in,
            } => {
                self.login.page = Page::Waiting(Some(url));
                self.login.device_code = Some(code);
                self.login.expires_in = Some(expires_in);
                self.login.notice = "Approve this code at the provider's sign-in page.".into();
            }
            Reply::Browser { url } => {
                self.login.device_code = None;
                self.login.expires_in = None;
                self.login.page = Page::Waiting(Some(url));
                self.login.notice = "Press o to open the browser, or c to copy the link.".into();
            }
            Reply::Finished { providers, error } => {
                self.login.device_code = None;
                self.login.expires_in = None;
                self.login.pending = None;
                if error.is_none() || !providers.is_empty() {
                    self.login.accounts = providers;
                }
                self.login.page = if self.login.selected.is_empty() {
                    Page::List
                } else {
                    Page::Account
                };
                self.login.notice = error.unwrap_or_default();
            }
        }
        if self.pane_titled("login") {
            self.show_login();
        }
    }

    pub fn login_key(&mut self, key: KeyEvent) -> Option<Option<UiCommand>> {
        if !self.pane_titled("login") && !matches!(self.login.page, Page::Key(_)) {
            return None;
        }
        if key.kind == KeyEventKind::Release {
            return Some(None);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc || (ctrl && key.code == KeyCode::Char('c')) {
            return Some(self.close_login());
        }
        if let Page::Key(value) = &mut self.login.page {
            match key.code {
                KeyCode::Enter if !value.is_empty() => {
                    let key = std::mem::take(value);
                    let provider = self.login.selected.clone();
                    return Some(Some(self.login_request(Request::SetKey { provider, key })));
                }
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char('u') if ctrl => value.clear(),
                KeyCode::Char(c)
                    if !ctrl
                        && !key.modifiers.contains(KeyModifiers::ALT)
                        && c.is_ascii_graphic()
                        && value.len() < 8192 =>
                {
                    value.push(c)
                }
                _ => {}
            }
            self.show_login();
            return Some(None);
        }
        if let Page::Waiting(url) = &self.login.page {
            if let Some(url) = url {
                match key.code {
                    KeyCode::Char('o') => open_browser(url, &self.login.accounts),
                    KeyCode::Char('c') if !ctrl => {
                        crate::clipboard::put(self.login.device_code.as_deref().unwrap_or(url))
                    }
                    KeyCode::Char('u') if !ctrl => crate::clipboard::put(url),
                    _ => {}
                }
            }
            return Some(None);
        }
        if key.code == KeyCode::Char('r') {
            return Some(Some(self.login_request(Request::List)));
        }
        if key.code == KeyCode::Char('q') {
            return Some(self.close_login());
        }
        None
    }

    pub fn login_paste(&mut self, text: &str) -> bool {
        if !self.pane_titled("login") && !matches!(self.login.page, Page::Key(_)) {
            return false;
        }
        if let Page::Key(key) = &mut self.login.page {
            let text = text.trim_end_matches(['\r', '\n']);
            if key.len() + text.len() <= 8192 && text.bytes().all(|b| b.is_ascii_graphic()) {
                key.push_str(text);
                self.login.notice.clear();
            } else {
                self.login.notice =
                    "Key must be at most 8192 characters, without spaces or control characters."
                        .into();
            }
            self.show_login();
        }
        true
    }

    pub fn close_login(&mut self) -> Option<UiCommand> {
        self.login.device_code = None;
        self.login.expires_in = None;
        self.login.page = Page::List;
        self.pane = None;
        self.login
            .pending
            .take()
            .map(|request| UiCommand::CancelAuthentication { request })
    }

    pub fn login_cleanup(&mut self) -> Option<UiCommand> {
        if matches!(self.login.page, Page::Key(_)) {
            self.show_login();
            return None;
        }
        self.login.page = Page::List;
        self.login.device_code = None;
        self.login.expires_in = None;
        self.login
            .pending
            .take()
            .map(|request| UiCommand::CancelAuthentication { request })
    }
}

fn open_browser(url: &str, accounts: &[Account]) {
    let mut command = std::process::Command::new("xdg-open");
    magi_tools::environ::without_capability(&mut command);
    for name in accounts.iter().flat_map(|account| &account.variables) {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(|s| {
            ["MAGI_", "MELCHIOR_", "CASPER_", "BALTHASAR_", "NERV_"]
                .iter()
                .any(|p| s.starts_with(p))
                || ["_KEY", "_TOKEN", "_SECRET"].iter().any(|p| s.ends_with(p))
        }) {
            command.env_remove(name);
        }
    }
    if let Ok(mut child) = command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests;

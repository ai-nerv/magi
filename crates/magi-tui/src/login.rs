//! Provider-login floats; credential values never enter the renderer.

use crate::{colour, footer::format_tokens, pane::Pane};
use magi_proto::authentication::Account;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

const LISTED_MODELS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub provider: String,
    pub context_window: u64,
    pub ready: bool,
    pub subscription: bool,
    pub unpriced: bool,
    pub active: bool,
}

pub enum View<'a> {
    List(&'a [Account]),
    Account(&'a Account),
    Key {
        account: &'a Account,
        filled: bool,
    },
    Remove(&'a Account),
    Waiting {
        url: Option<&'a str>,
        code: Option<&'a str>,
        expires_in: Option<u64>,
    },
}

pub fn pane(view: View<'_>, notice: &str, width: usize) -> Pane {
    pane_with_models(view, notice, width, &[])
}

struct Builder {
    rows: Vec<Line<'static>>,
    picks: Vec<Option<String>>,
    rule: String,
    rule_style: Style,
    heading_style: Style,
    dim: Style,
}

impl Builder {
    fn new(width: usize, dim: Style) -> Self {
        Self {
            rows: Vec::new(),
            picks: Vec::new(),
            rule: "- ".repeat(width.max(20) / 2).trim_end().to_owned(),
            rule_style: Style::default().fg(colour::border()),
            heading_style: Style::default()
                .fg(colour::text())
                .add_modifier(Modifier::BOLD),
            dim,
        }
    }

    fn add(&mut self, line: Line<'static>, id: Option<String>) {
        self.rows.push(line);
        self.picks.push(id);
    }

    fn section(&mut self, title: &str, note: &str) {
        self.add(Line::default(), None);
        self.add(Line::styled(self.rule.clone(), self.rule_style), None);
        let mut spans = vec![Span::styled(title.to_owned(), self.heading_style)];
        if !note.is_empty() {
            spans.push(Span::styled(format!("  {note}"), self.dim));
        }
        self.add(Line::from(spans), None);
    }

    fn group_heading(&mut self, title: &str, count: usize, first: bool) {
        if !first {
            self.add(Line::styled(self.rule.clone(), self.rule_style), None);
        }
        self.add(
            Line::from(vec![
                Span::styled(title.to_owned(), self.heading_style),
                Span::styled(format!("  {count}"), self.dim),
            ]),
            None,
        );
    }
}

pub fn pane_with_models(view: View<'_>, notice: &str, width: usize, models: &[Model]) -> Pane {
    let dim = Style::default().fg(colour::dim());
    let text = Style::default().fg(colour::text());
    let bright = Style::default()
        .fg(colour::accent())
        .add_modifier(Modifier::BOLD);
    let active_dot = Style::default().fg(colour::success());
    let mut out = Builder::new(width, dim);

    out.add(Line::styled("PROVIDER ACCOUNTS", bright), None);
    if !notice.is_empty() {
        for line in wrap(notice, width) {
            out.add(
                Line::styled(line, Style::default().fg(colour::warning())),
                None,
            );
        }
    }
    match view {
        View::List(accounts) => {
            out.add(
                Line::styled("↑↓ select · enter manage · r refresh · esc close", dim),
                None,
            );
            out.add(Line::default(), None);
            if accounts.is_empty() {
                out.add(
                    Line::from("No provider accounts loaded. Press r to retry."),
                    None,
                );
            } else {
                let mut signed_in = Vec::new();
                let mut plans = Vec::new();
                let mut metered = Vec::new();
                for account in accounts {
                    if account.source != "none" {
                        signed_in.push(account);
                    } else if is_plan(account, models) {
                        plans.push(account);
                    } else {
                        metered.push(account);
                    }
                }
                let groups: [(&str, Vec<&Account>); 3] = [
                    ("Active", signed_in),
                    ("Subscriptions & plans", plans),
                    ("API keys", metered),
                ];
                let palette = Palette {
                    active_dot,
                    bright,
                    text,
                    dim,
                };
                let mut shown_any = false;
                for (title, group) in groups {
                    if group.is_empty() {
                        continue;
                    }
                    out.group_heading(title, group.len(), !shown_any);
                    shown_any = true;
                    for account in group {
                        push_account_row(&mut out.rows, &mut out.picks, account, models, palette);
                    }
                }
            }
        }
        View::Account(account) => {
            let signed_in = account.source != "none";
            let provider_models: Vec<&Model> = models
                .iter()
                .filter(|model| model.provider == account.id)
                .collect();
            let kind = kind_label(account, models);
            out.add(Line::default(), None);
            out.add(
                Line::from(vec![
                    Span::styled(
                        if signed_in { "● " } else { "○ " },
                        if signed_in { active_dot } else { dim },
                    ),
                    Span::styled(account.name.clone(), bright),
                    Span::styled(format!("  {kind}"), dim),
                ]),
                None,
            );
            out.add(Line::styled(format!("  {}", account.status), dim), None);
            if account.source == "environment" {
                out.add(
                    Line::styled(
                        "  Environment key takes priority over saved credentials.",
                        dim,
                    ),
                    None,
                );
            }
            if !account.variables.is_empty() {
                for line in wrap(&account.variables.join(" or "), width.saturating_sub(2)) {
                    out.add(Line::styled(format!("  {line}"), dim), None);
                }
            }
            out.section("Actions", "");
            if signed_in {
                let label = if provider_models.is_empty() {
                    "→  Open model picker".to_owned()
                } else {
                    format!("→  Open model picker ({} models)", provider_models.len())
                };
                out.add(Line::styled(label, bright), Some("models".into()));
            }
            if account.available {
                let label = if account.method.starts_with("oauth") {
                    "Open browser to sign in"
                } else if account.saved {
                    "Replace saved API key"
                } else {
                    "Add API key"
                };
                out.add(
                    Line::styled(format!("+  {label}"), if signed_in { text } else { bright }),
                    Some("add".into()),
                );
            } else {
                out.add(
                    Line::styled("Authentication is not configured for this provider.", dim),
                    None,
                );
            }
            if account.saved {
                out.add(Line::from("−  Remove saved login…"), Some("remove".into()));
            }
            out.add(Line::from("←  All providers"), Some("back".into()));

            if !provider_models.is_empty() {
                let ready_count = provider_models.iter().filter(|model| model.ready).count();
                let note = if ready_count > 0 {
                    format!("{ready_count} ready · ⏎ to select")
                } else {
                    format!("{} offered · sign in to use", provider_models.len())
                };
                out.section("Models", &note);
                for model in provider_models.iter().take(LISTED_MODELS) {
                    let short = model
                        .id
                        .strip_prefix(&format!("{}/", account.id))
                        .unwrap_or(&model.id);
                    let ctx = if model.context_window > 0 {
                        format_tokens(model.context_window)
                    } else {
                        "—".to_owned()
                    };
                    let marker = if model.active {
                        Span::styled("◉ ", bright)
                    } else if model.ready {
                        Span::styled("○ ", dim)
                    } else {
                        Span::styled("· ", dim)
                    };
                    let name_style = if model.active {
                        bright
                    } else if model.ready {
                        text
                    } else {
                        dim
                    };
                    let pick = model.ready.then(|| format!("model:{}", model.id));
                    out.add(
                        Line::from(vec![
                            marker,
                            Span::styled(short.to_owned(), name_style),
                            Span::styled(format!("  {ctx}"), dim),
                        ]),
                        pick,
                    );
                }
                if provider_models.len() > LISTED_MODELS {
                    out.add(
                        Line::styled(
                            format!(
                                "  … {} more — open model picker to search",
                                provider_models.len() - LISTED_MODELS
                            ),
                            dim,
                        ),
                        None,
                    );
                }
            }
        }
        View::Key { account, filled } => {
            out.add(Line::default(), None);
            out.add(
                Line::styled(format!("API key · {}", account.name), bright),
                None,
            );
            out.add(
                Line::from(if filled {
                    "  ●●●●●●●●●●●●"
                } else {
                    "  Paste or type your API key…"
                }),
                None,
            );
            out.add(Line::default(), None);
            out.add(
                Line::styled("enter save · ctrl+u clear · esc cancel", dim),
                None,
            );
            out.add(
                Line::styled("Hidden input. Never sent to chat or command history.", dim),
                None,
            );
            out.add(
                Line::styled("Saved privately by Melchior, not encrypted at rest.", dim),
                None,
            );
        }
        View::Remove(account) => {
            out.add(Line::default(), None);
            out.add(
                Line::styled(format!("Remove saved login for {}?", account.name), bright),
                None,
            );
            out.add(
                Line::styled("Local removal only; no provider-side revocation.", dim),
                None,
            );
            out.add(
                Line::styled("Environment keys remain available.", dim),
                None,
            );
            out.add(Line::default(), None);
            out.add(Line::from("Keep login"), Some("keep".into()));
            out.add(
                Line::styled("Remove saved login", Style::default().fg(colour::error())),
                Some("confirm-remove".into()),
            );
        }
        View::Waiting {
            url,
            code,
            expires_in,
        } => {
            out.add(Line::default(), None);
            out.add(
                Line::styled(
                    if url.is_some() {
                        "Finish sign-in in your browser"
                    } else {
                        "Contacting Melchior…"
                    },
                    bright,
                ),
                None,
            );
            out.add(
                Line::styled(
                    format!(
                        "esc cancel · expires within {} seconds",
                        expires_in.unwrap_or(300)
                    ),
                    dim,
                ),
                None,
            );
            if let Some(url) = url {
                if let Some(code) = code {
                    out.add(Line::styled(format!("Your code:  {code}"), bright), None);
                    out.add(
                        Line::from("o open browser · c copy code · u copy link"),
                        None,
                    );
                } else {
                    out.add(Line::from("o open browser · c copy link"), None);
                }
                for line in wrap(url, width) {
                    out.add(Line::styled(line, dim), None);
                }
            }
        }
    }
    let mut pane = Pane::new("login", out.rows)
        .selectable(out.picks)
        .tinted(Some(colour::accent()));
    pane.first();
    pane
}

#[derive(Clone, Copy)]
struct Palette {
    active_dot: Style,
    bright: Style,
    text: Style,
    dim: Style,
}

fn push_account_row(
    rows: &mut Vec<Line<'static>>,
    picks: &mut Vec<Option<String>>,
    account: &Account,
    models: &[Model],
    palette: Palette,
) {
    let signed_in = account.source != "none";
    let id = Some(format!("provider:{}", account.id));
    let count = models
        .iter()
        .filter(|model| model.provider == account.id)
        .count();
    let has_active_model = models
        .iter()
        .any(|model| model.provider == account.id && model.active);
    let kind = kind_label(account, models);
    let mut first = vec![
        Span::styled(
            if signed_in { "● " } else { "○ " },
            if signed_in {
                palette.active_dot
            } else {
                palette.dim
            },
        ),
        Span::styled(
            account.name.clone(),
            if signed_in {
                palette.bright
            } else {
                palette.text
            },
        ),
        Span::styled(format!("  {kind}"), palette.dim),
    ];
    if count > 0 {
        let suffix = if count == 1 { "model" } else { "models" };
        first.push(Span::styled(format!(" · {count} {suffix}"), palette.dim));
    }
    if signed_in {
        first.push(Span::styled(format!(" · {}", account.status), palette.dim));
    }
    if has_active_model {
        first.push(Span::styled(" · in use", palette.active_dot));
    }
    rows.push(Line::from(first));
    picks.push(id);
}

fn is_plan(account: &Account, models: &[Model]) -> bool {
    if account.method.starts_with("oauth")
        || account.id.contains("token-plan")
        || account.id == "github-copilot"
    {
        return true;
    }
    let mut matched = models
        .iter()
        .filter(|model| model.provider == account.id)
        .peekable();
    matched.peek().is_some() && matched.all(|model| model.subscription || model.unpriced)
}

fn kind_label(account: &Account, models: &[Model]) -> &'static str {
    if account.method.starts_with("oauth") {
        "subscription"
    } else if account.id.contains("token-plan") {
        "token plan"
    } else if is_plan(account, models) {
        "plan"
    } else {
        "api-key"
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    text.chars()
        .collect::<Vec<_>>()
        .chunks(width.max(1))
        .map(|chunk| chunk.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_defaults_to_keep_and_rows_pair_with_selection() {
        let account = Account {
            id: "mistral".into(),
            name: "Mistral".into(),
            method: "api-key".into(),
            status: "Environment key".into(),
            source: "environment".into(),
            saved: true,
            available: true,
            variables: vec!["MISTRAL_API_KEY".into()],
        };
        let confirm = pane(View::Remove(&account), "", 50);
        assert_eq!(confirm.chosen(), Some("keep"));
        assert_eq!(confirm.rows.len(), confirm.picks.len());
        let card = pane(View::Account(&account), "", 50);
        assert!(
            card.rows
                .iter()
                .any(|r| r.to_string().contains("Environment key takes priority"))
        );
        let mask = pane(
            View::Key {
                account: &account,
                filled: true,
            },
            "",
            50,
        );
        assert!(mask.rows.iter().any(|r| r.to_string().contains("●●●●")));
    }

    #[test]
    fn list_groups_active_subscriptions_and_api_keys_and_account_lists_models() {
        let accounts = vec![
            Account {
                id: "openrouter".into(),
                name: "OpenRouter".into(),
                method: "api-key".into(),
                status: "Not signed in".into(),
                source: "none".into(),
                saved: false,
                available: true,
                variables: vec!["OPENROUTER_API_KEY".into()],
            },
            Account {
                id: "qwen-token-plan".into(),
                name: "Qwen Token Plan".into(),
                method: "api-key".into(),
                status: "Saved API key".into(),
                source: "saved".into(),
                saved: true,
                available: true,
                variables: vec!["QWEN_TOKEN_PLAN_API_KEY".into()],
            },
            Account {
                id: "github-copilot".into(),
                name: "GitHub Copilot".into(),
                method: "oauth".into(),
                status: "Not signed in".into(),
                source: "none".into(),
                saved: false,
                available: true,
                variables: Vec::new(),
            },
        ];
        let models = vec![Model {
            id: "qwen-token-plan/qwen3-coder".into(),
            provider: "qwen-token-plan".into(),
            context_window: 200_000,
            ready: true,
            subscription: false,
            unpriced: true,
            active: true,
        }];
        let list = pane_with_models(View::List(&accounts), "", 70, &models);
        let text: Vec<String> = list.rows.iter().map(ToString::to_string).collect();
        let active_pos = text
            .iter()
            .position(|row| row.starts_with("Active"))
            .expect("active");
        let plans_pos = text
            .iter()
            .position(|row| row.starts_with("Subscriptions & plans"))
            .expect("plans");
        let api_pos = text
            .iter()
            .position(|row| row.starts_with("API keys"))
            .expect("api");
        assert!(active_pos < plans_pos && plans_pos < api_pos, "{text:?}");
        assert!(
            text.iter()
                .any(|row| row.contains("1 model") && row.contains("in use"))
        );

        let detail = pane_with_models(View::Account(&accounts[1]), "", 70, &models);
        assert!(
            detail
                .picks
                .iter()
                .flatten()
                .any(|pick| pick == "model:qwen-token-plan/qwen3-coder")
        );
        assert!(
            detail
                .rows
                .iter()
                .any(|row| row.to_string().contains("qwen3-coder"))
        );
    }
}

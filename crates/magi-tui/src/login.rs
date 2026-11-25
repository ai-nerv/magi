//! Provider-login floats; credential values never enter the renderer.

use crate::{colour, footer::format_tokens, pane::Pane};
use magi_proto::authentication::Account;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

const LISTED_MODELS: usize = 20;

/// Filter tabs across the `:login` list float heading.
pub const TABS: [&str; 3] = ["all", "plans", "api keys"];

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

#[must_use]
pub fn pane(view: View<'_>, notice: &str, width: usize) -> Pane {
    pane_tabbed(view, notice, width, &[], 0)
}

#[must_use]
pub fn pane_with_models(view: View<'_>, notice: &str, width: usize, models: &[Model]) -> Pane {
    pane_tabbed(view, notice, width, models, 0)
}

struct Builder {
    rows: Vec<Line<'static>>,
    picks: Vec<Option<String>>,
    dim: Style,
    heading: Style,
}

impl Builder {
    fn new(dim: Style) -> Self {
        Self {
            rows: Vec::new(),
            picks: Vec::new(),
            dim,
            heading: Style::default()
                .fg(colour::hint())
                .add_modifier(Modifier::BOLD),
        }
    }

    fn add(&mut self, line: Line<'static>, id: Option<String>) {
        self.rows.push(line);
        self.picks.push(id);
    }

    fn blank(&mut self) {
        self.add(Line::default(), None);
    }

    fn section(&mut self, title: &str, note: &str, first: bool) {
        if !first {
            self.blank();
        }
        let mut spans = vec![Span::styled(title.to_owned(), self.heading)];
        if !note.is_empty() {
            spans.push(Span::styled(format!("  {note}"), self.dim));
        }
        self.add(Line::from(spans), None);
    }

    fn fact(&mut self, label: &str, value: impl Into<String>, style: Style) {
        self.add(
            Line::from(vec![
                Span::styled(format!("{label:<11}"), self.dim),
                Span::styled(value.into(), style),
            ]),
            None,
        );
    }

    fn callout(&mut self, text: impl Into<String>, style: Style, width: usize) {
        let bg = colour::tool_bg();
        let bar = Style::default().fg(colour::accent()).bg(bg);
        let body = text.into();
        let inner = width.saturating_sub(4).max(8);
        let pad = inner.saturating_sub(body.chars().count());
        self.add(
            Line::from(vec![
                Span::styled("┃ ", bar),
                Span::styled(body, style.bg(bg)),
                Span::styled(" ".repeat(pad), Style::default().bg(bg)),
            ]),
            None,
        );
    }
}

#[must_use]
pub fn pane_tabbed(
    view: View<'_>,
    notice: &str,
    width: usize,
    models: &[Model],
    tab: usize,
) -> Pane {
    let dim = Style::default().fg(colour::dim());
    let text = Style::default().fg(colour::text());
    let bright = Style::default()
        .fg(colour::accent())
        .add_modifier(Modifier::BOLD);
    let active_dot = Style::default().fg(colour::success());
    let palette = Palette {
        active_dot,
        bright,
        text,
        dim,
    };
    let mut out = Builder::new(dim);

    if !notice.is_empty() {
        for line in wrap(notice, width) {
            out.add(
                Line::styled(line, Style::default().fg(colour::warning())),
                None,
            );
        }
        out.blank();
    }

    let tabbed = matches!(view, View::List(_));
    match view {
        View::List(accounts) => render_list(&mut out, accounts, models, tab, width, palette),
        View::Account(account) => render_account(&mut out, account, models, width, palette),
        View::Key { account, filled } => {
            out.add(
                Line::from(vec![
                    Span::styled("API key  ", dim),
                    Span::styled(account.name.clone(), bright),
                ]),
                None,
            );
            out.blank();
            let field = if filled {
                "●●●●●●●●●●●●"
            } else {
                "Paste or type your API key…"
            };
            out.callout(field, if filled { text } else { dim }, width);
            out.blank();
            out.add(
                Line::styled("enter save · ctrl+u clear · esc cancel", dim),
                None,
            );
            out.add(
                Line::styled(
                    "Hidden input; saved privately by Melchior and never sent to chat.",
                    dim,
                ),
                None,
            );
        }
        View::Remove(account) => {
            out.add(
                Line::styled(format!("Remove saved login for {}?", account.name), bright),
                None,
            );
            out.add(
                Line::styled(
                    "Local removal only; environment keys remain active if set.",
                    dim,
                ),
                None,
            );
            out.blank();
            out.add(Line::from("  Keep login"), Some("keep".into()));
            out.add(
                Line::styled("  Remove saved login", Style::default().fg(colour::error())),
                Some("confirm-remove".into()),
            );
        }
        View::Waiting {
            url,
            code,
            expires_in,
        } => {
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
                    format!("esc cancel · expires within {}s", expires_in.unwrap_or(300)),
                    dim,
                ),
                None,
            );
            if let Some(url) = url {
                out.blank();
                if let Some(code) = code {
                    out.callout(code, bright, width);
                    out.blank();
                    out.add(
                        Line::from("o open browser · c copy code · u copy link"),
                        None,
                    );
                } else {
                    out.add(Line::from("o open browser · c copy link"), None);
                }
                out.blank();
                for line in wrap(url, width) {
                    out.add(Line::styled(line, dim), None);
                }
            }
        }
    }

    let mut pane = Pane::new("login", out.rows)
        .selectable(out.picks)
        .tinted(Some(colour::accent()));
    if tabbed {
        pane = pane.tabbed(TABS.iter().map(|name| (*name).to_owned()).collect(), tab);
    }
    pane.first();
    pane
}

fn render_list(
    out: &mut Builder,
    accounts: &[Account],
    models: &[Model],
    tab: usize,
    width: usize,
    palette: Palette,
) {
    let has_openrouter = accounts.iter().any(|account| account.id == "openrouter");
    let visible: Vec<&Account> = accounts
        .iter()
        .filter(|account| !(has_openrouter && account.id == "decisions"))
        .collect();

    if visible.is_empty() {
        out.add(
            Line::from("No provider accounts loaded. Press r to retry."),
            None,
        );
        return;
    }

    let mut signed_in = Vec::new();
    let mut plans = Vec::new();
    let mut metered = Vec::new();
    for account in visible {
        if account.source != "none" {
            signed_in.push(account);
        } else if is_plan(account, models) {
            plans.push(account);
        } else {
            metered.push(account);
        }
    }

    let active_plans: Vec<&Account> = signed_in
        .iter()
        .copied()
        .filter(|account| is_plan(account, models))
        .collect();
    let active_keys: Vec<&Account> = signed_in
        .iter()
        .copied()
        .filter(|account| !is_plan(account, models))
        .collect();

    let groups: Vec<(&str, Vec<&Account>)> = match TABS.get(tab).copied() {
        Some("plans") => vec![("Active", active_plans), ("Subscriptions & plans", plans)],
        Some("api keys") => vec![("Active", active_keys), ("API keys", metered)],
        _ => vec![
            ("Active", signed_in),
            ("Subscriptions & plans", plans),
            ("API keys", metered),
        ],
    };

    out.add(
        Line::styled(
            "↑↓ select · ⏎ manage · tab filter · r refresh · esc close",
            palette.dim,
        ),
        None,
    );

    let layout = Columns::for_accounts(&groups, width);
    let mut shown_any = false;
    for (title, group) in groups {
        if group.is_empty() {
            continue;
        }
        out.section(title, &group.len().to_string(), !shown_any);
        shown_any = true;
        for account in group {
            push_account_row(
                &mut out.rows,
                &mut out.picks,
                account,
                models,
                layout,
                palette,
            );
        }
    }
    if !shown_any {
        out.blank();
        out.add(
            Line::styled("No accounts in this category.", palette.dim),
            None,
        );
    }
}

fn render_account(
    out: &mut Builder,
    account: &Account,
    models: &[Model],
    width: usize,
    palette: Palette,
) {
    let signed_in = account.source != "none";
    let provider_models: Vec<&Model> = models
        .iter()
        .filter(|model| model.provider == account.id)
        .collect();
    let kind = kind_label(account, models);

    out.add(
        Line::from(vec![
            Span::styled(
                if signed_in { "● " } else { "○ " },
                if signed_in {
                    palette.active_dot
                } else {
                    palette.dim
                },
            ),
            Span::styled(account.name.clone(), palette.bright),
            Span::styled(format!("  {kind}"), palette.dim),
        ]),
        None,
    );
    out.blank();
    out.fact(
        "Status",
        account.status.clone(),
        if signed_in {
            palette.active_dot
        } else {
            palette.text
        },
    );
    if account.source == "environment" {
        out.fact(
            "Priority",
            "Environment key overrides saved credentials",
            palette.dim,
        );
    }
    if !account.variables.is_empty() {
        let joined = account.variables.join(" or ");
        for (index, line) in wrap(&joined, width.saturating_sub(11).max(12))
            .into_iter()
            .enumerate()
        {
            out.fact(if index == 0 { "Env var" } else { "" }, line, palette.dim);
        }
    }

    out.section("Actions", "", false);
    if signed_in {
        let label = if provider_models.is_empty() {
            "▸  Open model picker".to_owned()
        } else {
            format!("▸  Open model picker ({} models)", provider_models.len())
        };
        out.add(Line::styled(label, palette.bright), Some("models".into()));
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
            Line::styled(
                format!("+  {label}"),
                if signed_in {
                    palette.text
                } else {
                    palette.bright
                },
            ),
            Some("add".into()),
        );
    } else {
        out.add(
            Line::styled(
                "Authentication is not configured for this provider.",
                palette.dim,
            ),
            None,
        );
    }
    if account.saved {
        out.add(
            Line::styled(
                "−  Remove saved login…",
                Style::default().fg(colour::error()),
            ),
            Some("remove".into()),
        );
    }
    out.add(
        Line::styled("←  All accounts", palette.dim),
        Some("back".into()),
    );

    if !provider_models.is_empty() {
        let ready_count = provider_models.iter().filter(|model| model.ready).count();
        let note = if ready_count > 0 {
            format!("{ready_count} ready · ⏎ select")
        } else {
            format!("{} offered · sign in to use", provider_models.len())
        };
        out.section("Models", &note, false);
        let name_col = width.saturating_sub(12).clamp(16, 42);
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
                Span::styled("◉ ", palette.active_dot)
            } else if model.ready {
                Span::styled("○ ", palette.dim)
            } else {
                Span::styled("· ", palette.dim)
            };
            let name_style = if model.active {
                palette.bright
            } else if model.ready {
                palette.text
            } else {
                palette.dim
            };
            let pick = model.ready.then(|| format!("model:{}", model.id));
            out.add(
                Line::from(vec![
                    marker,
                    Span::styled(format!("{:<name_col$}", fit(short, name_col)), name_style),
                    Span::styled(format!("{ctx:>6}"), palette.dim),
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
                    palette.dim,
                ),
                None,
            );
        }
    }
}

#[derive(Clone, Copy)]
struct Palette {
    active_dot: Style,
    bright: Style,
    text: Style,
    dim: Style,
}

#[derive(Clone, Copy)]
struct Columns {
    name: usize,
    show_kind: bool,
    show_models: bool,
    show_status: bool,
}

impl Columns {
    fn for_accounts(groups: &[(&str, Vec<&Account>)], width: usize) -> Self {
        let longest = groups
            .iter()
            .flat_map(|(_, accounts)| accounts.iter())
            .map(|account| account.name.chars().count())
            .max()
            .unwrap_or(14);
        let show_kind = width >= 44;
        let show_models = width >= 56;
        let show_status = width >= 72;
        let reserved = 2
            + if show_kind { 12 } else { 0 }
            + if show_models { 11 } else { 0 }
            + if show_status { 16 } else { 0 };
        let max_name = width.saturating_sub(reserved).clamp(12, 30);
        Self {
            name: longest.clamp(12, max_name),
            show_kind,
            show_models,
            show_status,
        }
    }
}

fn push_account_row(
    rows: &mut Vec<Line<'static>>,
    picks: &mut Vec<Option<String>>,
    account: &Account,
    models: &[Model],
    layout: Columns,
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
    let name_col = layout.name;

    let mut spans = vec![
        Span::styled(
            if signed_in { "● " } else { "○ " },
            if signed_in {
                palette.active_dot
            } else {
                palette.dim
            },
        ),
        Span::styled(
            format!("{:<name_col$}", fit(&account.name, name_col)),
            if signed_in {
                palette.bright
            } else {
                palette.text
            },
        ),
    ];

    if layout.show_kind {
        spans.push(Span::styled(format!("  {kind:<10}"), palette.dim));
    }
    if layout.show_models {
        let label = if count == 0 {
            "—".to_owned()
        } else if count == 1 {
            "1 model".to_owned()
        } else {
            format!("{count} models")
        };
        spans.push(Span::styled(format!("  {label:>9}"), palette.dim));
    }
    if has_active_model {
        spans.push(Span::styled("  in use", palette.active_dot));
    } else if layout.show_status && signed_in {
        spans.push(Span::styled(
            format!("  {}", short_status(&account.status)),
            palette.dim,
        ));
    }
    rows.push(Line::from(spans));
    picks.push(id);
}

fn short_status(status: &str) -> &str {
    match status {
        "Saved API key" => "saved key",
        "Environment key" => "env key",
        other => other,
    }
}

fn fit(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", text.chars().take(keep).collect::<String>())
}

fn is_plan(account: &Account, models: &[Model]) -> bool {
    account.method.starts_with("oauth")
        || account.id.contains("token-plan")
        || account.id == "github-copilot"
        || models
            .iter()
            .any(|model| model.provider == account.id && model.subscription)
}

fn kind_label(account: &Account, models: &[Model]) -> &'static str {
    if account.method.starts_with("oauth") {
        "oauth"
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
                .any(|r| r.to_string().contains("Environment key overrides"))
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
    fn list_groups_active_plans_and_keys_hides_decisions_alias_and_supports_tabs() {
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
                id: "decisions".into(),
                name: "Decisions".into(),
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
        let list = pane_tabbed(View::List(&accounts), "", 72, &models, 0);
        assert_eq!(list.tabs, vec!["all", "plans", "api keys"]);
        let text: Vec<String> = list.rows.iter().map(ToString::to_string).collect();
        assert!(
            !text.iter().any(|row| row.contains("Decisions")),
            "decisions is part of openrouter: {text:?}"
        );
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

        let plans_only = pane_tabbed(View::List(&accounts), "", 72, &models, 1);
        let plans_text: Vec<String> = plans_only.rows.iter().map(ToString::to_string).collect();
        assert!(plans_text.iter().any(|r| r.contains("GitHub Copilot")));
        assert!(!plans_text.iter().any(|r| r.contains("OpenRouter")));

        let detail = pane_with_models(View::Account(&accounts[2]), "", 72, &models);
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

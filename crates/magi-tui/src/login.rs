//! Provider-login floats; credential values never enter the renderer.

use crate::{colour, pane::Pane};
use magi_proto::authentication::Account;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

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
    let mut rows = Vec::new();
    let mut picks = Vec::new();
    let dim = Style::default().fg(colour::dim());
    let bright = Style::default()
        .fg(colour::accent())
        .add_modifier(Modifier::BOLD);
    let mut add = |line: Line<'static>, id: Option<String>| {
        rows.push(line);
        picks.push(id);
    };
    add(Line::styled("PROVIDER ACCOUNTS", bright), None);
    add(Line::styled("Authentication by Melchior", dim), None);
    if !notice.is_empty() {
        for line in wrap(notice, width) {
            add(
                Line::styled(line, Style::default().fg(colour::warning())),
                None,
            );
        }
    }
    add(Line::default(), None);
    match view {
        View::List(accounts) => {
            add(
                Line::styled("↑↓ select · enter manage · r refresh · esc close", dim),
                None,
            );
            for account in accounts {
                let id = Some(format!("provider:{}", account.id));
                let badge = if account.source != "none" {
                    "●"
                } else {
                    "○"
                };
                add(
                    Line::from(vec![
                        Span::styled(format!("{badge}  {}", account.name), bright),
                        Span::styled(format!("  {}", account.method), dim),
                    ]),
                    id.clone(),
                );
                add(
                    Line::styled(format!("   {} · {}", account.id, account.status), dim),
                    id,
                );
            }
            if accounts.is_empty() {
                add(
                    Line::from("No provider accounts loaded. Press r to retry."),
                    None,
                );
            }
        }
        View::Account(account) => {
            add(Line::styled(account.name.clone(), bright), None);
            add(
                Line::from(format!("{} · {}", account.method, account.status)),
                None,
            );
            if account.source == "environment" {
                add(
                    Line::styled(
                        "Environment key takes priority over saved credentials.",
                        dim,
                    ),
                    None,
                );
            }
            for line in wrap(&account.variables.join(" or "), width) {
                add(Line::styled(line, dim), None);
            }
            add(Line::default(), None);
            if account.available {
                let label = if account.method.starts_with("oauth") {
                    "Open browser to sign in"
                } else if account.saved {
                    "Replace saved API key"
                } else {
                    "Add API key"
                };
                add(
                    Line::styled(format!("+  {label}"), bright),
                    Some("add".into()),
                );
            } else {
                add(
                    Line::styled("Authentication is not configured for this provider.", dim),
                    None,
                );
            }
            if account.saved {
                add(Line::from("−  Remove saved login…"), Some("remove".into()));
            }
            add(Line::from("←  All providers"), Some("back".into()));
        }
        View::Key { account, filled } => {
            add(
                Line::styled(format!("API key · {}", account.name), bright),
                None,
            );
            add(
                Line::from(if filled {
                    "  ●●●●●●●●●●●●"
                } else {
                    "  Paste or type your API key…"
                }),
                None,
            );
            add(Line::default(), None);
            add(
                Line::styled("enter save · ctrl+u clear · esc cancel", dim),
                None,
            );
            add(
                Line::styled("Hidden input. Never sent to chat or command history.", dim),
                None,
            );
            add(
                Line::styled("Saved privately by Melchior, not encrypted at rest.", dim),
                None,
            );
        }
        View::Remove(account) => {
            add(
                Line::styled(format!("Remove saved login for {}?", account.name), bright),
                None,
            );
            add(
                Line::styled("Local removal only; no provider-side revocation.", dim),
                None,
            );
            add(
                Line::styled("Environment keys remain available.", dim),
                None,
            );
            add(Line::default(), None);
            add(Line::from("Keep login"), Some("keep".into()));
            add(
                Line::styled("Remove saved login", Style::default().fg(colour::error())),
                Some("confirm-remove".into()),
            );
        }
        View::Waiting {
            url,
            code,
            expires_in,
        } => {
            add(
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
            add(
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
                    add(Line::styled(format!("Your code:  {code}"), bright), None);
                    add(
                        Line::from("o open browser · c copy code · u copy link"),
                        None,
                    );
                } else {
                    add(Line::from("o open browser · c copy link"), None);
                }
                for line in wrap(url, width) {
                    add(Line::styled(line, dim), None);
                }
            }
        }
    }
    let mut pane = Pane::new("login", rows)
        .selectable(picks)
        .tinted(Some(colour::accent()));
    pane.first();
    pane
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
}

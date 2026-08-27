//! `axum models` — what axum can talk to, and what it would take to enable it.

use axum_provider::provider::Provider;

/// Print the catalog.
///
/// Unconfigured providers are listed too, with the variable that would enable them: someone
/// choosing a model wants to see what exists, not a list narrowed to whatever happens to be
/// exported in this shell.
pub fn print(all: bool) {
    let (providers, default) = match crate::config::load() {
        Ok(loaded) => {
            let default = loaded.config.string("model").map(str::to_owned);
            (loaded.providers, default)
        }
        Err(e) => {
            eprintln!("{e}");
            (crate::config::builtin().unwrap_or_default(), None)
        }
    };
    let configured = providers.iter().filter(|p| p.is_configured()).count();

    for provider in &providers {
        if !all && !provider.is_configured() {
            continue;
        }
        print_provider(provider, default.as_deref());
    }

    if !all && configured < providers.len() {
        println!(
            "\n{} of {} providers configured. `axum models --all` shows the rest.",
            configured,
            providers.len()
        );
    }
}

fn print_provider(provider: &Provider, default: Option<&str>) {
    // A protocol axum cannot speak matters more than a missing key: no amount of configuring
    // will help, and finding that out after setting one is a waste of the reader's time.
    let status = if let Some(why) = axum_lua::adapter::why_unspoken(provider.api.as_str()) {
        format!("  (not yet spoken: {})", first_sentence(why))
    } else if provider.is_configured() {
        String::new()
    } else {
        format!("  ({})", provider.auth.requirement())
    };
    println!("\n{} — {}{}", provider.name, provider.api.as_str(), status);

    for model in &provider.models {
        let qualified = model.qualified();
        // The chosen model is marked in place rather than listed apart: a person scanning for
        // it wants to see it beside the alternatives they might switch to.
        let marker = if default == Some(qualified.as_str()) {
            "*"
        } else {
            " "
        };
        let reasoning = if model.reasoning { " reasoning" } else { "" };
        println!(
            "{marker} {:<44} {:>9} ctx  ${:.2}/${:.2}{}",
            qualified,
            thousands(model.context_window),
            model.cost.input,
            model.cost.output,
            reasoning
        );
    }
}

/// `200000` becomes `200k`, because a context window is compared, not counted.
fn thousands(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// The first sentence of an explanation, for a line that has to fit on a terminal.
fn first_sentence(text: &str) -> String {
    text.split_once(". ")
        .map_or_else(|| text.to_owned(), |(first, _)| format!("{first}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_abbreviate() {
        assert_eq!(thousands(8192), "8192");
        assert_eq!(thousands(200_000), "200k");
        assert_eq!(thousands(1_048_576), "1.0M");
    }

    #[test]
    fn the_lua_catalog_is_reachable_from_the_binary() {
        assert!(!crate::config::builtin().expect("the catalog").is_empty());
    }

    #[test]
    fn local_providers_need_no_variable_named() {
        let providers = crate::config::builtin().expect("the catalog");
        let ollama = providers.iter().find(|p| p.id == "ollama").expect("ollama");
        assert!(ollama.auth.vars().is_empty());
    }
}

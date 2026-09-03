//! `magi models` — what magi can talk to, and what it would take to enable it.
//!
//! Asked of melchior, which owns the catalog. magi kept its own once and this printed that; the
//! two were the same file copied twice, and the copy here was the one that went stale.

use magi_proto::ask::Card;

/// Print the catalog.
///
/// Models that are not ready are listed too, with the variable that would enable them: somebody
/// choosing a model wants to see what exists, not a list narrowed to whatever happens to be
/// exported in this shell.
pub fn print(all: bool) {
    let default = crate::config::load()
        .ok()
        .and_then(|loaded| loaded.config.string("model").map(str::to_owned));

    let cards = melchior();
    if cards.is_empty() {
        eprintln!("magi: melchior is not answering, so there are no models to list.");
        eprintln!("      install it, or run `melchior models` to see what it says.");
        return;
    }

    let ready = cards.iter().filter(|card| card.ready).count();
    let mut provider = String::new();
    for card in &cards {
        if !all && !card.ready {
            continue;
        }
        // Grouped under the provider, printed when it changes. The cards arrive sorted, so this
        // is a header rather than a sort of its own.
        if card.provider != provider {
            provider.clone_from(&card.provider);
            println!("\n{provider}");
        }
        print_card(card, default.as_deref());
    }

    if !all && ready < cards.len() {
        println!(
            "\n{} more not ready. `magi models --all` lists them with what each needs.",
            cards.len() - ready
        );
    }
}

/// One card, as a line.
fn print_card(card: &Card, default: Option<&str>) {
    let marker = if default == Some(card.id.as_str()) {
        "*"
    } else {
        " "
    };
    let window = card
        .context_window
        .map(|n| format!("{}k", n / 1000))
        .unwrap_or_default();
    let note = match (&card.needs, card.ready) {
        (_, true) => String::new(),
        (Some(variable), _) => format!("  needs {variable}"),
        (None, _) => "  not reachable".to_owned(),
    };
    let reasons = if card.reasons { " reasons" } else { "" };
    println!("{marker} {:<48} {window:>7}{reasons}{note}", card.id);
}

/// Every card melchior offers, or nothing when it will not answer.
///
/// Blocking, because this is a command rather than a session: there is no runtime to borrow and
/// nothing else to get on with while it answers.
fn melchior() -> Vec<Card> {
    let Ok(out) = std::process::Command::new("melchior")
        .arg("models")
        .arg("--json")
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let Ok(reply) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    if reply.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Vec::new();
    }
    reply
        .get("result")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| serde_json::from_value(row.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(ready: bool) -> Card {
        Card {
            id: "p/m".into(),
            provider: "p".into(),
            name: "m".into(),
            api: "openai-completions".into(),
            context_window: Some(128_000),
            max_output: None,
            reasons: false,
            ready,
            needs: if ready { None } else { Some("P_KEY".into()) },
        }
    }

    #[test]
    fn a_card_that_is_not_ready_names_the_variable_rather_than_hiding() {
        // Printed rather than asserted on, because the value here is that the reason reaches a
        // person at all: a list that silently omits the model they wanted is the failure.
        let card = card(false);
        assert_eq!(card.needs.as_deref(), Some("P_KEY"));
        print_card(&card, None);
    }

    #[test]
    fn an_absent_melchior_is_no_models_rather_than_a_panic() {
        // Whatever is on this machine, the shape of the answer is a list.
        let _: Vec<Card> = melchior();
    }

    #[test]
    fn the_chosen_model_is_the_one_marked() {
        let card = card(true);
        assert_eq!(card.id, "p/m");
        print_card(&card, Some("p/m"));
    }
}

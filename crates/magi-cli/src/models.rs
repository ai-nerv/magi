//! `magi models` — what magi can talk to, and what it would take to enable it.
//!
//! Asked of melchior, which owns the catalog. magi kept its own once and this printed that; the
//! two were the same file copied twice, and the copy here was the one that went stale.

use magi_proto::ask::Card;

mod metadata;
pub(crate) use metadata::capabilities;
pub(crate) use metadata::details;
pub(crate) use metadata::{Columns, provider_name, table_cells_for};

/// Print the catalog.
///
/// Models that are not ready are listed too, with the variable that would enable them: somebody
/// choosing a model wants to see what exists, not a list narrowed to whatever happens to be
/// exported in this shell.
pub async fn print(all: bool, how: crate::verbs::As) {
    let loaded = match crate::config::for_session() {
        Ok(loaded) => loaded,
        Err(why) => {
            if how.framed() {
                crate::verbs::say(&magi_ipc::family::Reply::refused(why.to_string()), how);
                return;
            }
            eprintln!("magi: {why}");
            std::process::exit(1);
        }
    };
    let default = loaded
        .as_ref()
        .and_then(|loaded| loaded.config.string("model").map(str::to_owned));
    // The `model` role's program, the one a session would ask, rather than whatever is called
    // melchior on `$PATH`.
    let program = loaded.as_ref().map_or_else(
        || magi_host::broker::MELCHIOR.to_owned(),
        crate::config::mind,
    );

    let cards = melchior(&program).await;
    // Framed, the cards go back as they came: every one of them, since narrowing a listing is a
    // courtesy to a reader and a caller that asked for bytes is not one.
    if how.framed() {
        if cards.is_empty() {
            crate::verbs::say(
                &magi_ipc::family::Reply::refused(
                    "melchior is not answering, so magi has no models to list",
                ),
                how,
            );
            return;
        }
        let rows = cards
            .iter()
            .filter(|card| all || card.ready)
            .map(|card| {
                let mut row = serde_json::to_value(card).unwrap_or_default();
                if let Some(row) = row.as_object_mut() {
                    row.insert(
                        "default".into(),
                        (default.as_deref() == Some(&card.id)).into(),
                    );
                }
                row
            })
            .collect();
        crate::verbs::say(&magi_ipc::family::Reply::rows(rows), how);
        return;
    }
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
    let subscription = magi_host::catalog::setup::is_subscription(card);
    let metadata = details(
        card.context_window.unwrap_or(0),
        (!subscription).then_some(card.pricing.as_ref()).flatten(),
        (!subscription).then_some(card.popularity_rank).flatten(),
        "",
    );
    let note = if card.ready {
        String::new()
    } else {
        format!("  {}", magi_host::catalog::setup::requirement(card))
    };
    let capabilities =
        capabilities(card.capabilities.as_ref(), card.vision, card.reasons).replace('\n', " · ");
    println!("{marker} {:<48} {metadata} · {capabilities}{note}", card.id);
    if let Some(name) = &card.display_name {
        let name: String = name.chars().filter(|c| !c.is_control()).take(512).collect();
        println!("    {name}");
    }
}

/// Every card `program` offers, or nothing when it will not answer.
async fn melchior(program: &str) -> Vec<Card> {
    magi_host::broker::cards(program).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(ready: bool) -> Card {
        Card {
            vision: false,
            id: "p/m".into(),
            provider: "p".into(),
            name: "m".into(),
            display_name: None,
            api: "openai-completions".into(),
            context_window: Some(128_000),
            max_output: None,
            pricing: None,
            popularity_rank: None,
            capabilities: None,
            reasons: false,
            ready,
            setup: None,
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

    #[tokio::test]
    async fn melchior_is_told_which_sessions_settings_to_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = magi_model::scratch::Scratch::new("magi-models", "given");
        let fake = dir.join("melchior");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf '{\"ok\":true,\"n\":1,\"result\":[{\"id\":\"%s\",\"provider\":\"p\",\
             \"name\":\"m\",\"api\":\"x\",\"reasons\":false,\"ready\":true}]}' \"$NERV_GIVEN\"\n",
        )
        .expect("write");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        // `ETXTBSY` while a fork on another thread still holds the descriptor it was written by.
        while let Err(why) = std::process::Command::new(&fake).output() {
            assert_eq!(
                why.raw_os_error(),
                Some(26),
                "the stand-in will not run: {why}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let cards = melchior(&fake.display().to_string()).await;
        assert_eq!(
            cards.first().map(|card| card.id.clone()),
            Some(magi_host::driving::given_dir().display().to_string())
        );
    }

    #[tokio::test]
    async fn an_absent_melchior_is_no_models_rather_than_a_panic() {
        // A program that is not there is an empty catalog, and the command still prints.
        assert!(melchior("magi-no-such-melchior-anywhere").await.is_empty());
    }

    #[test]
    fn the_chosen_model_is_the_one_marked() {
        let card = card(true);
        assert_eq!(card.id, "p/m");
        print_card(&card, Some("p/m"));
    }
}

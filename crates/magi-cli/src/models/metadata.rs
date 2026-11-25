//! Model-list context, published prices, and OpenRouter weekly popularity positions.

use magi_proto::ask::Pricing;

#[must_use]
pub(crate) fn provider_name(choice: &magi_proto::ModelChoice) -> &str {
    let raw = choice
        .provider
        .as_deref()
        .filter(|provider| !provider.is_empty())
        .unwrap_or_else(|| {
            choice
                .name
                .split_once('/')
                .map_or("", |(provider, _)| provider)
        });
    if raw == "decisions" {
        "openrouter"
    } else {
        raw
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Columns {
    pub priced: bool,
    pub ranked: bool,
}

impl Columns {
    #[must_use]
    pub(crate) fn for_choices(choices: &[&magi_proto::ModelChoice]) -> Self {
        Self {
            priced: choices
                .iter()
                .any(|choice| !choice.subscription && has_price(choice.pricing.as_ref())),
            ranked: choices.iter().any(|choice| {
                !choice.subscription && choice.popularity_rank.is_some_and(|r| r > 0)
            }),
        }
    }

    #[must_use]
    pub(crate) fn headers(self) -> Vec<&'static str> {
        let mut columns = vec!["Model", "Ctx"];
        if self.priced {
            columns.push("In$/M");
            columns.push("Out$/M");
        }
        if self.ranked {
            columns.push("ORwk");
        }
        columns.push("Modality");
        columns.push("Thinking");
        columns
    }
}

pub(crate) fn table_cells_for(choice: &magi_proto::ModelChoice, columns: Columns) -> String {
    let context = if choice.context_window == 0 {
        "—".into()
    } else {
        magi_tui::footer::format_tokens(choice.context_window)
    };
    let pricing = (!choice.subscription)
        .then_some(choice.pricing.as_ref())
        .flatten();
    let input = price(pricing.and_then(|pricing| pricing.input.as_deref()));
    let output = price(pricing.and_then(|pricing| pricing.output.as_deref()));
    let rank = (!choice.subscription)
        .then_some(choice.popularity_rank)
        .flatten()
        .filter(|rank| *rank > 0)
        .map_or_else(|| "—".into(), |rank| format!("#{rank}"));
    let caps = capabilities(
        choice.capabilities.as_ref(),
        choice.vision,
        choice.reasoning,
    );
    let (modality, thinking) = caps.split_once('\n').unwrap_or((&caps, ""));
    let modality = modality
        .trim_start_matches("input: ")
        .replace(" → output: ", "→");
    let thinking = thinking
        .trim_start_matches("thinking: ")
        .replace("available (levels unknown)", "on(?)");
    let mut cells = vec![context.as_str()];
    if columns.priced {
        cells.push(&input);
        cells.push(&output);
    }
    if columns.ranked {
        cells.push(&rank);
    }
    cells.push(&modality);
    cells.push(&thinking);
    cells.join("\t")
}

pub(crate) fn capabilities(
    caps: Option<&magi_model::Capabilities>,
    vision: bool,
    reasoning: bool,
) -> String {
    let input = caps.filter(|caps| !caps.input.is_empty()).map_or_else(
        || {
            if vision {
                "text,image".into()
            } else {
                "text".into()
            }
        },
        |caps| caps.input.join(","),
    );
    let output = caps
        .filter(|caps| !caps.output.is_empty())
        .map_or_else(|| "—".into(), |caps| caps.output.join(","));
    let thinking = caps
        .and_then(|caps| caps.thinking.as_ref())
        .filter(|levels| !levels.is_empty())
        .map_or_else(
            || {
                if reasoning {
                    "available (levels unknown)".into()
                } else {
                    "off".into()
                }
            },
            |levels| levels.join(","),
        );
    format!("input: {input} → output: {output}\nthinking: {thinking}")
}

pub(crate) fn details(
    window: u64,
    pricing: Option<&Pricing>,
    rank: Option<u64>,
    requirement: &str,
) -> String {
    let context = if window == 0 {
        "context —".into()
    } else {
        magi_tui::footer::format_tokens(window)
    };
    let mut detail = context;
    if has_price(pricing) {
        let input = price(pricing.and_then(|pricing| pricing.input.as_deref()));
        let output = price(pricing.and_then(|pricing| pricing.output.as_deref()));
        detail.push_str(&format!(" · in {input} out {output} USD/M"));
    }
    if let Some(rank) = rank.filter(|rank| *rank > 0) {
        detail.push_str(&format!(" · OR wk #{rank}"));
    }
    if !requirement.is_empty() {
        detail.push_str(&format!(" · {requirement}"));
    }
    detail
}

pub(crate) fn has_price(pricing: Option<&Pricing>) -> bool {
    pricing.is_some_and(|pricing| {
        valid_price(pricing.input.as_deref()).is_some()
            || valid_price(pricing.output.as_deref()).is_some()
    })
}

fn valid_price(value: Option<&str>) -> Option<f64> {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|rate| rate.is_finite() && *rate >= 0.0)
}

fn price(value: Option<&str>) -> String {
    let Some(rate) = valid_price(value) else {
        return "—".into();
    };
    let number = if rate > 0.0 && rate < 0.0001 {
        rate.to_string()
    } else {
        format!("{rate:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    };
    format!("${number}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_show_native_levels_and_do_not_guess_missing_levels() {
        let caps = magi_model::Capabilities {
            session_identity: None,
            input: vec!["text".into(), "image".into(), "audio".into()],
            output: vec!["text".into()],
            thinking: Some(vec!["low".into(), "xhigh".into()]),
            thinking_levels: None,
            strict_tools: None,
            input_limits: None,
        };
        assert_eq!(
            capabilities(Some(&caps), true, true),
            "input: text,image,audio → output: text\nthinking: low,xhigh"
        );
        assert_eq!(
            capabilities(None, true, true),
            "input: text,image → output: —\nthinking: available (levels unknown)"
        );
        assert!(capabilities(None, false, false).ends_with("thinking: off"));
    }

    #[test]
    fn context_prices_and_weekly_position_have_explicit_units() {
        let pricing = Pricing {
            input: Some("0.3".into()),
            output: Some("1.2".into()),
        };
        let shown = details(128_000, Some(&pricing), Some(7), "");
        assert_eq!(shown, "128k · in $0.3 out $1.2 USD/M · OR wk #7");
    }

    #[test]
    fn unknown_is_not_free_and_auth_does_not_hide_metadata() {
        assert_eq!(details(0, None, None, "set KEY"), "context — · set KEY");
        let pricing = Pricing {
            input: Some("0".into()),
            output: None,
        };
        assert_eq!(
            details(128_000, Some(&pricing), None, ""),
            "128k · in $0 out — USD/M"
        );
    }

    #[test]
    fn invalid_prices_are_unknown_and_small_prices_do_not_round_to_free() {
        for invalid in ["-1", "NaN", "inf", "not-a-price"] {
            assert_eq!(price(Some(invalid)), "—");
        }
        assert_eq!(price(Some("0.00001")), "$0.00001");
        assert_eq!(price(Some("0.30000000000004")), "$0.3");
    }
}

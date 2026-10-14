//! Model and request measurements underneath each tree entry.

use super::Agent;

pub(super) fn lines(agent: &Agent) -> Vec<String> {
    let Some(details) = &agent.details else {
        return vec!["model/context: unavailable (connecting or older host)".to_owned()];
    };
    let model = details
        .model
        .as_ref()
        .map_or("not configured", |model| model.name.as_str());
    let mut rows = vec![
        format!("model: {model}"),
        format!("thinking: {}", details.thinking),
    ];
    let context = match &details.context {
        Some(context) => {
            let prefix = if context.estimated { "~" } else { "" };
            let percent = if context.capacity > 0 {
                format!(
                    " ({:.1}%)",
                    context.input as f64 / context.capacity as f64 * 100.0
                )
            } else {
                String::new()
            };
            format!(
                "last input: {prefix}{} / {}{percent}; limit {}",
                context.input, context.capacity, context.limit
            )
        }
        None => match details
            .model
            .as_ref()
            .filter(|model| model.context_window > 0)
        {
            Some(model) => format!("context: not measured / {}", model.context_window),
            None => "context: not measured / unknown".to_owned(),
        },
    };
    rows.push(context);
    if details.cost_micros > 0 {
        rows.push(format!(
            "reported cost: ${:.4}",
            details.cost_micros as f64 / 1_000_000.0
        ));
    }
    rows
}

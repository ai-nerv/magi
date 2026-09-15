//! What the provider publishes about a model, asked of melchior's `card`: a network call, so it runs
//! on a thread of its own, and the model's card is drawn without it and filled in when it answers.

use magi_tui::model_card::{Details, Endpoint};
use serde_json::Value;

/// Ask `program` about `model`. The error is what the card says in the details' place.
pub fn fetch(program: &str, model: &str) -> Result<Details, String> {
    let out = std::process::Command::new(program)
        .args(["card", "--model", model, "--json"])
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|why| format!("{program} could not be asked: {why}"))?;
    let reply: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| format!("{program} gave no card; it may be older than this magi"))?;
    if reply.get("ok").and_then(Value::as_bool) != Some(true) {
        let why = ["/error/message", "/error", "/why", "/message"]
            .iter()
            .find_map(|at| reply.pointer(at).and_then(Value::as_str))
            .unwrap_or("nothing is published about this model");
        return Err(why.to_owned());
    }
    reply
        .pointer("/result/0")
        .map(read)
        .ok_or_else(|| "an empty answer".to_owned())
}

/// A card as melchior writes one, into what the model's card draws. Anything absent stays absent.
fn read(card: &Value) -> Details {
    let text = |key: &str| {
        card.get(key)
            .and_then(Value::as_str)
            .filter(|said| !said.is_empty())
            .map(ToOwned::to_owned)
    };
    let price = |key: &str| {
        card.pointer(&format!("/price/{key}"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let rows = |key: &str| {
        card.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let names = |key: &str| -> Vec<String> {
        rows(key)
            .iter()
            .filter_map(|name| name.as_str().map(ToOwned::to_owned))
            .collect()
    };
    Details {
        tokenizer: text("tokenizer"),
        inputs: names("inputs"),
        outputs: names("outputs"),
        features: names("features"),
        created: card.get("created").and_then(Value::as_u64),
        moderated: card.get("moderated").and_then(Value::as_bool),
        description: text("description"),
        knowledge_cutoff: text("knowledge_cutoff"),
        modality: text("modality"),
        max_output: card.get("max_output").and_then(Value::as_u64),
        price: [
            price("input"),
            price("output"),
            price("cache_read"),
            price("cache_write"),
        ],
        benchmarks: rows("benchmarks")
            .iter()
            .filter_map(|row| {
                Some((
                    row.get("name")?.as_str()?.to_owned(),
                    row.get("score")?.as_f64()?,
                ))
            })
            .collect(),
        endpoints: rows("endpoints")
            .iter()
            .filter_map(|row| {
                let priced = |key: &str| {
                    row.pointer(&format!("/price/{key}"))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                };
                Some(Endpoint {
                    provider: row.get("provider")?.as_str()?.to_owned(),
                    tag: row
                        .get("tag")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    quantization: row
                        .get("quantization")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    price: [
                        priced("input"),
                        priced("output"),
                        priced("cache_read"),
                        priced("cache_write"),
                    ],
                    context: row.get("context").and_then(Value::as_u64),
                    max_output: row.get("max_output").and_then(Value::as_u64),
                    latency_ms: row.get("latency_ms").and_then(Value::as_f64),
                    throughput: row.get("throughput").and_then(Value::as_f64),
                })
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_is_read_field_for_field_and_what_is_absent_stays_absent() {
        let card = serde_json::json!({
            "description": "fast", "modality": "text->text", "max_output": 32000,
            "price": {"input": 0.3, "output": 1.2},
            "benchmarks": [{"name": "coding", "score": 38.5}],
            "endpoints": [{"provider": "DeepInfra", "tag": "deepinfra/fp4", "quantization": "fp4",
                           "price": {"input": 0.04, "output": 0.1}, "context": 1048576,
                           "throughput": 85.5},
                          {"provider": "Novita"}],
            "tokenizer": "DeepSeek", "inputs": ["text"], "outputs": ["text"],
            "features": ["tools", "reasoning"], "created": 1700000000, "moderated": false,
        });
        let read = read(&card);
        assert_eq!(read.description.as_deref(), Some("fast"));
        assert_eq!(read.knowledge_cutoff, None);
        assert_eq!(read.max_output, Some(32_000));
        assert_eq!(read.price, [0.3, 1.2, 0.0, 0.0]);
        assert_eq!(read.benchmarks, [("coding".to_owned(), 38.5)]);
        assert_eq!(read.endpoints.len(), 2);
        assert_eq!(read.endpoints[0].tag.as_deref(), Some("deepinfra/fp4"));
        assert_eq!(read.endpoints[0].price, [0.04, 0.1, 0.0, 0.0]);
        assert_eq!(read.endpoints[0].context, Some(1_048_576));
        assert_eq!(read.endpoints[0].throughput, Some(85.5));
        assert_eq!(read.endpoints[1].tag, None);
        assert_eq!(read.tokenizer.as_deref(), Some("DeepSeek"));
        assert_eq!(read.features, ["tools", "reasoning"]);
        assert_eq!(read.created, Some(1_700_000_000));
        assert_eq!(read.moderated, Some(false));
    }

    #[test]
    fn a_program_that_is_not_there_is_said_rather_than_panicked() {
        let why = fetch("/nonexistent/melchior-xyz", "a/b").expect_err("no such program");
        assert!(why.contains("could not be asked"), "{why}");
    }
}

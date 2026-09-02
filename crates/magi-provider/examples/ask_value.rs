//! Ask a live provider for one value of a fixed shape.
//!
//! Not a test: it needs a key and a network. It exists so "structured output works" is a thing
//! somebody can check in ten seconds rather than a claim in a commit message.
//!
//! ```sh
//! OPENROUTER_API_KEY=… cargo run -p magi-provider --example ask_value
//! ```

fn main() {
    let Ok(key) = std::env::var("OPENROUTER_API_KEY") else {
        eprintln!("set OPENROUTER_API_KEY");
        std::process::exit(1);
    };
    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "deepseek/deepseek-v4-flash-0731".to_owned());

    let body = serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": "Which of these does deleting a file need: read, write, run? Answer the schema.",
        }],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "needs",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {
                        "verbs": { "type": "array", "items": { "type": "string" } },
                        "why": { "type": "string" },
                    },
                    "required": ["verbs", "why"],
                    "additionalProperties": false,
                },
            },
        },
    });

    let out = std::process::Command::new("curl")
        .args(["-s", "https://openrouter.ai/api/v1/chat/completions"])
        .args(["-H", &format!("Authorization: Bearer {key}")])
        .args(["-H", "Content-Type: application/json"])
        .args(["-d", &body.to_string()])
        .output()
        .expect("curl");
    let text = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let content = parsed["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    println!("raw: {content}");
    match serde_json::from_str::<serde_json::Value>(content) {
        Ok(value) => println!("parsed: {value:#}"),
        Err(why) => println!("NOT the shape asked for: {why}"),
    }
}

//! `magi verbs` — what this program answers, on each of its doors. The command line half is read
//! off clap rather than listed by hand, so the surface cannot have a second definition.

use clap::CommandFactory;

/// The revision of the family contract this reply is written in.
const FAMILY: u16 = magi_ipc::family::FAMILY;

/// The revision of the *registrar* surface — what a third party writes against, separate from
/// [`FAMILY`]. Goes up only when something already published stops working. See EXTENDING.md.
const SURFACE: u16 = 1;

/// Print the surface, in the family's reply shape.
pub fn print(cbor: bool) {
    let listed: Vec<serde_json::Value> = super::Cli::command()
        .get_subcommands()
        .map(|sub| {
            serde_json::json!({
                "verb": sub.get_name(),
                "about": sub.get_about().map(|a| a.to_string()).unwrap_or_default(),
                "door": "cli",
            })
        })
        .collect();

    let body = serde_json::json!({
        "ok": true,
        "family": FAMILY,
        "surface": SURFACE,
        "n": listed.len(),
        "result": listed,
    });

    if cbor {
        let mut bytes = Vec::new();
        if ciborium::into_writer(&body, &mut bytes).is_ok() {
            use std::io::Write;
            if std::io::stdout().lock().write_all(&bytes).is_ok() {
                return;
            }
        }
    }
    println!("{body}");
}

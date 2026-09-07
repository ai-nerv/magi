//! `magi verbs` — what this program answers, on each of its doors.
//!
//! Every family program answers this, in the family's reply shape. A self-description another
//! program cannot parse is one only a person can use, and then the way to discover a surface is
//! to read its source — which is the situation the contract exists to end.
//!
//! **magi coordinates rather than being coordinated**, so it answers the floor and not `needs` or
//! `configure`: there is nothing above it to hand it settings. That asymmetry is written down in
//! FAMILY.md rather than left for a reader to infer from an absence.
//!
//! The command line half is read off clap rather than listed by hand. A hand-kept list is a
//! second place for the surface to live, and the contract's own rule — everything advertised is
//! dispatched — becomes something somebody has to remember instead of something that cannot be
//! otherwise.

use clap::CommandFactory;

/// The revision of the family contract this reply is written in.
const FAMILY: u16 = magi_ipc::family::FAMILY;

/// The revision of the *registrar* surface — what a third party writes against.
///
/// Separate from [`FAMILY`], because they change for different reasons and a consumer cares about
/// different halves. `family` is the wire between these programs: the reply shape, the encodings,
/// which verbs exist. `surface` is what somebody's plugin file is written against: the registrar
/// names, the fields each declaration owes, and the events a watcher is told about.
///
/// **It goes up when something already published stops working.** Adding a registrar, a field, or
/// an event does not move it — a file written against 1 keeps running. Renaming one, removing one,
/// or changing what a field means does, and that is the number a plugin checks if it wants to
/// refuse rather than fail halfway.
///
/// Five registrars were published, dead and unversioned for most of this project's life. This is
/// what closes that window deliberately rather than by accident. See EXTENDING.md.
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
        // Falls through to JSON rather than writing nothing: silence is the one answer a caller
        // cannot read, because it waits for a frame that never comes.
    }
    println!("{body}");
}

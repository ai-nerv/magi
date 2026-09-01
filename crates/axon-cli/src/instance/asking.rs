//! Calling another instance.
//!
//! The dialling half of the mirror, and the same frames [`super::serving`] answers. One
//! question, one answer, nothing held: a call acquires a connection, sends a frame, reads the
//! reply and closes.
//!
//! Held sessions are the other verb and are not written yet. The split matters — a channel you
//! keep can subscribe and must be closed, a question cannot do either — so `ask` is named for
//! what the caller wanted rather than for what the transport happens to be, and a later `hold`
//! changes no call site here.

use super::wire::{Call, Reply};
use super::{Address, Kind, Reach};
use crate::identity::Identity;
use std::time::Duration;

/// How long to wait on a peer before giving up.
///
/// Short, because this is called from a session somebody is sitting in front of. A peer that is
/// mid-turn answers from its own frame loop and is not slow; one that does not answer in two
/// seconds is wedged, and saying so beats blocking the asker.
const PATIENCE: Duration = Duration::from_secs(2);

/// Ask `who` something.
///
/// `kind` is what the *caller* believes the far end is, and it is checked here as well as
/// there. Twice on purpose: the far end enforces it because a caller cannot be trusted, and
/// this end enforces it so a refusal reads as "you may not do that" rather than travelling to
/// another process to come back as the same sentence a round trip later.
pub async fn ask(
    who: &Address,
    me: &Identity,
    kind: Kind,
    verb: &str,
    args: Vec<serde_json::Value>,
) -> Result<Reply, String> {
    let wanted = match verb {
        "tell" => Reach::Tell,
        "stop" => Reach::Stop,
        _ => Reach::Ask,
    };
    if !wanted.allows(kind) {
        return Err(wanted.refusal(who));
    }
    let path = who.socket(me);
    let stream = tokio::time::timeout(PATIENCE, axon_ipc::connect(&path))
        .await
        .map_err(|_| format!("{} did not answer", who.written()))?
        .map_err(|_| format!("{} is not listening", who.written()))?;

    let (read, write) = stream.into_split();
    let mut reader = axon_ipc::FrameReader::new(read);
    let mut writer = axon_ipc::FrameWriter::new(write);
    let call = Call {
        call: verb.to_owned(),
        args,
    };
    writer
        .write(&call)
        .await
        .map_err(|why| format!("{}: {why}", who.written()))?;
    tokio::time::timeout(PATIENCE, reader.read())
        .await
        .map_err(|_| format!("{} did not answer {verb}", who.written()))?
        .map_err(|why| format!("{}: {why}", who.written()))
}

/// What is listening, as addresses.
///
/// Every socket in the directory, whether or not anything is behind it. A name that no longer
/// answers is discovered by asking it, which is the only way that does not go stale.
#[must_use]
pub fn around(me: &Identity) -> Vec<Address> {
    super::listening()
        .into_iter()
        .filter_map(|name| Address::read(&name.replace('-', "/")))
        .filter(|address| address.against(me).full() != me.full())
        .collect()
}

/// A refusal happens before the round trip, and a name has to resolve to somewhere.
#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Identity {
        Identity {
            project: "axon".to_owned(),
            role: "main".to_owned(),
            id: "alpha".to_owned(),
        }
    }

    #[tokio::test]
    async fn stopping_a_peer_is_refused_without_dialling_it() {
        // Checked at both ends. Here so the refusal reads as "you may not do that" rather than
        // travelling to another process to come back as the same sentence a round trip later.
        let who = Address::read("$main/delta").expect("an address");
        let why = ask(&who, &me(), Kind::Peer, "stop", Vec::new())
            .await
            .expect_err("a peer was stopped");
        assert!(why.contains("peer"), "{why}");
        assert!(why.contains("$main/delta"), "{why}");
    }

    #[tokio::test]
    async fn asking_a_peer_is_allowed_and_fails_only_because_nobody_is_there() {
        // The permission check must not be what stops an `ask`; the missing socket must be.
        let who = Address::read("$main/nobody-is-here").expect("an address");
        let why = ask(&who, &me(), Kind::Peer, "status", Vec::new())
            .await
            .expect_err("something answered");
        assert!(
            why.contains("not listening") || why.contains("did not answer"),
            "refused for the wrong reason: {why}"
        );
    }

    #[tokio::test]
    async fn a_fork_may_be_told_and_stopped() {
        let who = Address::read("$gamma").expect("an address");
        for verb in ["tell", "stop"] {
            let why = ask(&who, &me(), Kind::Fork, verb, Vec::new())
                .await
                .expect_err("something answered");
            assert!(
                !why.contains("peer"),
                "{verb} on a fork was refused as a peer: {why}"
            );
        }
    }

    #[test]
    fn we_are_not_in_our_own_list_of_neighbours() {
        // A session that could address itself is a session that can deadlock on its own socket:
        // the frame loop that would answer is the one waiting for the answer.
        assert!(
            around(&me())
                .iter()
                .all(|a| a.against(&me()).full() != me().full()),
            "it offered itself"
        );
    }
}

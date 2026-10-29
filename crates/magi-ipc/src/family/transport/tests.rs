use super::*;
use magi_model::scratch::Scratch;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

async fn request(stream: &mut UnixStream) -> Value {
    let length = stream.read_u32().await.expect("frame length");
    assert!(length < 4096);
    let mut body = vec![0; length as usize];
    stream.read_exact(&mut body).await.expect("request");
    crate::Wire::read(&body).expect("request JSON")
}

async fn reply(stream: &mut UnixStream, value: Value) {
    let body = serde_json::to_vec(&value).expect("encode");
    stream.write_u32(body.len() as u32).await.expect("header");
    stream.write_all(&body).await.expect("reply");
}

async fn no_redial(listener: &UnixListener, family: &mut Family) {
    for _ in 0..2 {
        let fault = family
            .call("observe", vec![])
            .await
            .expect_err("lost ownership");
        assert!(matches!(fault, Fault::Failed(_)));
        assert!(fault.is_fatal());
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn lost_or_malformed_reply_never_reconnects_a_pinned_transport() {
    for malformed in [false, true] {
        let dir = Scratch::new("mfp", "lost");
        let path = dir.join("m.sock");
        let listener = UnixListener::bind(&path).expect("bind");
        let mut family = Family::dial(&path).await.expect("dial");
        let (mut peer, _) = listener.accept().await.expect("accept");
        family.pin_connection().expect("pin");
        let serving = async {
            assert_eq!(request(&mut peer).await["call"], "observe");
            if malformed {
                reply(&mut peer, json!({"ok":true,"result":"not a list"})).await;
            }
            drop(peer);
        };
        let (result, ()) = tokio::join!(family.call("observe", vec![]), serving);
        assert!(matches!(result, Err(Fault::Failed(_))));
        no_redial(&listener, &mut family).await;
    }
}

#[tokio::test]
async fn cancelled_call_loses_ownership_even_without_a_returned_error() {
    let dir = Scratch::new("mfp", "cancel");
    let path = dir.join("m.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    let mut family = Family::dial(&path).await.expect("dial");
    let (mut peer, _) = listener.accept().await.expect("accept");
    family.pin_connection().expect("pin");
    let mut call = Box::pin(family.call("observe", vec![]));
    tokio::select! {
        _ = &mut call => panic!("reply without server response"),
        value = request(&mut peer) => assert_eq!(value["call"], "observe"),
        _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("request deadline"),
    }
    drop(call);
    assert!(family.check_connection().is_err());
    no_redial(&listener, &mut family).await;
    assert_eq!(
        peer.read_u8().await.expect_err("closed").kind(),
        std::io::ErrorKind::UnexpectedEof
    );
}

#[tokio::test]
async fn timeout_loses_ownership_but_a_complete_refusal_does_not() {
    let dir = Scratch::new("mfp", "timeout");
    let path = dir.join("m.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    let mut family = Family::dial(&path).await.expect("dial");
    let (mut peer, _) = listener.accept().await.expect("accept");
    family.pin_connection().expect("pin");
    for value in [
        json!({"ok":false,"error":"feature refused"}),
        json!({"ok":false,"fault":"failed","error":"disk full"}),
        json!({"ok":true,"n":0,"result":[]}),
    ] {
        let serving = async {
            request(&mut peer).await;
            reply(&mut peer, value).await;
        };
        let _ = tokio::join!(family.call("observe", vec![]), serving);
        family
            .check_connection()
            .expect("complete reply retains connection");
    }
    let result = family
        .call_within("observe", vec![], Duration::from_millis(50))
        .await;
    assert!(matches!(result, Err(Fault::Failed(_))));
    no_redial(&listener, &mut family).await;
}

#[tokio::test]
async fn oversized_unsent_call_preserves_the_pinned_connection() {
    let dir = Scratch::new("mfp", "unsent");
    let path = dir.join("m.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    let mut family = Family::dial(&path).await.expect("dial");
    let (mut peer, _) = listener.accept().await.expect("accept");
    family.pin_connection().expect("pin");
    assert!(
        family
            .call(
                "observe",
                vec![json!("x".repeat(super::super::super::MAX_FRAME_BYTES))]
            )
            .await
            .is_err()
    );
    family
        .check_connection()
        .expect("unsent request did not lose owner");
    let serving = async {
        request(&mut peer).await;
        reply(&mut peer, json!({"ok":true,"n":0,"result":[]})).await;
    };
    let (result, ()) = tokio::join!(family.call("observe", vec![]), serving);
    assert!(result.is_ok());
}

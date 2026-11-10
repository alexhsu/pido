//! End-to-end tests of [`PiClient`] against the bundled mock Pi binary.
//!
//! These exercise the real plumbing: process spawn, strict JSONL framing,
//! id correlation, fan-out to subscribers, and graceful disconnect.

use std::path::PathBuf;
use std::time::Duration;

use pi_rpc::client::{ClientEvent, DisconnectReason, PiClient, PiProcessConfig};
use pi_rpc::protocol::{Command, Event, RpcRecord, SessionState};

fn mock_pi_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mock_pi"))
}

fn spawn_mock() -> PiClient {
    let mut config = PiProcessConfig::new(".");
    config.pi_path = Some(mock_pi_path());
    // The mock ignores --mode rpc but receives it, mirroring the real CLI.
    PiClient::spawn(&config).expect("failed to spawn mock pi")
}

#[test]
fn request_correlates_by_id() {
    let client = spawn_mock();

    let response = client
        .request(Command::GetState)
        .expect("get_state request failed");
    assert!(response.success);
    let state = response
        .data_as::<SessionState>()
        .expect("state payload parse failed")
        .expect("state payload missing");
    assert_eq!(state.session_id.as_deref(), Some("mock-session"));
    assert!(!state.is_streaming);
}

#[test]
fn error_responses_carry_error_text() {
    let client = spawn_mock();

    let response = client
        .request(Command::SetModel {
            provider: "mock".into(),
            model_id: "invalid/model".into(),
        })
        .expect("set_model request failed");
    assert!(!response.success);
    assert_eq!(
        response.error.as_deref(),
        Some("Model not found: invalid/model")
    );
}

#[test]
fn prompt_streams_events_to_subscribers() {
    let client = spawn_mock();

    // Subscribe BEFORE sending, per the documented topology.
    let subscriber = client.subscribe();
    let response = client
        .request(Command::Prompt {
            message: "hi".into(),
            images: None,
            streaming_behavior: None,
        })
        .expect("prompt request failed");
    assert!(response.success, "prompt rejected: {:?}", response.error);

    // Collect until agent_settled; assemble the streamed text along the way.
    let mut text = String::new();
    let mut settled = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match subscriber.recv_timeout(Duration::from_secs(10)) {
            Ok(ClientEvent::Record(RpcRecord::Event(event))) => match event {
                Event::MessageUpdate {
                    assistant_message_event:
                        pi_rpc::protocol::AssistantMessageEvent::TextDelta { delta, .. },
                    ..
                } => text.push_str(&delta),
                Event::AgentSettled { .. } => {
                    settled = true;
                    break;
                }
                _ => {}
            },
            Ok(ClientEvent::Disconnected(reason)) => {
                panic!("unexpected disconnect: {reason:?}")
            }
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for agent_settled"),
        }
    }
    assert!(settled, "stream ended without agent_settled");
    assert_eq!(text, "Hello world");
}

#[test]
fn events_flow_while_a_request_is_in_flight() {
    let client = spawn_mock();

    // The mock answers `prompt` with a response followed immediately by the
    // event burst, so a second request racing against those events proves the
    // fan-out does not swallow them.
    let subscriber = client.subscribe();
    client
        .send(Command::Prompt {
            message: "hi".into(),
            images: None,
            streaming_behavior: None,
        })
        .expect("send prompt failed");

    // Concurrent request while the burst is streaming.
    let response = client
        .request(Command::GetState)
        .expect("concurrent get_state failed");
    assert!(response.success);

    // The subscriber still saw the full burst from the prompt.
    let mut settled = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match subscriber.recv_timeout(Duration::from_secs(10)) {
            Ok(ClientEvent::Record(RpcRecord::Event(Event::AgentSettled { .. }))) => {
                settled = true;
                break;
            }
            Ok(ClientEvent::Disconnected(reason)) => panic!("unexpected disconnect: {reason:?}"),
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for agent_settled"),
        }
    }
    assert!(settled, "subscriber missed the prompt's event burst");
}

#[test]
fn closing_stdin_disconnects_gracefully() {
    let client = spawn_mock();
    let subscriber = client.subscribe();

    client.shutdown();

    let _deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match subscriber.recv_timeout(Duration::from_secs(10)) {
            Ok(ClientEvent::Disconnected(reason)) => {
                assert_eq!(reason, DisconnectReason::ClosedByClient);
                return;
            }
            Ok(_) => continue,
            Err(_) => panic!("timed out waiting for disconnect notification"),
        }
    }
}

#[test]
fn spawn_fails_for_missing_cwd() {
    let mut config = PiProcessConfig::new("/definitely/not/a/real/directory");
    config.pi_path = Some(mock_pi_path());
    assert!(PiClient::spawn(&config).is_err());
}

//! Client library for the [Pi coding agent](https://github.com/earendil-works/pi)
//! RPC protocol.
//!
//! Pi is spawned as a child process with `pi --mode rpc`; the client and Pi
//! then exchange strict JSONL records over stdin/stdout (commands in,
//! responses and session events out). This crate owns that protocol: typed
//! records, process lifecycle, request/response correlation by `id`, and a
//! broadcast stream of session events. It has no UI dependencies and is fully
//! unit-testable against a bundled mock Pi binary.

pub mod client;
pub mod protocol;
pub mod sessions;

pub use client::{
    ClientEvent, DisconnectReason, PiClient, PiProcessConfig, probe_auth, probe_pi_install,
};
pub use protocol::{
    AgentMessage, Command, Event, Request, Response, RpcRecord, StreamingBehavior, parse_record,
};
pub use sessions::{SessionGroup, SessionScanner, SessionSummary, read_session_cwd};

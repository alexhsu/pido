//! Manual smoke test against a real `pi --mode rpc` on this machine.
//!
//! Run: `cargo run -p pi-rpc --example real_pi -- [cwd]`
//!
//! Performs a `get_state` round trip (no LLM/auth needed), then optionally a
//! one-shot prompt + agent_settled wait if PI_SMOKE_PROMPT is set. Exits 0 on
//! success.

use std::time::Duration;

use pi_rpc::client::PiClient;
use pi_rpc::protocol::{Command, RpcRecord};

fn main() -> anyhow::Result<()> {
    let cwd = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    println!("pi version: {:?}", pi_rpc::client::probe_pi_install(None));
    println!("spawning pi --mode rpc in {} ...", cwd.display());

    let config =
        pi_rpc::client::PiProcessConfig::new(&cwd).with_args(vec!["--no-session".to_string()]);
    let client = PiClient::spawn(&config)?;
    let subscriber = client.subscribe();

    println!("-- get_state --");
    let response = client.request(Command::GetState)?;
    println!("success={} command={}", response.success, response.command);
    println!("data: {}", serde_json::to_string_pretty(&response.data)?);
    if !response.success {
        anyhow::bail!("get_state failed: {:?}", response.error);
    }

    if let Ok(prompt) = std::env::var("PI_SMOKE_PROMPT") {
        println!("-- prompt: {prompt} --");
        let response = client.request(Command::Prompt {
            message: prompt,
            images: None,
            streaming_behavior: None,
        })?;
        println!("accepted: {}", response.success);

        let mut text = String::new();
        loop {
            match subscriber.recv_timeout(Duration::from_secs(120))? {
                pi_rpc::client::ClientEvent::Record(RpcRecord::Event(event)) => match event {
                    pi_rpc::protocol::Event::MessageUpdate {
                        assistant_message_event:
                            pi_rpc::protocol::AssistantMessageEvent::TextDelta { delta, .. },
                        ..
                    } => {
                        text.push_str(&delta);
                        print!("{delta}");
                    }
                    pi_rpc::protocol::Event::AgentSettled { aborted } => {
                        println!("\n-- agent settled (aborted={aborted}) --");
                        break;
                    }
                    _ => {}
                },
                pi_rpc::client::ClientEvent::Disconnected(reason) => {
                    anyhow::bail!("disconnected: {reason:?}");
                }
                _ => {}
            }
        }
        println!("streamed text: {text}");
    }

    client.shutdown();
    println!("-- done --");
    Ok(())
}

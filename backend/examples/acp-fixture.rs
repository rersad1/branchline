//! Deterministic ACP process fixture. Never shipped or selected outside isolated tests.
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
fn send(v: Value) {
    println!("{v}");
    io::stdout().flush().unwrap();
}
#[allow(clippy::zombie_processes)] // Deliberately owned/reaped by the production supervisor under test.
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!(
            "{}",
            if std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                == "incompatible"
            {
                "9.9.9"
            } else {
                "1.19.0"
            }
        );
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--descendant") {
        std::thread::sleep(std::time::Duration::from_secs(120));
        return;
    }
    let root = std::env::var_os("ACP_FIXTURE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--descendant")
        .spawn()
        .unwrap();
    std::fs::write(
        root.join(format!(
            "pid-{}-{}",
            std::env::consts::OS,
            std::process::id()
        )),
        format!("{} {}", std::process::id(), child.id()),
    )
    .unwrap();
    assert!(std::env::var_os("AWM_TOKEN").is_none());
    let mut n = 0;
    let mut prompts = std::collections::HashMap::<String, Value>::new();
    for line in io::stdin().lock().lines() {
        let v: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let id = &v["id"];
        let p = &v["params"];
        match v["method"].as_str() {
            Some("initialize") => send(
                json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{}}}}}),
            ),
            Some("session/new") => {
                n += 1;
                send(
                    json!({"jsonrpc":"2.0","id":id,"result":{"sessionId":format!("s{n}"),"modes":{"currentModeId":"safe","availableModes":[{"id":"safe","name":"Safe"},{"id":"plan","name":"Plan"}]}}}),
                );
            }
            Some("session/prompt") => {
                prompts.insert(p["sessionId"].as_str().unwrap().to_owned(), id.clone());
                send(
                    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":p["sessionId"],"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"fixture working"}}}}),
                );
                send(
                    json!({"jsonrpc":"2.0","id":format!("permission-{}",p["sessionId"].as_str().unwrap()),"method":"session/request_permission","params":{"sessionId":p["sessionId"],"toolCall":{"title":"Fixture tool"},"options":[{"optionId":"allow","name":"Allow once","kind":"allow_once"},{"optionId":"deny","name":"Reject","kind":"reject_once"}]}}),
                );
            }
            Some("session/cancel") => {
                send(
                    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":p["sessionId"],"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("cancel_seen:{}",p["sessionId"].as_str().unwrap())}}}}),
                );
                if let Some(rid) = prompts.remove(p["sessionId"].as_str().unwrap()) {
                    send(json!({"jsonrpc":"2.0","id":rid,"result":{"stopReason":"cancelled"}}));
                }
            }
            Some("session/set_mode") => send(json!({"jsonrpc":"2.0","id":id,"result":{}})),
            Some("session/load") => send(json!({"jsonrpc":"2.0","id":id,"result":{}})),
            Some("session/list") => send(
                json!({"jsonrpc":"2.0","id":id,"result":{"sessions":[{"sessionId":"foreign","cwd":root}]}}),
            ),
            None => {
                if let Some(rid) = id
                    .as_str()
                    .and_then(|s| s.strip_prefix("permission-"))
                    .and_then(|s| prompts.remove(s))
                {
                    send(json!({"jsonrpc":"2.0","id":rid,"result":{"stopReason":"end_turn"}}));
                }
            }
            _ => send(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"unsupported"}}),
            ),
        }
    }
}

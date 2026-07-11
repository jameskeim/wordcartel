//! Guarded reconfirm of the two remaining harper-ls facts (spec §16) against the packaged binary.
//! `#[ignore]` by default; run with `cargo test -p wordcartel --test harper_ls_probe -- --ignored`.
//! Skips (passes) cleanly when `harper-ls` is not on PATH.
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn harper_on_path() -> bool {
    Command::new("harper-ls").arg("--version").stdout(Stdio::null())
        .stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn frame(v: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).expect("serialize");
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

/// Read one Content-Length-framed JSON message from `r`.
fn read_frame<R: BufRead>(r: &mut R) -> serde_json::Value {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        r.read_line(&mut line).expect("header line");
        let t = line.trim_end();
        if t.is_empty() { break; }
        if let Some(n) = t.strip_prefix("Content-Length:") {
            len = n.trim().parse().expect("content-length");
        }
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).expect("body");
    serde_json::from_slice(&buf).expect("json")
}

#[test]
#[ignore = "requires harper-ls on PATH; run with --ignored"]
// Skip-diagnostic prints to stderr; the workspace denies clippy::print_stderr, so allow it here
// (item-local, house-style exception — an ignored probe's skip message is legitimate test output).
#[allow(clippy::print_stderr)]
fn config_pull_is_unwrapped_and_dictionary_applies() {
    if !harper_on_path() { eprintln!("skip: harper-ls not on PATH"); return; }
    // A temp dictionary containing "wcartelword".
    let dir = std::env::temp_dir().join(format!("wcartel_probe_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dict = dir.join("dictionary.txt");
    std::fs::write(&dict, "wcartelword\n").unwrap();

    let mut child = Command::new("harper-ls").arg("--stdio")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn harper-ls");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let settings = serde_json::json!({
        "dialect": "American",
        "userDictPath": dict.to_string_lossy(),
        "maxFileLength": 10_000_000,
    });

    // initialize (advertise workspace.configuration = true)
    stdin.write_all(&frame(&serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"processId":std::process::id(),"rootUri":null,
            "capabilities":{"workspace":{"configuration":true,"didChangeConfiguration":{}},
                "textDocument":{"publishDiagnostics":{"versionSupport":true},"codeAction":{}}}}
    }))).unwrap();
    // The LSP spec requires the client to await the `initialize` response before sending any
    // further request/notification; harper-ls enforces this (empirically reconfirmed here — an
    // out-of-order client that fires `initialized` before reading this response deadlocks it).
    let _init_response = read_frame(&mut stdout);
    // Pump until we've answered a workspace/configuration pull, then opened a doc, then
    // observed a publishDiagnostics that does NOT flag the dictionary word.
    stdin.write_all(&frame(&serde_json::json!({"jsonrpc":"2.0","method":"initialized","params":{}}))).unwrap();
    stdin.write_all(&frame(&serde_json::json!({
        "jsonrpc":"2.0","method":"workspace/didChangeConfiguration",
        "params":{"settings":{"harper-ls":settings}}}))).unwrap();
    stdin.write_all(&frame(&serde_json::json!({
        "jsonrpc":"2.0","method":"textDocument/didOpen",
        "params":{"textDocument":{"uri":"untitled:wcartel-probe-1","languageId":"markdown",
            "version":1,"text":"wcartelword teh\n"}}}))).unwrap();

    let mut answered_pull = false;
    let mut saw_publish = false;
    let mut dict_word_flagged = true;
    for _ in 0..200 {
        let msg = read_frame(&mut stdout);
        if msg.get("method").and_then(|m| m.as_str()) == Some("workspace/configuration") {
            // VERIFY: request items are empty-section objects.
            let items = msg["params"]["items"].as_array().cloned().unwrap_or_default();
            assert!(!items.is_empty(), "configuration request has items");
            // Respond UNWRAPPED: result is an array of bare settings objects, one per item.
            let result: Vec<serde_json::Value> = items.iter().map(|_| settings.clone()).collect();
            let id = msg["id"].clone();
            stdin.write_all(&frame(&serde_json::json!({"jsonrpc":"2.0","id":id,"result":result}))).unwrap();
            answered_pull = true;
        }
        if msg.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics") {
            saw_publish = true;
            let diags = msg["params"]["diagnostics"].as_array().cloned().unwrap_or_default();
            // "wcartelword" must NOT be flagged (dictionary applied); "teh" SHOULD be.
            let text = "wcartelword teh\n";
            dict_word_flagged = diags.iter().any(|d| {
                let s = d["range"]["start"]["character"].as_u64().unwrap_or(0) as usize;
                s < "wcartelword".len() && text.starts_with("wcartelword")
                    && d["range"]["end"]["character"].as_u64().unwrap_or(0) as usize <= "wcartelword".len()
            });
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait(); // reap — avoid leaving a zombie process behind
    let _ = std::fs::remove_dir_all(&dir);
    assert!(answered_pull, "harper-ls pulled config (PULL model confirmed)");
    assert!(saw_publish, "harper-ls published diagnostics after config answered");
    assert!(!dict_word_flagged, "userDictPath word must not be flagged (unwrapped pull response applied)");
}

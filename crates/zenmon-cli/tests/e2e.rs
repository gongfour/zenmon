//! End-to-end tests: the real `zenmon` binary against a real Zenoh session.
//!
//! Everything else in the suite is pure logic; the paths that open a session
//! (`sub`, `pub`, `query`, `capture`) were only ever checked by hand. Here the
//! test process itself is the network: it opens a peer session listening on a
//! loopback port the OS picks, and each `zenmon` run connects to it in client
//! mode. No `zenohd`, no multicast scouting, nothing outside loopback — so it
//! runs in CI unchanged.
//!
//! Each test uses its own key prefix (so concurrent tests never see each
//! other's traffic) and keeps its traffic flowing until `zenmon` has seen
//! what it needs, because a client's declarations reach the hub some time
//! after its process starts and a single early put could be missed.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use zenoh::Wait;

/// The test's own Zenoh peer, and the endpoint `zenmon` should connect to.
struct Hub {
    session: zenoh::Session,
    endpoint: String,
}

impl Hub {
    fn open() -> Hub {
        Hub::open_as("peer")
    }

    /// A router hub, for what only routers answer (the admin space's
    /// declaration lists).
    fn open_router() -> Hub {
        Hub::open_as("router")
    }

    fn open_as(mode: &str) -> Hub {
        let mut config = zenoh::Config::default();
        let mode = format!("\"{mode}\"");
        for (key, value) in [
            ("mode", mode.as_str()),
            ("listen/endpoints", r#"["tcp/127.0.0.1:0"]"#),
            ("scouting/multicast/enabled", "false"),
            ("scouting/gossip/enabled", "false"),
            // zenohd serves its admin space by default; a library session
            // does not.
            ("adminspace/enabled", "true"),
            ("adminspace/permissions/read", "true"),
        ] {
            config.insert_json5(key, value).expect("hub config");
        }
        let session = zenoh::open(config).wait().expect("open hub session");
        let endpoint = session
            .info()
            .locators()
            .wait()
            .into_iter()
            .map(|l| l.to_string())
            .find(|l| l.starts_with("tcp/"))
            .expect("hub has a tcp locator");
        Hub { session, endpoint }
    }

    /// `zenmon` pointed at this hub: client mode, no scouting beyond it.
    fn zenmon(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_zenmon"));
        cmd.args([
            "-m",
            "client",
            "-e",
            &self.endpoint,
            "--connect-timeout",
            "10s",
        ])
        .args(args);
        cmd
    }

    /// Put `payload(n)` on `key` every 50 ms until the returned guard drops.
    fn publish_until_dropped(
        &self,
        key: &str,
        payload: impl Fn(u64) -> String + Send + 'static,
    ) -> Publishing {
        let stop = Arc::new(AtomicBool::new(false));
        let session = self.session.clone();
        let key = key.to_string();
        let flag = stop.clone();
        let handle = thread::spawn(move || {
            let mut n = 0;
            while !flag.load(Ordering::Relaxed) {
                session.put(&key, payload(n)).wait().expect("hub put");
                n += 1;
                thread::sleep(Duration::from_millis(50));
            }
        });
        Publishing {
            stop,
            handle: Some(handle),
        }
    }
}

struct Publishing {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for Publishing {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// A key prefix no other test (or concurrent run) shares.
fn prefix(test: &str) -> String {
    format!("zenmon-e2e/{}/{}", std::process::id(), test)
}

fn run(mut cmd: Command) -> Output {
    let out = cmd.output().expect("run zenmon");
    assert!(
        out.status.success(),
        "zenmon failed ({}):\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn ndjson(out: &Output) -> Vec<Value> {
    String::from_utf8(out.stdout.clone())
        .expect("stdout utf8")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect()
}

fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zenmon-e2e-{}-{}", std::process::id(), test));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn sub_streams_published_messages_as_ndjson() {
    let hub = Hub::open();
    let p = prefix("sub");
    let _traffic = hub.publish_until_dropped(&format!("{p}/state"), |n| format!(r#"{{"n":{n}}}"#));

    let sel = format!("{p}/**");
    let out = run(hub.zenmon(&["--json", "sub", &sel, "--count", "3", "--duration", "20s"]));
    let lines = ndjson(&out);

    assert_eq!(lines.len(), 3, "{lines:?}");
    for line in &lines {
        assert_eq!(line["key_expr"], format!("{p}/state"), "{line}");
        assert!(
            line["payload"]["n"].is_u64(),
            "payload decoded as JSON: {line}"
        );
    }
}

#[test]
fn pub_reaches_a_subscriber_with_its_attachment() {
    let hub = Hub::open();
    let p = prefix("pub");
    let key = format!("{p}/cmd");
    let sub = hub
        .session
        .declare_subscriber(&key)
        .wait()
        .expect("hub subscriber");

    // `--rate` rather than a single put: the hub's interest may not have
    // reached the client yet when a one-shot put fires.
    run(hub.zenmon(&[
        "pub",
        &key,
        r#"{"v":0.3}"#,
        "--att",
        r#"{"correlation_id":"e2e-1"}"#,
        "--rate",
        "20",
        "--duration",
        "3s",
    ]));

    let sample = sub
        .recv_timeout(Duration::from_secs(5))
        .expect("subscriber channel")
        .expect("no message reached the hub");
    let payload: Value =
        serde_json::from_str(&sample.payload().try_to_string().expect("utf8 payload"))
            .expect("JSON payload");
    assert_eq!(payload["v"], 0.3);
    let att = sample.attachment().expect("attachment present");
    let att: Value = serde_json::from_str(&att.try_to_string().expect("utf8 attachment"))
        .expect("JSON attachment");
    assert_eq!(att["correlation_id"], "e2e-1");
}

#[test]
fn query_returns_the_queryable_reply() {
    let hub = Hub::open();
    let p = prefix("query");
    let key = format!("{p}/params");
    let reply_key = key.clone();
    let _queryable = hub
        .session
        .declare_queryable(&key)
        .callback(move |q| {
            q.reply(&reply_key, r#"{"ok":true}"#).wait().expect("reply");
        })
        .wait()
        .expect("hub queryable");

    let out = run(hub.zenmon(&["--json", "query", &key, "--timeout", "5s"]));
    let v: Value = serde_json::from_slice(&out.stdout).expect("query emits one JSON object");

    assert_eq!(v["count"], 1, "{v}");
    assert_eq!(v["items"][0]["key_expr"], key, "{v}");
    assert_eq!(v["items"][0]["payload"]["ok"], true, "{v}");
}

#[test]
fn capture_store_reads_back_offline() {
    for compress in ["none", "zstd"] {
        let hub = Hub::open();
        let p = prefix(&format!("capture-{compress}"));
        let _traffic =
            hub.publish_until_dropped(&format!("{p}/pose"), |n| format!(r#"{{"x":{n}}}"#));
        let dir = temp_dir(&format!("capture-{compress}"));
        let dir_s = dir.to_str().expect("utf8 temp path");

        let sel = format!("{p}/**");
        run(hub.zenmon(&[
            "capture",
            &sel,
            "--dir",
            dir_s,
            "--compress",
            compress,
            "--count",
            "5",
            "--duration",
            "20s",
        ]));

        // Offline: `trace read` opens no session, so no endpoint.
        let mut read = Command::new(env!("CARGO_BIN_EXE_zenmon"));
        read.args(["--json", "trace", "read", dir_s, "--key", &sel]);
        let out = run(read);
        // Records, then one `{"summary":{...}}` line.
        let mut lines = ndjson(&out);
        let summary = lines.pop().expect("summary line");
        assert_eq!(summary["summary"]["matched"], 5, "{compress}: {summary}");
        assert_eq!(lines.len(), 5, "{compress}: {lines:?}");
        for r in &lines {
            assert_eq!(r["key_expr"], format!("{p}/pose"), "{compress}: {r}");
            assert!(r["payload_base64"].is_string(), "{compress}: {r}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Plain mode logs at INFO by default; those lines belong on stderr; on stdout
/// they read as records to whatever consumes the stream.
#[test]
fn plain_sub_stdout_carries_only_messages() {
    let hub = Hub::open();
    let p = prefix("plain");
    let _traffic = hub.publish_until_dropped(&format!("{p}/state"), |n| format!(r#"{{"n":{n}}}"#));

    let sel = format!("{p}/**");
    let out = run(hub.zenmon(&["sub", &sel, "--count", "2", "--duration", "20s"]));
    let stdout = String::from_utf8(out.stdout).expect("utf8");

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    for line in lines {
        assert!(line.starts_with(&format!("{p}/state | ")), "{line}");
    }
}

/// A gap in a numbered publisher's sequence shows up inline as a `miss` line,
/// between the messages either side of it.
#[test]
fn detect_misses_reports_a_sequence_gap_inline() {
    use zenoh::sample::SourceInfo;

    let hub = Hub::open();
    let p = prefix("misses");
    let key = format!("{p}/a");
    let publisher = hub.session.declare_publisher(key.clone()).wait().unwrap();
    let id = publisher.id();

    // Keep re-sending the same gapped sequence (fresh numbers each round) until
    // zenmon has seen enough, since its subscription lands at an unknown time.
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let feeder = thread::spawn(move || {
        let mut base = 0u32;
        while !flag.load(Ordering::Relaxed) {
            for sn in [base + 1, base + 2, base + 6] {
                publisher
                    .put(sn.to_string())
                    .source_info(SourceInfo::new(id, sn))
                    .wait()
                    .unwrap();
            }
            base += 10;
            thread::sleep(Duration::from_millis(100));
        }
    });

    let sel = format!("{p}/**");
    let out = run(hub.zenmon(&[
        "--json",
        "sub",
        &sel,
        "--detect-misses",
        "--count",
        "6",
        "--duration",
        "20s",
    ]));
    stop.store(true, Ordering::Relaxed);
    feeder.join().unwrap();

    let lines = ndjson(&out);
    let misses: Vec<&Value> = lines.iter().filter(|l| l.get("miss").is_some()).collect();
    assert!(!misses.is_empty(), "{lines:?}");
    // Within a round the gap is 3 (2 → 6); between rounds it is 4 (6 → 11).
    for m in &misses {
        let n = m["miss"]["missed"].as_u64().unwrap();
        assert!(n == 3 || n == 4, "{m}");
        assert_eq!(m["miss"]["source_eid"], id.eid(), "{m}");
    }
    let messages = lines.iter().filter(|l| l.get("key_expr").is_some()).count();
    assert_eq!(messages, 6, "--count counts messages, not misses");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("missed "), "{stderr}");
}

/// A DELETE prints as such, not as an empty value, and `--kind` filters on it.
#[test]
fn sub_shows_deletes_and_filters_by_kind() {
    let hub = Hub::open();
    let p = prefix("kind");
    let key = format!("{p}/a");
    let session = hub.session.clone();
    let k = key.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let feeder = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            session.put(&k, "1").wait().unwrap();
            session.delete(&k).wait().unwrap();
            thread::sleep(Duration::from_millis(50));
        }
    });

    let sel = format!("{p}/**");
    let out = run(hub.zenmon(&[
        "sub",
        &sel,
        "--kind",
        "delete",
        "--count",
        "2",
        "--duration",
        "20s",
    ]));
    stop.store(true, Ordering::Relaxed);
    feeder.join().unwrap();

    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        [format!("{key} | DELETE"), format!("{key} | DELETE")],
        "{stdout}"
    );
}

/// `pub` says whether anyone was listening: a put to nobody still succeeds,
/// and only `matched` tells the two apart.
#[test]
fn pub_reports_whether_a_subscriber_matched() {
    let hub = Hub::open();
    let p = prefix("matched");
    let key = format!("{p}/cmd");

    let out = run(hub.zenmon(&["--json", "pub", &key, "1"]));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["matched"], false, "{v}");

    let _sub = hub.session.declare_subscriber(&key).wait().unwrap();
    let out = run(hub.zenmon(&["--json", "pub", &key, "1"]));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["matched"], true, "{v}");

    let out = run(hub.zenmon(&["pub", &format!("{p}/nobody"), "1"]));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no subscriber matches"), "{stderr}");
}

/// `--timestamp` shows wall-clock time and the stamping node, not raw HLC.
#[test]
fn sub_timestamp_is_readable() {
    let hub = Hub::open();
    let p = prefix("ts");
    let session = hub.session.clone();
    let key = format!("{p}/a");
    let k = key.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let feeder = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            session
                .put(&k, "1")
                .timestamp(session.new_timestamp())
                .wait()
                .unwrap();
            thread::sleep(Duration::from_millis(50));
        }
    });

    let sel = format!("{p}/**");
    let out = run(hub.zenmon(&[
        "sub",
        &sel,
        "--timestamp",
        "--count",
        "1",
        "--duration",
        "20s",
    ]));
    stop.store(true, Ordering::Relaxed);
    feeder.join().unwrap();

    let line = String::from_utf8(out.stdout).unwrap();
    let zid = hub.session.zid().to_string();
    // e.g. `[2026-10-11 13:22:15.123456 @5f3a…] zenmon-e2e/…/a | 1`
    assert!(line.starts_with("[20"), "{line}");
    assert!(
        line.contains(&format!("@{}]", &zid[..zid.len().min(16)])),
        "{line} / {zid}"
    );
    assert!(line.trim_end().ends_with(&format!("{key} | 1")), "{line}");
}

/// `--qos` carries the publisher's QoS choices through to the output.
#[test]
fn sub_qos_shows_the_publishers_settings() {
    use zenoh::qos::{CongestionControl, Priority};

    let hub = Hub::open();
    let p = prefix("qos");
    let session = hub.session.clone();
    let k = format!("{p}/a");
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let feeder = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            session
                .put(&k, "1")
                .priority(Priority::RealTime)
                .congestion_control(CongestionControl::Block)
                .express(true)
                .wait()
                .unwrap();
            thread::sleep(Duration::from_millis(50));
        }
    });

    let sel = format!("{p}/**");
    let out = run(hub.zenmon(&[
        "--json",
        "sub",
        &sel,
        "--qos",
        "--count",
        "1",
        "--duration",
        "20s",
    ]));
    stop.store(true, Ordering::Relaxed);
    feeder.join().unwrap();

    let line = &ndjson(&out)[0];
    assert_eq!(line["qos"]["priority"], "RealTime", "{line}");
    assert_eq!(line["qos"]["congestion_control"], "Block", "{line}");
    assert_eq!(line["qos"]["express"], true, "{line}");
    assert!(line["qos"]["reliability"].is_string(), "{line}");
}

/// `pub --congestion/--priority/--express` reach the wire as asked.
#[test]
fn pub_sends_with_the_requested_qos() {
    use zenoh::qos::{CongestionControl, Priority};

    let hub = Hub::open();
    let key = format!("{}/cmd", prefix("pubqos"));
    let sub = hub.session.declare_subscriber(&key).wait().unwrap();

    run(hub.zenmon(&[
        "pub",
        &key,
        "1",
        "--congestion",
        "block",
        "--priority",
        "real-time",
        "--express",
    ]));

    let sample = sub
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .expect("message reached the hub");
    assert_eq!(sample.congestion_control(), CongestionControl::Block);
    assert_eq!(sample.priority(), Priority::RealTime);
    assert!(sample.express());
}

/// `--validate` checks each payload against the contract and says what is
/// wrong, per message.
#[test]
fn sub_validate_reports_contract_violations() {
    let hub = Hub::open();
    let p = prefix("validate");
    let dir = temp_dir("validate");
    let contract = dir.join("c.contract.yaml");
    std::fs::write(
        &contract,
        format!("topics:\n  - key: {p}/state\n    payload: {{ mode: idle|run, battery: u8 }}\n"),
    )
    .unwrap();

    let session = hub.session.clone();
    let k = format!("{p}/state");
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let feeder = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            session
                .put(&k, r#"{"mode":"flying","battery":300}"#)
                .wait()
                .unwrap();
            thread::sleep(Duration::from_millis(50));
        }
    });

    let sel = format!("{p}/**");
    let out = run(hub.zenmon(&[
        "--json",
        "--contract",
        contract.to_str().unwrap(),
        "sub",
        &sel,
        "--validate",
        "--count",
        "1",
        "--duration",
        "20s",
    ]));
    stop.store(true, Ordering::Relaxed);
    feeder.join().unwrap();

    let line = &ndjson(&out)[0];
    assert_eq!(line["payload_check"]["status"], "checked", "{line}");
    let v: Vec<&str> = line["payload_check"]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert!(
        v.contains(&"'mode' must be one of [idle, run], got 'flying'"),
        "{v:?}"
    );
    assert!(
        v.contains(&"'battery' = 300 is out of range for u8"),
        "{v:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `zenmon mcp` speaks MCP over stdio and its tools reach the network the
/// server was pointed at.
#[test]
fn mcp_server_lists_and_runs_tools() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;

    let hub = Hub::open();
    let p = prefix("mcp");
    let _traffic = hub.publish_until_dropped(&format!("{p}/state"), |n| format!(r#"{{"n":{n}}}"#));

    let mut child = hub
        .zenmon(&["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |msg: Value| -> Value {
        writeln!(stdin, "{msg}").unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };

    let init = ask(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" } }
    }));
    assert_eq!(init["result"]["serverInfo"]["name"], "zenmon", "{init}");

    let list = ask(serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"subscribe") && !names.contains(&"publish"),
        "{names:?}"
    );

    let call = ask(serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "subscribe",
                    "arguments": { "key_expr": format!("{p}/**"), "count": 2, "duration": "20s" } }
    }));
    assert_eq!(call["result"]["isError"], false, "{call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert_eq!(lines[0]["key_expr"], format!("{p}/state"));

    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// `admin decls` answers "is anything subscribed to this?" from the router's
/// admin space, naming who declared it.
#[test]
fn admin_decls_names_who_subscribes_to_a_key() {
    let hub = Hub::open_router();
    let p = prefix("admin");

    // A subscriber, held by a separate zenmon client for the duration.
    let mut sub = hub
        .zenmon(&["sub", &format!("{p}/**"), "--duration", "20s"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let key = format!("{p}/robot/pose");
    let mut found = Value::Null;
    for _ in 0..40 {
        let out = run(hub.zenmon(&["--json", "admin", "decls", &key, "--kind", "subscriber"]));
        found = serde_json::from_slice(&out.stdout).unwrap();
        if found["count"] == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    sub.kill().unwrap();
    let _ = sub.wait();

    assert_eq!(found["count"], 1, "{found}");
    let d = &found["items"][0];
    assert_eq!(d["kind"], "subscriber");
    assert_eq!(d["key_expr"], format!("{p}/**"));
    assert_eq!(d["router"], hub.session.zid().to_string());
    assert_eq!(d["clients"].as_array().unwrap().len(), 1, "{d}");

    let out = run(hub.zenmon(&[
        "admin",
        "decls",
        &format!("{p}/nobody/**"),
        "--kind",
        "subscriber",
    ]));
    assert!(String::from_utf8_lossy(&out.stdout).contains("No router reports"));
}

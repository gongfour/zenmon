//! `zenmon scenario`: a correlated, bounded diagnostic session.

use std::time::Duration;

use zenmon_core::config::ZenmonConfig;
use zenmon_core::contract::Contract;
use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::ScenarioArgs;
use crate::common::{
    internal_err, load_contract_opt, resolve_payload_arg, warn_redundant_namespace,
};
use crate::watch;

pub async fn run(ctx: &Ctx, config: ZenmonConfig, args: ScenarioArgs) -> Result<(), ZenmonError> {
    let ScenarioArgs {
        observe,
        preset,
        prefix,
        pub_,
        task,
        pub_rate,
        pub_for,
        pub_count,
        for_,
        settle,
        track,
        no_timeline,
        explain,
    } = args;
    // A resolved contract lets --task surface & validate its request schema.
    let contract = load_contract_opt(&ctx.contract)?;
    run_scenario(
        ctx.json,
        &config,
        observe,
        preset,
        prefix,
        pub_,
        task,
        pub_rate,
        pub_for,
        pub_count,
        for_,
        settle,
        track,
        no_timeline,
        explain,
        contract,
    )
    .await?;
    Ok(())
}

/// Build an observed [`ScenarioEvent`] from a received message, decoding its
/// payload and attachment. `trigger` is always false — the synthetic trigger
/// event is built separately.
fn observed_event(
    msg: zenmon_core::types::ZenohMessage,
    t_rel_ms: u64,
) -> zenmon_core::scenario::ScenarioEvent {
    zenmon_core::scenario::ScenarioEvent {
        t_rel_ms,
        attachment: msg.attachment.as_ref().map(|a| a.to_view()),
        encoding: msg.encoding.clone(),
        kind: msg.kind.clone(),
        payload: msg.payload.to_view(),
        key_expr: msg.key_expr,
        trigger: false,
    }
}

/// Build the synthetic trigger event at `t_rel_ms = 0` — the scenario's own
/// actuation/request that caused the episode. `value` is parsed as JSON for the
/// payload view, falling back to a plain string.
fn make_trigger_event(key: &str, value: &str) -> zenmon_core::scenario::ScenarioEvent {
    zenmon_core::scenario::ScenarioEvent {
        t_rel_ms: 0,
        key_expr: key.to_string(),
        attachment: None,
        encoding: String::new(),
        kind: "PUT".to_string(),
        payload: serde_json::from_str(value)
            .unwrap_or_else(|_| serde_json::Value::String(value.to_string())),
        trigger: true,
    }
}

/// The `scenario` orchestration: resolve observed keys, subscribe, optionally
/// trigger a `--pub`/`--task`, observe a bounded window (early-exiting on the
/// task response), then build and print the episode.
#[allow(clippy::too_many_arguments)]
async fn run_scenario(
    json: bool,
    config: &zenmon_core::config::ZenmonConfig,
    observe: Vec<String>,
    preset: Option<String>,
    prefix: String,
    mut pub_: Option<Vec<String>>,
    mut task: Option<Vec<String>>,
    pub_rate: Option<f64>,
    pub_for: Option<Duration>,
    pub_count: Option<u64>,
    for_: Duration,
    settle: Option<Duration>,
    track: Vec<String>,
    no_timeline: bool,
    explain: bool,
    contract: Option<Contract>,
) -> Result<(), ZenmonError> {
    use std::time::Instant;
    use zenmon_core::scenario::{
        build_episode, build_events_from_tracks, build_tracks, expand_preset, EndedReason,
        ScenarioEvent, ScenarioMeta, TrackSpec, TriggerInfo,
    };

    // Parse --track KEY:FIELD specs up front so a malformed one fails before we
    // open a session.
    let mut specs: Vec<TrackSpec> = Vec::with_capacity(track.len());
    for t in &track {
        let (key, field) = t.split_once(':').ok_or_else(|| {
            ZenmonError::invalid_input(format!("--track must be KEY:FIELD, got '{}'", t))
        })?;
        if key.is_empty() || field.is_empty() {
            return Err(ZenmonError::invalid_input(format!(
                "--track KEY and FIELD must be non-empty, got '{}'",
                t
            )));
        }
        specs.push(TrackSpec {
            key: key.to_string(),
            field: field.to_string(),
        });
    }

    // Resolve @file / stdin payloads for the trigger before anyone reads them.
    if let Some(p) = pub_.as_mut() {
        p[1] = resolve_payload_arg(&p[1])?;
    }
    if let Some(p) = task.as_mut() {
        p[1] = resolve_payload_arg(&p[1])?;
    }

    // If a contract is resolved and we're triggering a --task, surface its
    // request schema (so the caller need not read source to build the request)
    // and lightly validate the provided request. Display-only, never fatal.
    if let (Some(pair), Some(c)) = (&task, &contract) {
        if let Some(schema) = c.request_schema(&pair[0]) {
            eprintln!("# contract request schema for '{}':", pair[0]);
            for line in serde_json::to_string_pretty(&schema)
                .unwrap_or_default()
                .lines()
            {
                eprintln!("#   {}", line);
            }
            if let Ok(provided) = serde_json::from_str::<serde_json::Value>(&pair[1]) {
                for w in zenmon_core::contract::validate_against_schema(&schema, &provided) {
                    eprintln!("# ⚠ request: {}", w);
                }
            }
        }
    }

    // Resolve the observed key set: --observe, then --preset expansion, then the
    // two task-derived topics. Deduplicate while preserving order.
    let mut observed: Vec<String> = Vec::new();
    let push_unique = |set: &mut Vec<String>, k: String| {
        if !set.contains(&k) {
            set.push(k);
        }
    };
    for k in observe {
        push_unique(&mut observed, k);
    }
    if let Some(name) = &preset {
        let Some(c) = &contract else {
            return Err(ZenmonError::invalid_input(
                "--preset reads named topic sets from a contract: pass --contract or set ZENMON_CONTRACT",
            ));
        };
        let Some(keys) = c.presets.get(name).filter(|k| !k.is_empty()) else {
            let known: Vec<&str> = c.presets.keys().map(String::as_str).collect();
            return Err(ZenmonError::invalid_input(format!(
                "unknown --preset '{}' (contract declares: {})",
                name,
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            )));
        };
        for k in expand_preset(keys, &prefix) {
            push_unique(&mut observed, k);
        }
    }

    // A --task adds its feedback/response topics and defines a response key that
    // ends the scenario early.
    let mut response_key: Option<String> = None;
    let trigger: TriggerInfo = if let Some(pair) = &task {
        let task_prefix = pair[0].trim_end_matches('/').to_string();
        let request_json = pair[1].clone();
        let request_key = format!("{}/request", task_prefix);
        let feedback_key = format!("{}/feedback", task_prefix);
        let resp_key = format!("{}/response", task_prefix);
        push_unique(&mut observed, feedback_key);
        push_unique(&mut observed, resp_key.clone());
        response_key = Some(resp_key);
        TriggerInfo::Task {
            request_key,
            request_bytes: request_json.len(),
        }
    } else if let Some(pair) = &pub_ {
        TriggerInfo::Pub {
            key_expr: pair[0].clone(),
            bytes: pair[1].len(),
        }
    } else {
        TriggerInfo::None
    };

    if observed.is_empty() {
        return Err(ZenmonError::invalid_input(
            "no topics to observe: pass --observe or --preset",
        ));
    }

    {
        let mut keys: Vec<&str> = observed.iter().map(String::as_str).collect();
        if let Some(p) = &pub_ {
            keys.push(p[0].as_str());
        }
        if let Some(p) = &task {
            keys.push(p[0].as_str());
        }
        warn_redundant_namespace(keys, config.namespace.as_deref());
    }

    let trigger_desc = if let Some(p) = &task {
        format!("task -> {}/request", p[0].trim_end_matches('/'))
    } else if let Some(p) = &pub_ {
        format!("pub -> {}", p[0])
    } else {
        "none".to_string()
    };
    let track_specs: Vec<String> = specs
        .iter()
        .map(|s| format!("{}:{}", s.key, s.field))
        .collect();

    // --explain: print the resolved plan and stop before any network side effect.
    if explain {
        if json {
            let plan = serde_json::json!({
                "trigger": trigger_desc,
                "observed": observed,
                "tracks": track_specs,
                "for_ms": for_.as_millis() as u64,
                "settle_ms": settle.map(|s| s.as_millis() as u64).unwrap_or(0),
            });
            println!("{}", serde_json::to_string(&plan)?);
        } else {
            println!("scenario plan (dry run):");
            println!("  trigger: {trigger_desc}");
            println!("  observe ({}):", observed.len());
            for k in &observed {
                println!("    {k}");
            }
            if !track_specs.is_empty() {
                println!("  track: {}", track_specs.join(", "));
            }
            println!(
                "  window: {}ms (+{}ms settle)",
                for_.as_millis(),
                settle.map(|s| s.as_millis()).unwrap_or(0)
            );
        }
        return Ok(());
    }

    let session = zenmon_core::session::open_session(config).await?;

    // Subscribe to every observed key into one channel BEFORE triggering, so we
    // never miss the reaction to our own actuation.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut subscriptions = Vec::with_capacity(observed.len());
    for key in &observed {
        subscriptions.push(zenmon_core::subscriber::subscribe(&session, key, tx.clone()).await?);
    }
    // Drop our sender clone so the channel closes when all subscribers stop.
    drop(tx);

    if !json {
        eprintln!(
            "Recording scenario over {} topic(s) for {} ... (Ctrl+C to stop)",
            observed.len(),
            humantime::format_duration(for_)
        );
    }

    // The stopwatch defines t_rel_ms = 0; trigger fires right after so its
    // reaction is measured from the same origin.
    let stopwatch = Instant::now();
    let mut events: Vec<ScenarioEvent> = Vec::new();
    // A sustained --pub runs on a background task, bounded by --pub-for/--pub-count.
    let mut pub_task: Option<tokio::task::JoinHandle<()>> = None;

    // Fire the trigger and record it as the causal origin (t_rel_ms = 0). A
    // publish failure on a one-shot path is fatal.
    let trigger_ev: Option<ScenarioEvent> = match (&task, &pub_) {
        (Some(pair), _) => {
            let request_key = format!("{}/request", pair[0].trim_end_matches('/'));
            session
                .put(&request_key, pair[1].clone())
                .await
                .map_err(internal_err)?;
            Some(make_trigger_event(&request_key, &pair[1]))
        }
        (_, Some(pair)) => {
            let key = pair[0].clone();
            let value = pair[1].clone();
            match pub_rate {
                // Sustained: republish at a fixed rate on a background task so
                // the actuation persists through the observe window (defeats a
                // command watchdog), bounded by --pub-for/--pub-count.
                Some(hz) => {
                    let s = session.clone();
                    let bounds = watch::Bounds::new(pub_count, pub_for);
                    let (k, v) = (key.clone(), value.clone());
                    pub_task = Some(tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(crate::duration::rate_tick_interval(hz));
                        let mut budget = watch::Budget::start(bounds);
                        loop {
                            interval.tick().await;
                            if s.put(&k, v.clone()).await.is_err() {
                                break;
                            }
                            if budget.record() {
                                break;
                            }
                        }
                    }));
                }
                None => {
                    session
                        .put(&key, value.clone())
                        .await
                        .map_err(internal_err)?;
                }
            }
            Some(make_trigger_event(&key, &value))
        }
        (None, None) => None,
    };
    if let Some(te) = trigger_ev {
        events.push(te);
    }

    // Observe the window. Ends when the task response arrives OR --for elapses;
    // then observe --settle more. Ctrl+C ends early with what we have.
    let window_deadline = stopwatch + for_;
    let mut ended_reason = EndedReason::WindowElapsed;
    let mut settle_until: Option<Instant> = None;

    loop {
        // In the settle phase the effective deadline is the settle end; before
        // that it is the --for window end.
        let deadline = settle_until.unwrap_or(window_deadline);
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline.into()) => break,
            _ = tokio::signal::ctrl_c() => {
                if !json {
                    eprintln!("\nStopped.");
                }
                break;
            }
            item = rx.recv() => match item {
                Some(msg) => {
                    let t_rel_ms = stopwatch.elapsed().as_millis() as u64;
                    let is_response = response_key
                        .as_deref()
                        .is_some_and(|rk| rk == msg.key_expr);
                    events.push(observed_event(msg, t_rel_ms));
                    // First response ends the active window; then run --settle.
                    if is_response && settle_until.is_none() {
                        ended_reason = EndedReason::TaskResponse;
                        match settle {
                            Some(s) => settle_until = Some(Instant::now() + s),
                            None => break,
                        }
                    }
                }
                None => break,
            }
        }
    }

    // If --settle was requested but the window elapsed without a response, still
    // observe the extra settle time before finishing.
    if settle_until.is_none() {
        if let Some(s) = settle {
            let settle_end = Instant::now() + s;
            loop {
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(settle_end.into()) => break,
                    _ = tokio::signal::ctrl_c() => break,
                    item = rx.recv() => match item {
                        Some(msg) => {
                            let t_rel_ms = stopwatch.elapsed().as_millis() as u64;
                            events.push(observed_event(msg, t_rel_ms));
                        }
                        None => break,
                    }
                }
            }
        }
    }

    if let Some(h) = pub_task {
        h.abort();
    }
    // Deterministic teardown: each subscriber is undeclared before the session
    // closes, instead of being aborted and undeclared by drop glue.
    for subscription in subscriptions {
        subscription.stop().await;
    }
    session.close().await.map_err(internal_err)?;

    let meta = ScenarioMeta {
        trigger,
        for_ms: for_.as_millis() as u64,
        settle_ms: settle.map(|s| s.as_millis() as u64).unwrap_or(0),
        observed,
        ended_reason,
    };
    let mut episode = build_episode(&meta, &events);
    // One time-ordered list: value changes on tracked fields, plus what
    // timing alone shows (a key going quiet or stopping, a DELETE).
    // When observation actually ended — earlier than the window when a task
    // response cut it short, and a key cannot have "stopped" after that.
    let window_end = stopwatch.elapsed().as_millis() as u64;
    let mut all_events = zenmon_core::scenario::build_auto_events(&events, window_end);
    if !specs.is_empty() {
        episode["tracks"] = build_tracks(&events, &specs);
        if let Some(changes) = build_events_from_tracks(&episode["tracks"]).as_array() {
            all_events.extend(changes.iter().cloned());
        }
    }
    all_events.sort_by_key(|e| e["t_rel_ms"].as_u64().unwrap_or(0));
    if !all_events.is_empty() {
        episode["events"] = serde_json::Value::Array(all_events);
    }
    // Annotate each topics entry with its contract context (reusing enrich), so
    // the episode is self-describing: a reader learns what each topic *is* and
    // whether its encoding matches, and judges the values itself.
    if let Some(c) = &contract {
        if let Some(topics) = episode.get_mut("topics").and_then(|t| t.as_object_mut()) {
            for (key, entry) in topics.iter_mut() {
                let enc = events
                    .iter()
                    .find(|e| &e.key_expr == key && !e.trigger)
                    .map(|e| e.encoding.as_str())
                    .unwrap_or("");
                if let (Some(obj), Ok(v)) = (
                    entry.as_object_mut(),
                    serde_json::to_value(c.enrich(key, enc)),
                ) {
                    obj.insert("contract".to_string(), v);
                }
            }
        }
    }
    if no_timeline {
        if let Some(obj) = episode.as_object_mut() {
            obj.remove("timeline");
        }
    }

    if json {
        println!("{}", serde_json::to_string(&episode)?);
    } else {
        print_scenario_summary(&episode);
    }
    Ok(())
}

/// Compact human summary of an episode: trigger, per-topic counts, ended
/// reason.
fn print_scenario_summary(episode: &serde_json::Value) {
    let meta = &episode["meta"];
    let trigger = &meta["trigger"];
    let trigger_str = match trigger["kind"].as_str() {
        Some("task") => format!("task -> {}", trigger["request_key"].as_str().unwrap_or("")),
        Some("pub") => format!("pub -> {}", trigger["key_expr"].as_str().unwrap_or("")),
        _ => "none".to_string(),
    };
    println!("trigger:       {}", trigger_str);
    println!(
        "window:        {}ms (+{}ms settle)",
        meta["for_ms"].as_u64().unwrap_or(0),
        meta["settle_ms"].as_u64().unwrap_or(0)
    );
    println!(
        "messages:      {}",
        meta["message_count"].as_u64().unwrap_or(0)
    );
    println!(
        "ended:         {}",
        meta["ended_reason"].as_str().unwrap_or("")
    );

    if let Some(topics) = episode["topics"].as_object() {
        println!("topics:");
        let mut keys: Vec<&String> = topics.keys().collect();
        keys.sort();
        for k in keys {
            let t = &topics[k];
            println!(
                "  {:<50} {} msg (t {}..{}ms)",
                k,
                t["count"].as_u64().unwrap_or(0),
                t["first_t_rel_ms"].as_u64().unwrap_or(0),
                t["last_t_rel_ms"].as_u64().unwrap_or(0),
            );
        }
    }
}

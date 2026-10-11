//! `zenmon contract`: inspect a contract file offline.

use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::ContractCommand;
use crate::common::load_contract_required;

pub fn run(ctx: &Ctx, command: ContractCommand) -> Result<(), ZenmonError> {
    match command {
        ContractCommand::Lint { path } => {
            let contract = load_contract_required(path, &ctx.contract)?;
            let report = contract.lint();
            if ctx.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!(
                    "{} topics, {} endpoints, {} types, {} services",
                    report.topics, report.endpoints, report.types, report.services
                );
                if report.warnings.is_empty() {
                    println!("no warnings");
                } else {
                    println!("warnings ({}):", report.warnings.len());
                    for w in &report.warnings {
                        println!("  - {}", w);
                    }
                }
            }
        }

        ContractCommand::List { path } => {
            let contract = load_contract_required(path, &ctx.contract)?;
            if ctx.json {
                let items: Vec<_> = contract
                    .topics
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "key": t.key,
                            "pattern": t.pattern,
                            "encoding": contract.effective_encoding(t),
                            "endpoints": t.endpoints.len(),
                        })
                    })
                    .collect();
                println!("{}", zenmon_core::output::to_collection_json(&items)?);
            } else {
                for t in &contract.topics {
                    println!(
                        "{:<44} {:<10} {:<24} {}",
                        t.key,
                        t.pattern,
                        contract.effective_encoding(t),
                        t.endpoints.len()
                    );
                }
                println!("\n{} topic(s)", contract.topics.len());
            }
        }

        ContractCommand::Show { key, path } => {
            let contract = load_contract_required(path, &ctx.contract)?;
            let topic = contract.lookup(&key).ok_or_else(|| {
                ZenmonError::not_found(format!("'{}' is not declared in the contract", key))
            })?;
            let resolve =
                |v: &Option<serde_json::Value>| v.as_ref().map(|s| contract.resolve_refs(s));
            let payload = resolve(&topic.payload);
            let phases = resolve(&topic.phases);
            let request = resolve(&topic.request);
            let response = resolve(&topic.response);

            if ctx.json {
                let mut obj = serde_json::json!({
                    "key": topic.key,
                    "pattern": topic.pattern,
                    "encoding": contract.effective_encoding(topic),
                    "enveloped": contract.effective_enveloped(topic),
                    "producers": topic.producer_names(),
                    "consumers": topic.consumer_names(),
                });
                if !topic.endpoints.is_empty() {
                    obj["endpoints"] = serde_json::json!(topic
                        .endpoints
                        .iter()
                        .map(|e| {
                            let mut o = serde_json::json!({
                                "service": e.service,
                                "role": e.role,
                            });
                            if let Some(t) = &e.message_type {
                                o["type"] = serde_json::json!(t);
                            }
                            if let Some(q) = &e.qos {
                                o["qos"] = q.clone();
                            }
                            if let Some(g) = &e.origin {
                                o["origin"] = serde_json::json!(g);
                            }
                            o
                        })
                        .collect::<Vec<_>>());
                }
                if let Some(s) = &topic.status {
                    obj["status"] = serde_json::json!(s);
                }
                if let Some(d) = &topic.description {
                    obj["description"] = serde_json::json!(d);
                }
                for (k, v) in [
                    ("payload", &payload),
                    ("phases", &phases),
                    ("request", &request),
                    ("response", &response),
                ] {
                    if let Some(v) = v {
                        obj[k] = v.clone();
                    }
                }
                println!("{}", serde_json::to_string(&obj)?);
            } else {
                println!("key:         {}", topic.key);
                println!("pattern:     {}", topic.pattern);
                println!(
                    "encoding:    {} ({})",
                    contract.effective_encoding(topic),
                    if contract.effective_enveloped(topic) {
                        "enveloped"
                    } else {
                        "bare"
                    }
                );
                if let Some(s) = &topic.status {
                    println!("status:      {}", s);
                }
                let producers = topic.producer_names();
                if !producers.is_empty() {
                    println!("producers:   {}", producers.join(", "));
                }
                let consumers = topic.consumer_names();
                if !consumers.is_empty() {
                    println!("consumers:   {}", consumers.join(", "));
                }
                if !topic.endpoints.is_empty() {
                    println!("endpoints:");
                    for e in &topic.endpoints {
                        let ty = e.message_type.as_deref().unwrap_or("-");
                        let origin = e.origin.as_deref().unwrap_or("-");
                        println!("  {:<28} {:<13} {:<26} {}", e.service, e.role, ty, origin);
                        if let Some(q) = &e.qos {
                            println!("  {:<28} qos: {}", "", q);
                        }
                    }
                }
                if let Some(d) = &topic.description {
                    println!("description: {}", d);
                }
                for (label, v) in [
                    ("payload", &payload),
                    ("phases", &phases),
                    ("request", &request),
                    ("response", &response),
                ] {
                    if let Some(v) = v {
                        println!("{}:", label);
                        let pretty = serde_json::to_string_pretty(v).unwrap_or_default();
                        for line in pretty.lines() {
                            println!("  {}", line);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

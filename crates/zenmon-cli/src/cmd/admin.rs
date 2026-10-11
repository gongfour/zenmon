//! `zenmon admin`: what routers' admin space says — declarations and raw
//! entries.

use zenmon_core::admin::{self, Declaration, DECLARATION_KINDS};
use zenmon_core::config::ZenmonConfig;
use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::AdminCommand;
use crate::common::{internal_err, warn_redundant_namespace};

pub async fn run(
    ctx: &Ctx,
    config: ZenmonConfig,
    command: AdminCommand,
) -> Result<(), ZenmonError> {
    // A usage error must not cost a connection attempt first.
    if let AdminCommand::Get { selector, .. } = &command {
        admin::check_selector(selector)?;
    }
    let session = zenmon_core::session::open_session(&config).await?;
    let result = match command {
        AdminCommand::Decls {
            key_expr,
            kind,
            timeout,
        } => {
            warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
            let kinds: Vec<&str> = if kind.is_empty() {
                DECLARATION_KINDS.to_vec()
            } else {
                kind.iter().map(|k| k.as_str()).collect()
            };
            match admin::declarations(&session, &key_expr, &kinds, timeout).await {
                Ok(decls) => print_decls(ctx, &key_expr, &kinds, &decls),
                Err(e) => Err(e),
            }
        }
        AdminCommand::Get { selector, timeout } => {
            match admin::entries(&session, &selector, timeout).await {
                Ok(entries) => print_entries(ctx, &selector, &entries),
                Err(e) => Err(e),
            }
        }
    };
    session.close().await.map_err(internal_err)?;
    result
}

fn print_decls(
    ctx: &Ctx,
    key_expr: &str,
    kinds: &[&str],
    decls: &[Declaration],
) -> Result<(), ZenmonError> {
    if ctx.json {
        println!("{}", zenmon_core::output::to_collection_json(decls)?);
        return Ok(());
    }
    if decls.is_empty() {
        println!(
            "No router reports a {} on keys intersecting '{key_expr}'.",
            kinds.join("/")
        );
        println!("(Routers only: a peer-only network has no admin space to ask.)");
        return Ok(());
    }
    for d in decls {
        let by: Vec<String> = d
            .clients
            .iter()
            .map(|z| format!("client {}", short(z)))
            .chain(d.peers.iter().map(|z| format!("peer {}", short(z))))
            .collect();
        let by = if by.is_empty() {
            "(via other routers)".to_string()
        } else {
            by.join(", ")
        };
        println!(
            "{:<10} {:<48} {}  @router {}",
            d.kind,
            d.key_expr,
            by,
            short(&d.router)
        );
    }
    // The question this command exists for: is anyone on the receiving end?
    let wants_subscribers = kinds.contains(&"subscriber");
    if wants_subscribers && !decls.iter().any(|d| d.kind == "subscriber") {
        println!("note: no subscriber is declared on keys intersecting '{key_expr}'");
    }
    Ok(())
}

fn print_entries(
    ctx: &Ctx,
    selector: &str,
    entries: &[(String, serde_json::Value)],
) -> Result<(), ZenmonError> {
    if ctx.json {
        let items: Vec<serde_json::Value> = entries
            .iter()
            .map(|(k, v)| serde_json::json!({ "key_expr": k, "payload": v }))
            .collect();
        println!("{}", zenmon_core::output::to_collection_json(&items)?);
        return Ok(());
    }
    if entries.is_empty() {
        println!("No admin-space entries under '{selector}'.");
        return Ok(());
    }
    for (key, value) in entries {
        println!("{key}");
        let text = match value {
            serde_json::Value::String(s) => s.clone(),
            v => serde_json::to_string_pretty(v)?,
        };
        for line in text.lines() {
            println!("  {line}");
        }
    }
    Ok(())
}

fn short(zid: &str) -> &str {
    &zid[..zid.len().min(16)]
}

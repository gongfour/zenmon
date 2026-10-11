//! `zenmon query`: one Zenoh GET.

use zenmon_core::config::ZenmonConfig;
use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::QueryArgs;
use crate::common::{internal_err, warn_redundant_namespace};

pub async fn run(ctx: &Ctx, config: ZenmonConfig, args: QueryArgs) -> Result<(), ZenmonError> {
    let QueryArgs {
        key_expr,
        payload,
        timeout,
        limit,
        consolidation,
        since,
        until,
    } = args;
    warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
    let limit = limit.map(|n| n as usize);
    let history = since.is_some() || until.is_some();
    let selector =
        zenmon_core::query::with_time_range(&key_expr, since.as_deref(), until.as_deref())?;
    // Consolidation keeps one value per key — exactly what a history query
    // must not do.
    let consolidation = match consolidation {
        crate::cli::ConsolidationArg::Auto if history => crate::cli::ConsolidationArg::None,
        other => other,
    };
    let session = zenmon_core::session::open_session(&config).await?;
    let outcome = zenmon_core::query::get(
        &session,
        &selector,
        payload.as_deref(),
        timeout,
        limit,
        consolidation.into(),
    )
    .await?;
    let limited = limit.is_some_and(|l| outcome.replies.len() >= l);

    if ctx.json {
        println!(
            "{}",
            zenmon_core::output::to_query_json(&outcome.replies, &outcome.errors, limited)?
        );
    } else if outcome.replies.is_empty() && outcome.errors.is_empty() {
        println!("No replies for '{}'", key_expr);
    } else {
        for msg in &outcome.replies {
            let att_str = msg
                .attachment
                .as_ref()
                .map(|a| format!(" [att: {}]", a))
                .unwrap_or_default();
            // History is a time series: say when each value is from.
            let when = match (history, msg.timestamp.as_deref()) {
                (true, Some(raw)) => format!("[{}] ", zenmon_core::time::readable_hlc(raw)),
                _ => String::new(),
            };
            println!("{when}{} | {}{}", msg.key_expr, msg.payload, att_str);
        }
        for err in &outcome.errors {
            println!("error reply: {}", err.message);
        }
        println!(
            "\n{} reply(ies), {} error(s)",
            outcome.replies.len(),
            outcome.errors.len()
        );
    }
    session.close().await.map_err(internal_err)?;
    Ok(())
}

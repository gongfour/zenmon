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
    } = args;
    warn_redundant_namespace([key_expr.as_str()], config.namespace.as_deref());
    let limit = limit.map(|n| n as usize);
    let session = zenmon_core::session::open_session(&config).await?;
    let outcome = zenmon_core::query::get(
        &session,
        &key_expr,
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
            println!("{} | {}{}", msg.key_expr, msg.payload, att_str);
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

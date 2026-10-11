//! `zenmon config`: validate and show the merged configuration, offline.

use zenmon_core::config::{EffectiveConfig, ResolvedValue};
use zenmon_core::error::ZenmonError;

use super::Ctx;
use crate::cli::ConfigCommand;

pub fn run(
    ctx: &Ctx,
    effective: &EffectiveConfig,
    command: ConfigCommand,
) -> Result<(), ZenmonError> {
    match command {
        ConfigCommand::Validate => {
            if ctx.json {
                let output = serde_json::json!({
                    "valid": true,
                    "config": effective,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("Configuration is valid.");
            }
        }
        ConfigCommand::Show { effective: _ } => print_effective_config(effective, ctx.json)?,
    }
    Ok(())
}

fn print_resolved<T: std::fmt::Display>(label: &str, value: &ResolvedValue<T>) {
    println!(
        "{:<16} {} ({})",
        format!("{}:", label),
        value.value,
        value.source
    );
}

fn print_optional_resolved(label: &str, value: &ResolvedValue<Option<String>>) {
    let rendered = value.value.as_deref().unwrap_or("(none)");
    println!(
        "{:<16} {} ({})",
        format!("{}:", label),
        rendered,
        value.source
    );
}

fn print_effective_config(effective: &EffectiveConfig, json: bool) -> Result<(), ZenmonError> {
    if json {
        println!("{}", serde_json::to_string_pretty(effective)?);
    } else {
        print_resolved("Endpoint", &effective.endpoint);
        print_resolved("Mode", &effective.mode);
        print_optional_resolved("Namespace", &effective.namespace);
        let config_file = effective
            .config_file
            .value
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(none)".to_string());
        println!(
            "{:<16} {} ({})",
            "Config file:", config_file, effective.config_file.source
        );
        print_resolved("Scout port", &effective.scout_port);
        print_optional_resolved("Connect timeout", &effective.connect_timeout);
    }
    Ok(())
}

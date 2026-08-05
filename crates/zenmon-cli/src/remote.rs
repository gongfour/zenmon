//! `zenmon remote` — inspect and edit the release-remote registry.
//!
//! The registry itself (schema, validation, persistence) lives in
//! `zenmon_core::remotes`, because the tray app resolves the same file. This
//! module is only the command surface: argument shaping, and the two output
//! forms every zenmon command owes — a human table and a JSON document.

use serde::Serialize;
use zenmon_core::error::{Result, ZenmonError};
use zenmon_core::output;
use zenmon_core::remotes::{self, RemoteSpec, RemotesConfig};

use crate::cli::RemoteCommand;

/// One row of `remote list`.
#[derive(Serialize)]
struct RemoteRow {
    name: String,
    kind: String,
    location: String,
    default: bool,
    /// False for an entry whose `kind` this build cannot act on. Listed
    /// anyway — hiding it would make the file and the listing disagree.
    usable: bool,
}

pub fn run(command: RemoteCommand, json: bool) -> Result<()> {
    let path = remotes::config_path()?;
    let mut config = remotes::load_from(&path)?;

    match command {
        RemoteCommand::Add {
            name,
            github,
            path: dir,
            default,
        } => {
            // clap's ArgGroup guarantees exactly one of the two is present.
            let spec = match (github, dir) {
                (Some(repo), None) => RemoteSpec::Github { repo },
                (None, Some(path)) => RemoteSpec::Path { path },
                _ => {
                    return Err(ZenmonError::invalid_input(
                        "exactly one of --github or --path is required",
                    ))
                }
            };
            let replaced = config.remotes.contains_key(&name);
            config.add(&name, spec.clone(), default)?;
            remotes::save_to(&path, &config)?;

            if json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "ok": true,
                        "name": name,
                        "kind": spec.kind(),
                        "location": spec.location(),
                        "default": config.is_default(&name),
                        "replaced": replaced,
                    }))?
                );
            } else {
                let verb = if replaced { "updated" } else { "added" };
                println!("{verb} remote {name} -> {spec}");
                if config.is_default(&name) {
                    println!("{name} is now the default remote");
                }
            }
        }

        RemoteCommand::List => {
            let rows: Vec<RemoteRow> = config
                .remotes
                .iter()
                .map(|(name, entry)| RemoteRow {
                    name: name.clone(),
                    kind: entry.kind().to_owned(),
                    location: entry.location().to_owned(),
                    default: config.is_default(name),
                    usable: entry.known().is_some(),
                })
                .collect();

            if json {
                println!("{}", output::to_collection_json(&rows)?);
            } else {
                print_table(&rows, &config, &path);
            }
        }

        RemoteCommand::Remove { name } => {
            config.remove(&name)?;
            remotes::save_to(&path, &config)?;

            if json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "ok": true,
                        "name": name,
                        "default": config.default,
                    }))?
                );
            } else {
                println!("removed remote {name}");
                match config.default.as_deref() {
                    Some(next) => println!("default remote is now {next}"),
                    // Worth saying out loud: the next bare `zenmon update`
                    // will refuse rather than pick for them.
                    None if !config.is_empty() => println!(
                        "no default remote is set; pass --remote or run \
                         `zenmon remote default <name>`"
                    ),
                    None => {}
                }
            }
        }

        RemoteCommand::Default { name } => {
            config.set_default(&name)?;
            remotes::save_to(&path, &config)?;

            if json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "ok": true,
                        "default": name,
                    }))?
                );
            } else {
                println!("default remote is now {name}");
            }
        }
    }

    Ok(())
}

fn print_table(rows: &[RemoteRow], config: &RemotesConfig, path: &std::path::Path) {
    if rows.is_empty() {
        // An empty registry is not a broken one — `update` falls back to the
        // built-in repository. Saying which one avoids the impression that
        // nothing works until something is added.
        println!("no remotes configured ({})", path.display());
        println!(
            "`zenmon update` will use the built-in default: github {}",
            remotes::BUILTIN_REPO
        );
        return;
    }

    let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let kind_width = rows.iter().map(|r| r.kind.len()).max().unwrap_or(4).max(4);

    println!("{}", path.display());
    println!("  {:name_width$}  {:kind_width$}  LOCATION", "NAME", "KIND");
    for row in rows {
        // The marker column carries the default; a "default" column of mostly
        // `false` would cost more width than it explains.
        let marker = if row.default { '*' } else { ' ' };
        println!(
            "{marker} {:name_width$}  {:kind_width$}  {}",
            row.name, row.kind, row.location
        );
    }

    if config.default.is_none() {
        println!();
        println!("no default remote is set; pass --remote or run `zenmon remote default <name>`");
    }

    if rows.iter().any(|row| !row.usable) {
        println!();
        println!(
            "some remotes have a kind this zenmon does not understand; they were \
             added by a newer version and are left untouched"
        );
    }
}

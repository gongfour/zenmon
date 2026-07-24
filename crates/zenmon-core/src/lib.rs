//! Zenoh network monitoring and debugging primitives.
//!
//! `zenmon-core` is the library behind the `zenmon` CLI and TUI, and is meant to
//! be linked directly by other applications — desktop monitors, test harnesses,
//! CI probes. It is deliberately **project-neutral**: it knows about Zenoh
//! sessions, key expressions, samples, queries and liveliness, and nothing about
//! any particular deployment's services, topics, presence rules or health
//! policy. Domain knowledge belongs in the consumer, not here.
//!
//! # Errors
//!
//! Every fallible function in this crate returns [`ZenmonError`], a `thiserror`
//! type carrying a stable, machine-readable [`ErrorKind`]. No application-level
//! reporting crate (`color_eyre`, `anyhow`, ...) appears anywhere in the public
//! API, so linking `zenmon-core` does not force an error-handling stack on the
//! consumer. `ZenmonError` is a plain `std::error::Error + Send + Sync +
//! 'static`, so `?` into `anyhow::Error`, `eyre::Report` or `Box<dyn Error>`
//! works with no glue code.
//!
//! # Getting started
//!
//! ```no_run
//! use std::time::Duration;
//! use zenmon_core::query::{self, ConsolidationMode};
//! use zenmon_core::types::ZenohMessage;
//! use zenmon_core::{open_session, subscriber, ZenmonConfig, ZenmonError};
//!
//! # async fn demo() -> Result<(), ZenmonError> {
//! // 1. Open a session. `ZenmonConfig::default()` is a client on
//! //    tcp/localhost:7447; set the fields (or use `config::resolve_config`)
//! //    to point somewhere else.
//! let session = open_session(&ZenmonConfig::default()).await?;
//!
//! // 2. Subscribe. Samples are decoded into `ZenohMessage` and pushed to the
//! //    channel by a background task; hold the handle to keep it alive.
//! let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ZenohMessage>();
//! let _pump = subscriber::subscribe(&session, "demo/**", tx).await?;
//! while let Some(msg) = rx.recv().await {
//!     println!("{} {}", msg.key_expr, msg.payload.pretty());
//! }
//!
//! // 3. Query. One round trip, bounded by an explicit timeout.
//! let outcome = query::get(
//!     &session,
//!     "demo/status",
//!     None,
//!     Duration::from_secs(2),
//!     None,
//!     ConsolidationMode::None,
//! )
//! .await?;
//! println!("{} replies, {} errors", outcome.replies.len(), outcome.errors.len());
//! # Ok(())
//! # }
//! ```
//!
//! # Public surface and stability
//!
//! Everything below is `pub`, because `zenmon-cli` and `zenmon-tui` are separate
//! crates in this workspace and between them consume every module — `pub(crate)`
//! is not available as a way to hide anything. The distinction is therefore
//! documented rather than enforced, and it is the contract external consumers
//! should rely on:
//!
//! - **Stable surface.** [`config`], [`error`], [`session`], [`subscriber`],
//!   [`query`], [`queryable`], [`discover`], [`scout`], [`registry`], [`info`],
//!   [`keyexpr`], [`types`]. These are general-purpose Zenoh primitives; they
//!   are what an external application is expected to build on, and they will not
//!   change shape without a version bump.
//!
//! - **Support surface.** [`doctor`], [`merge`], [`nodediff`], [`topology`],
//!   [`capture`], [`trace`]. Useful outside zenmon (connectivity checks, node
//!   set merging/diffing, capture files) but shaped by zenmon's own needs, so
//!   expect more churn than the stable surface.
//!
//! - **zenmon application internals.** [`output`], [`contract`], [`scenario`].
//!   These encode zenmon's own CLI behaviour — the stdout JSON envelope shapes,
//!   the topic-contract YAML format, the `zenmon scenario` episode/track
//!   aggregation and its named presets. They are `pub` only because
//!   `zenmon-cli` needs them across the crate boundary. Treat them as private:
//!   they are exempt from any stability promise and are the intended candidates
//!   for moving into `zenmon-cli` outright.

// ---------------------------------------------------------------------------
// Stable surface — general-purpose Zenoh primitives.
// ---------------------------------------------------------------------------

/// Connection settings and their resolution from file / environment / flags.
pub mod config;
/// Topic and liveliness discovery.
pub mod discover;
pub mod error;
/// Session-level facts: zid, mode, connected routers and peers.
pub mod info;
pub mod keyexpr;
/// Zenoh GET queries and their replies.
pub mod query;
pub mod queryable;
/// Zenoh admin-space node inventory.
pub mod registry;
/// Multicast scouting, including port-range scans.
pub mod scout;
/// Opening a Zenoh session from a `ZenmonConfig`.
pub mod session;
/// Subscribing to a key expression.
pub mod subscriber;
/// Data types crossing the API boundary: messages, payloads, nodes, tokens.
pub mod types;

// ---------------------------------------------------------------------------
// Support surface — reusable, but shaped by zenmon's own needs.
// ---------------------------------------------------------------------------

pub mod capture;
pub mod doctor;
/// Merging admin-space and scouted node sets.
pub mod merge;
pub mod nodediff;
pub mod topology;
pub mod trace;

// ---------------------------------------------------------------------------
// zenmon application internals — `pub` only for `zenmon-cli`. No stability
// promise; see the crate-level docs above.
// ---------------------------------------------------------------------------

pub mod contract;
pub mod output;
pub mod scenario;

// ---------------------------------------------------------------------------
// Convenience re-exports for the common path.
// ---------------------------------------------------------------------------

pub use crate::config::{ConnectMode, ZenmonConfig};
pub use crate::error::{ErrorKind, Result, ZenmonError};
pub use crate::session::open_session;

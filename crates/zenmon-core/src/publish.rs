//! Publishing a payload to a key expression.
//!
//! Lifted out of the CLI so the TUI publishes through exactly the same call —
//! the same pairing `subscriber` already has with the `sub` command. Two
//! implementations of "put a value on a key" would eventually disagree about
//! attachments or error mapping, and the one behind a UI is the one nobody
//! would notice drifting.

use zenoh::Session;

use crate::error::{Result, ZenmonError};

/// Put `value` on `key_expr`, optionally with an attachment.
///
/// One shot. Repeating at a rate is the caller's business: the CLI spends a
/// count/duration budget, and the TUI publishes once per confirmation.
pub async fn put(
    session: &Session,
    key_expr: &str,
    value: Vec<u8>,
    attachment: Option<&[u8]>,
) -> Result<()> {
    let mut builder = session.put(key_expr, value);
    if let Some(bytes) = attachment {
        builder = builder.attachment(bytes.to_vec());
    }
    builder
        .await
        .map_err(|e| ZenmonError::internal(e.to_string()))
}

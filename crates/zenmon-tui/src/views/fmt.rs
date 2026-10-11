//! Shared display formatting for the views.
//!
//! Lives outside any single view so a view can be deleted without taking a
//! formatter its neighbours depend on with it.

/// Render a zenoh HLC timestamp as a readable `YYYY-MM-DD HH:MM:SS.ffffff`
/// (the same formatting `zenmon sub --timestamp` uses). Unparseable input is
/// passed through verbatim.
pub(crate) fn format_stream_timestamp(raw: &str) -> String {
    zenmon_core::time::readable_hlc(raw)
}

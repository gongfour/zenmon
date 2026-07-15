use crate::config::ConnectMode;
use crate::types::SessionDetail;
use color_eyre::Result;
use zenoh::Session;

/// Get detailed information about the current session.
///
/// `mode` is the *configured* connection mode. We report it verbatim rather
/// than guessing from router presence, which previously misclassified a peer
/// connected to a router (as "client") and a disconnected client (as "peer").
pub async fn session_info(session: &Session, mode: ConnectMode) -> Result<SessionDetail> {
    let zid = format!("{}", session.info().zid().await);

    let mut routers = Vec::new();
    let mut router_iter = session.info().routers_zid().await;
    while let Some(rid) = router_iter.next() {
        routers.push(format!("{}", rid));
    }

    let mut peers = Vec::new();
    let mut peer_iter = session.info().peers_zid().await;
    while let Some(pid) = peer_iter.next() {
        peers.push(format!("{}", pid));
    }

    let mode = match mode {
        ConnectMode::Client => "client".to_string(),
        ConnectMode::Peer => "peer".to_string(),
    };
    let connected = !routers.is_empty() || !peers.is_empty();

    Ok(SessionDetail {
        zid,
        mode,
        connected,
        routers,
        peers,
    })
}

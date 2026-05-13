use crate::types::SessionDetail;
use color_eyre::Result;
use zenoh::Session;

/// Get detailed information about the current session.
pub async fn session_info(session: &Session) -> Result<SessionDetail> {
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

    let mode = if !routers.is_empty() {
        "client".to_string()
    } else {
        "peer".to_string()
    };

    Ok(SessionDetail {
        zid,
        mode,
        routers,
        peers,
    })
}

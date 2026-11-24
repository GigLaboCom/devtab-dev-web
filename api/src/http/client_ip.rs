//! The client's address, for rate limits only: it is kept in the limiters'
//! memory for minutes and never written to a table or a log.
//!
//! In production the proxy in front sets `X-Forwarded-For` to the address it
//! saw, so the last entry is the client. Without the header, the TCP peer.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let forwarded = parts
            .headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .filter_map(|s| s.trim().parse::<IpAddr>().ok())
            .next_back();
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());
        Ok(Self(
            forwarded
                .or(peer)
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;

    use super::*;

    async fn ip_of(request: Request<()>) -> IpAddr {
        let (mut parts, ()) = request.into_parts();
        let Ok(ClientIp(ip)) = ClientIp::from_request_parts(&mut parts, &()).await;
        ip
    }

    #[tokio::test]
    async fn the_last_forwarded_address_wins_then_the_peer() {
        let request = Request::builder()
            .header("x-forwarded-for", "203.0.113.9, 198.51.100.7")
            .body(())
            .unwrap();
        assert_eq!(
            ip_of(request).await,
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );

        let mut request = Request::builder().body(()).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 4000))));
        assert_eq!(ip_of(request).await, "192.0.2.1".parse::<IpAddr>().unwrap());

        let request = Request::builder()
            .header("x-forwarded-for", "not an address")
            .body(())
            .unwrap();
        assert_eq!(ip_of(request).await, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
}

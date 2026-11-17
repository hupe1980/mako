//! Where a webhook endpoint registered through the API may point.
//!
//! The delivery worker POSTs from inside the deployment's network, so an
//! endpoint URL a caller supplies must not reach infrastructure the operator
//! never named: a loopback, private, link-local or unique-local address. It is
//! checked twice — on registration, against the URL as written, and on each
//! delivery, against the address the name **resolves to** at that moment
//! ([`GuardedResolver`]): a name that resolved to a public address when it was
//! registered and to `10.0.0.5` now (DNS rebinding) is refused before a
//! connection is opened. Redirects are failures — the client follows none.
//!
//! The endpoint the service's own config names (`default`) is not checked: the
//! operator named it, and an in-cluster ERP adapter is a private address by
//! design.

use std::net::{IpAddr, SocketAddr};

/// Why a URL is not an acceptable endpoint; rendered as `endpoint_url_not_allowed`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct NotAllowed(pub String);

/// Check an endpoint URL as written: `https` (or `http` where `allow_loopback`
/// admits a developer's local receiver), and a host that is not a literal
/// loopback, private, link-local or otherwise non-routable address.
///
/// # Errors
///
/// [`NotAllowed`] naming the reason.
pub fn check_url(raw: &str, allow_loopback: bool) -> Result<reqwest::Url, NotAllowed> {
    let url = reqwest::Url::parse(raw).map_err(|e| NotAllowed(format!("not a valid URL: {e}")))?;
    match url.scheme() {
        "https" => {}
        "http" if allow_loopback => {}
        other => {
            return Err(NotAllowed(format!(
                "scheme {other:?} is not accepted: an endpoint is https"
            )));
        }
    }
    let Some(host) = url.host_str() else {
        return Err(NotAllowed("the URL has no host".to_owned()));
    };
    let bare = host.trim_end_matches('.');
    if bare.eq_ignore_ascii_case("localhost") || bare.to_ascii_lowercase().ends_with(".localhost") {
        return if allow_loopback {
            Ok(url)
        } else {
            Err(NotAllowed(
                "a loopback host is not an acceptable endpoint".to_owned(),
            ))
        };
    }
    // An IPv6 literal arrives bracketed (`[::1]`); a registered name never
    // parses as an address and is checked on the address it resolves to.
    let literal = bare.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = literal.parse::<IpAddr>()
        && !allowed(ip, allow_loopback)
    {
        return Err(NotAllowed(format!(
            "{ip} is a loopback, link-local, private or otherwise non-routable address"
        )));
    }
    Ok(url)
}

/// Whether a delivery may connect to `ip`.
#[must_use]
pub fn allowed(ip: IpAddr, allow_loopback: bool) -> bool {
    (allow_loopback && ip.is_loopback()) || is_globally_routable(ip)
}

/// Anything not plainly a public unicast address is refused, including the
/// IPv4-mapped IPv6 forms of private ranges, the usual way such a filter is
/// bypassed.
fn is_globally_routable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // 100.64.0.0/10 carrier-grade NAT
                || (o[0] == 100 && (64..128).contains(&o[1]))
                // 0.0.0.0/8 "this network"
                || o[0] == 0)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_globally_routable(IpAddr::V4(v4));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique-local
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // fe80::/10 link-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

/// The addresses `host` resolves to, or a refusal when none may be reached.
///
/// Every resolved address must be allowed: a name answering with one public
/// and one private address is refused rather than trimmed, because which of
/// the two a connection would use is not this check's choice.
///
/// # Errors
///
/// The lookup's error, or [`NotAllowed`] naming the refused address.
pub async fn resolve_allowed(
    host: &str,
    port: u16,
    allow_loopback: bool,
) -> Result<Vec<SocketAddr>, Box<dyn std::error::Error + Send + Sync>> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
    if let Some(refused) = addrs.iter().find(|a| !allowed(a.ip(), allow_loopback)) {
        return Err(Box::new(NotAllowed(format!(
            "{host} resolves to {}, a loopback, link-local, private or otherwise \
             non-routable address",
            refused.ip()
        ))));
    }
    if addrs.is_empty() {
        return Err(Box::new(NotAllowed(format!(
            "{host} resolves to no address"
        ))));
    }
    Ok(addrs)
}

/// A `reqwest` resolver refusing a name that resolves to an address
/// [`allowed`] refuses, so the check holds for the connection actually made.
#[derive(Debug, Clone, Copy)]
pub struct GuardedResolver {
    /// Admit loopback addresses (`[subscriptions] allow_loopback`).
    pub allow_loopback: bool,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let allow_loopback = self.allow_loopback;
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = resolve_allowed(&host, 0, allow_loopback).await?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The client deliveries to API-registered endpoints go through: the
/// resolved-address check, no redirects, a 10 s request timeout.
///
/// # Panics
///
/// Only if the TLS stack fails to initialise, which cannot happen with the
/// workspace's `reqwest` features.
#[must_use]
pub fn guarded_client(allow_loopback: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(std::sync::Arc::new(GuardedResolver { allow_loopback }))
        .build()
        .expect("reqwest guarded client: TLS initialisation is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_loopback_is_refused_at_registration() {
        for url in [
            "https://127.0.0.1/hook",
            "https://localhost/hook",
            "https://localhost./hook",
            "https://[::1]/hook",
            "https://169.254.169.254/latest/meta-data/",
            "https://10.0.0.5/hook",
            "https://[::ffff:192.168.1.10]/hook",
            "https://[fd00::1]/hook",
        ] {
            assert!(check_url(url, false).is_err(), "{url}");
        }
        assert!(check_url("https://erp.example.com/events", false).is_ok());
    }

    #[test]
    fn an_api_endpoint_is_https() {
        assert!(check_url("http://erp.example.com/events", false).is_err());
        assert!(check_url("file:///etc/passwd", false).is_err());
        assert!(check_url("http://127.0.0.1:8580/hook", true).is_ok());
        assert!(check_url("http://10.0.0.5/hook", true).is_err());
    }

    /// `localhost` passes no literal check once it is spelled as a name the
    /// registration check does not know; the address it resolves to is
    /// refused before a connection opens.
    #[tokio::test]
    async fn a_name_resolving_to_a_private_address_is_refused_at_delivery() {
        let refused = resolve_allowed("localhost", 443, false)
            .await
            .expect_err("localhost resolves to a loopback address");
        assert!(refused.to_string().contains("non-routable"), "{refused}");
        assert!(resolve_allowed("localhost", 443, true).await.is_ok());
    }

    /// The guarded client never opens a connection to a name resolving to a
    /// loopback address: the receiver listening there is not reached.
    #[tokio::test]
    async fn the_guarded_client_does_not_connect_to_a_refused_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept())
                .await
                .is_ok()
        });
        let err = guarded_client(false)
            .get(format!("http://localhost:{port}/"))
            .send()
            .await
            .expect_err("refused before connecting");
        let mut chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(s) = source {
            chain.push_str(": ");
            chain.push_str(&s.to_string());
            source = s.source();
        }
        assert!(chain.contains("non-routable"), "{chain}");
        assert!(
            !accepted.await.unwrap(),
            "a connection reached the receiver"
        );
    }
}

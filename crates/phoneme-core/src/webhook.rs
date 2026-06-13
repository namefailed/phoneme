//! Outbound webhook delivery, behind an SSRF guard.
//!
//! This module owns [`WebhookClient`], which POSTs a recording's [`HookPayload`]
//! as JSON to `hook.webhook_url`. The daemon's pipeline fires it alongside the
//! local hook ([`crate::hook`]) when one is configured.
//!
//! The load-bearing part is the guard, not the POST. Phoneme is local-first, so
//! the policy is three-tiered (`HostClass`): a webhook into THIS machine
//! (loopback) is the primary use case and always allowed; the LAN needs an
//! explicit opt-in; the public internet requires TLS. The target is validated —
//! including resolving a DNS name and classifying *every* address it yields —
//! before a single byte leaves the machine, and redirects are never followed, so
//! a mistyped or hostile URL can't bounce transcripts at an internal service
//! (S-H1).

use crate::config::WebhookConfig;
use crate::error::{Error, Result};
use crate::types::HookPayload;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

/// SSRF classification of a webhook target address (S-H1).
///
/// Phoneme is local-first, so the policy is deliberately three-tiered rather
/// than a blanket private-range block: webhooks into THIS machine are the
/// feature's primary job and stay open, the LAN needs an opt-in, and the
/// public internet needs TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostClass {
    /// 127.0.0.0/8, `::1`, or the literal `localhost` — always allowed, any
    /// scheme. Local n8n / Home Assistant / script servers live here.
    Loopback,
    /// Non-loopback private ranges: RFC1918 (10/8, 172.16/12, 192.168/16),
    /// link-local 169.254/16, IPv6 ULA fc00::/7, IPv6 link-local fe80::/10,
    /// and the unspecified addresses. Blocked unless
    /// `[webhook] allow_private_network = true`.
    Private,
    /// Everything else. HTTPS required unless `[webhook] allow_http = true`.
    Public,
}

fn classify_v4(ip: Ipv4Addr) -> HostClass {
    if ip.is_loopback() {
        HostClass::Loopback
    } else if ip.is_private() || ip.is_link_local() || ip.is_unspecified() {
        HostClass::Private
    } else {
        HostClass::Public
    }
}

fn classify_ip(ip: IpAddr) -> HostClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => {
            // An IPv4-mapped address (`::ffff:a.b.c.d`) reaches the v4 host it
            // wraps — classify the inner address so the mapping isn't a bypass.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return classify_v4(v4);
            }
            if v6.is_loopback() {
                HostClass::Loopback
            } else if v6.is_unique_local() || v6.is_unicast_link_local() || v6.is_unspecified() {
                HostClass::Private
            } else {
                HostClass::Public
            }
        }
    }
}

/// The class a resolved hostname gets: the most restrictive among its
/// addresses (`Private` > `Public` > `Loopback`). A name that resolves to ANY
/// private address is treated as private, and a name mixing loopback with
/// public addresses still gets the public-tier HTTPS check — the connection
/// could land on either.
fn classify_resolved(addrs: &[IpAddr]) -> HostClass {
    let mut class = HostClass::Loopback;
    for c in addrs.iter().map(|a| classify_ip(*a)) {
        match c {
            HostClass::Private => return HostClass::Private,
            HostClass::Public => class = HostClass::Public,
            HostClass::Loopback => {}
        }
    }
    class
}

/// Validate a webhook target against the `[webhook]` network policy BEFORE any
/// bytes leave the machine. The rules:
///
/// - **Loopback** (127.0.0.0/8, `::1`, the literal `localhost`) — always
///   allowed, any scheme, no knob can break this.
/// - **Private** (see [`HostClass::Private`]) — blocked unless
///   `[webhook] allow_private_network = true`.
/// - **Public** — must be `https` unless `[webhook] allow_http = true`.
///
/// A DNS hostname is resolved here and EVERY address it yields is classified
/// ([`classify_resolved`]), so a name pointing at a private IP is private.
/// `localhost` short-circuits to loopback without touching the resolver, and
/// the `url` parser canonicalizes IPv4 trickery (decimal/octal literals) into
/// dotted-quad form before classification.
async fn check_target(url: &str, policy: &WebhookConfig) -> Result<()> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| Error::InvalidConfig(format!("webhook URL {url:?} is invalid: {e}")))?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed
        .host_str()
        .ok_or_else(|| Error::InvalidConfig(format!("webhook URL {url:?} has no host")))?
        .to_string();

    // `host_str` wraps IPv6 literals in brackets; strip them for parsing.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let (class, resolved) = if host.eq_ignore_ascii_case("localhost") {
        (HostClass::Loopback, None)
    } else if let Ok(ip) = bare.parse::<IpAddr>() {
        (classify_ip(ip), None)
    } else {
        // The port only matters to the resolver call's shape, not the verdict.
        let port = parsed.port_or_known_default().unwrap_or(443);
        let addrs: Vec<IpAddr> = tokio::net::lookup_host((bare, port))
            .await
            .map_err(|e| {
                Error::InvalidConfig(format!("webhook host {host:?} did not resolve: {e}"))
            })?
            .map(|sa| sa.ip())
            .collect();
        if addrs.is_empty() {
            return Err(Error::InvalidConfig(format!(
                "webhook host {host:?} did not resolve to any address"
            )));
        }
        (classify_resolved(&addrs), Some(addrs))
    };

    match class {
        HostClass::Loopback => Ok(()),
        HostClass::Private => {
            if policy.allow_private_network {
                Ok(())
            } else {
                let what = match resolved
                    .as_deref()
                    .and_then(|a| a.iter().find(|ip| classify_ip(**ip) == HostClass::Private))
                {
                    Some(ip) => format!("{host} resolves to the private address {ip}"),
                    None => format!("{host} is a private network address"),
                };
                Err(Error::InvalidConfig(format!(
                    "webhook target {what}, which is blocked; set [webhook] \
                     allow_private_network = true in config.toml if this is intentional"
                )))
            }
        }
        HostClass::Public => {
            if scheme == "https" || policy.allow_http {
                Ok(())
            } else {
                Err(Error::InvalidConfig(format!(
                    "webhook target {host} is a public host reached over plain {scheme}; \
                     use an https:// URL, or set [webhook] allow_http = true in config.toml \
                     if this is intentional"
                )))
            }
        }
    }
}

/// HTTP client for delivering webhook POSTs, configured to never follow
/// redirects (so a 3xx can't bypass the SSRF guard).
#[derive(Clone)]
pub struct WebhookClient {
    http: reqwest::Client,
}

impl WebhookClient {
    /// Build a webhook client with a redirect-disabled HTTP client.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            // Never follow redirects: `check_target` classifies the URL the
            // user configured, so a 3xx from an allowed (public, https)
            // endpoint must not be able to bounce the POST into a private or
            // loopback service behind it. A redirect surfaces as `HookFailed`
            // with the 3xx status instead.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| {
                crate::error::Error::Internal(format!("Failed to build reqwest client: {e}"))
            })?;
        Ok(Self { http })
    }

    /// POST `payload` to `url` as JSON, after the `policy` SSRF guard passes.
    ///
    /// The guard runs first, so a blocked target never receives a packet.
    /// Returns [`Error::InvalidConfig`] when the target is disallowed by policy,
    /// [`Error::HookTimeout`] on a slow response, and [`Error::HookFailed`]
    /// (carrying the status and body) on a non-2xx answer — a 3xx included,
    /// since redirects are deliberately not followed.
    pub async fn post(
        &self,
        url: &str,
        timeout: Duration,
        payload: &HookPayload,
        policy: &WebhookConfig,
    ) -> Result<()> {
        // The SSRF guard runs first so a blocked target never sees a packet.
        check_target(url, policy).await?;
        let response = self
            .http
            .post(url)
            .timeout(timeout)
            .json(payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    Error::HookTimeout {
                        secs: timeout.as_secs(),
                    }
                } else {
                    Error::Internal(format!("webhook send failed: {e}"))
                }
            })?;
        if !response.status().is_success() {
            return Err(Error::HookFailed {
                code: response.status().as_u16() as i32,
                stderr_tail: response.text().await.unwrap_or_default(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HookMetadata;
    use crate::RecordingId;
    use chrono::Local;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_payload() -> HookPayload {
        HookPayload {
            id: RecordingId::new(),
            timestamp: Local::now(),
            transcript: "hello world".into(),
            audio_path: "C:/tmp/x.wav".into(),
            duration_ms: 1234,
            model: "test-model".into(),
            metadata: HookMetadata::current(),
        }
    }

    /// A 2xx response is success, and the client POSTs the payload exactly once
    /// to the given URL (verified by the `.expect(1)` on drop).
    #[tokio::test]
    async fn post_succeeds_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let url = format!("{}/hook", server.uri());
        client
            .post(
                &url,
                Duration::from_secs(5),
                &sample_payload(),
                &WebhookConfig::default(),
            )
            .await
            .expect("2xx must be Ok");
    }

    /// A non-2xx response maps to `HookFailed` carrying the status code and the
    /// response body (so the failure surfaces a useful reason).
    #[tokio::test]
    async fn post_maps_non_2xx_to_hook_failed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("upstream boom"))
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let err = client
            .post(
                &server.uri(),
                Duration::from_secs(5),
                &sample_payload(),
                &WebhookConfig::default(),
            )
            .await
            .expect_err("500 must be an error");
        match err {
            Error::HookFailed { code, stderr_tail } => {
                assert_eq!(code, 500);
                assert!(
                    stderr_tail.contains("upstream boom"),
                    "body should be carried, got: {stderr_tail}"
                );
            }
            other => panic!("expected HookFailed, got {other:?}"),
        }
    }

    /// A response slower than the per-request timeout maps to `HookTimeout`
    /// (not a generic error), so callers can distinguish a slow endpoint.
    #[tokio::test]
    async fn post_maps_timeout_to_hook_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let err = client
            .post(
                &server.uri(),
                Duration::from_millis(200),
                &sample_payload(),
                &WebhookConfig::default(),
            )
            .await
            .expect_err("a response slower than the timeout must error");
        assert!(
            matches!(err, Error::HookTimeout { .. }),
            "expected HookTimeout, got {err:?}"
        );
    }

    /// An unreachable endpoint maps to a (non-timeout) error rather than hanging
    /// or panicking. `allow_http` is set so the guard lets the public TEST-NET
    /// address through and the transport failure itself is what's exercised.
    #[tokio::test]
    async fn post_unreachable_host_errors() {
        let client = WebhookClient::new().unwrap();
        let policy = WebhookConfig {
            allow_http: true,
            ..Default::default()
        };
        // Reserved TEST-NET-1 address that should not accept connections.
        let err = client
            .post(
                "http://192.0.2.1:9/hook",
                Duration::from_secs(2),
                &sample_payload(),
                &policy,
            )
            .await
            .expect_err("unreachable host must error");
        // Either a connect error (Internal) or a timeout — both are acceptable;
        // the contract is "returns an error, doesn't hang/panic".
        assert!(matches!(
            err,
            Error::Internal(_) | Error::HookTimeout { .. }
        ));
    }

    // ── SSRF guard: classification ─────────────────────────────────────────

    /// The classification table: loopback v4/v6, every private range the guard
    /// blocks, and representative public addresses — including just-outside-
    /// range neighbours that must stay public.
    #[test]
    fn classifies_addresses_into_loopback_private_public() {
        use HostClass::*;
        let table: &[(&str, HostClass)] = &[
            ("127.0.0.1", Loopback),
            ("127.250.1.2", Loopback), // the whole /8, not just .1
            ("::1", Loopback),
            ("10.0.0.1", Private),
            ("172.16.0.1", Private),
            ("172.31.255.254", Private),
            ("192.168.1.1", Private),
            ("169.254.169.254", Private), // link-local (cloud metadata endpoint)
            ("fc00::1", Private),         // ULA, low half
            ("fd12:3456::1", Private),    // ULA, high half
            ("fe80::1", Private),         // IPv6 link-local
            ("::ffff:192.168.0.1", Private), // v4-mapped must not bypass
            ("0.0.0.0", Private),         // unspecified is never a sane target
            ("::", Private),
            ("8.8.8.8", Public),
            ("1.1.1.1", Public),
            ("172.32.0.1", Public),  // one past 172.16/12
            ("192.169.0.1", Public), // one past 192.168/16
            ("169.255.0.1", Public), // one past 169.254/16
            ("2001:4860:4860::8888", Public),
        ];
        for (s, want) in table {
            let ip: IpAddr = s.parse().unwrap();
            assert_eq!(classify_ip(ip), *want, "classification of {s}");
        }
    }

    /// A resolved name takes the most restrictive class among its addresses:
    /// any private address makes the name private; loopback mixed with public
    /// still gets the public HTTPS rule.
    #[test]
    fn resolved_names_take_the_most_restrictive_class() {
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let public: IpAddr = "8.8.8.8".parse().unwrap();
        let private: IpAddr = "10.0.0.1".parse().unwrap();
        assert_eq!(classify_resolved(&[lo]), HostClass::Loopback);
        assert_eq!(classify_resolved(&[lo, public]), HostClass::Public);
        assert_eq!(classify_resolved(&[public, private]), HostClass::Private);
        assert_eq!(classify_resolved(&[lo, private]), HostClass::Private);
    }

    // ── SSRF guard: policy ─────────────────────────────────────────────────

    /// Loopback (and the literal `localhost`, classified without a resolver
    /// round-trip) is ALWAYS allowed, any scheme, with both knobs off — local
    /// n8n / Home Assistant must keep working on a default config.
    #[tokio::test]
    async fn loopback_and_localhost_always_pass_any_scheme() {
        let policy = WebhookConfig::default();
        for url in [
            "http://127.0.0.1:9999/hook",
            "https://127.0.0.1/hook",
            "http://127.99.1.2/hook",
            "http://[::1]:8123/api/webhook/x",
            "http://localhost:5678/hook",
            "https://LOCALHOST/hook",
        ] {
            check_target(url, &policy)
                .await
                .unwrap_or_else(|e| panic!("{url} must be allowed: {e}"));
        }
    }

    /// Non-loopback private targets are blocked by default — over any scheme —
    /// and the error names the exact knob that opens them.
    #[tokio::test]
    async fn private_targets_blocked_by_default_with_actionable_error() {
        let policy = WebhookConfig::default();
        for url in [
            "http://10.0.0.5/hook",
            "https://192.168.1.50:5678/hook", // https does not bypass the gate
            "http://172.16.0.9/hook",
            "http://169.254.169.254/latest/meta-data",
            "http://[fd00::5]/hook",
            "http://[fe80::1]/hook",
        ] {
            let err = check_target(url, &policy)
                .await
                .expect_err(&format!("{url} must be blocked by default"));
            let msg = err.to_string();
            assert!(
                msg.contains("set [webhook] allow_private_network = true in config.toml"),
                "error must name the knob, got: {msg}"
            );
        }
    }

    /// `allow_private_network = true` opens private targets — and ONLY private
    /// targets; public http stays gated by `allow_http`.
    #[tokio::test]
    async fn allow_private_network_opens_private_targets() {
        let policy = WebhookConfig {
            allow_private_network: true,
            ..Default::default()
        };
        check_target("http://192.168.1.50:5678/hook", &policy)
            .await
            .expect("LAN target allowed once opted in");
        check_target("http://[fd00::5]/hook", &policy)
            .await
            .expect("ULA target allowed once opted in");
        assert!(
            check_target("http://8.8.8.8/hook", &policy).await.is_err(),
            "the private knob must not open public http"
        );
    }

    /// Public targets are https-only by default; `allow_http = true` opens
    /// plain http, and the default-deny error names that knob.
    #[tokio::test]
    async fn public_targets_require_https_unless_allow_http() {
        let policy = WebhookConfig::default();
        check_target("https://8.8.8.8/hook", &policy)
            .await
            .expect("public https is the default-allowed shape");
        let err = check_target("http://8.8.8.8/hook", &policy)
            .await
            .expect_err("public http must be blocked by default");
        let msg = err.to_string();
        assert!(
            msg.contains("set [webhook] allow_http = true in config.toml"),
            "error must name the knob, got: {msg}"
        );
        let open = WebhookConfig {
            allow_http: true,
            ..Default::default()
        };
        check_target("http://8.8.8.8/hook", &open)
            .await
            .expect("public http allowed once opted in");
    }

    /// `post` runs the guard before building a request, so a blocked target
    /// fails with the policy error (knob text and all), not a connect error.
    #[tokio::test]
    async fn post_enforces_the_guard_before_sending() {
        let client = WebhookClient::new().unwrap();
        let err = client
            .post(
                "http://10.255.0.1/hook",
                Duration::from_secs(2),
                &sample_payload(),
                &WebhookConfig::default(),
            )
            .await
            .expect_err("private target must be blocked, not attempted");
        assert!(
            err.to_string().contains("allow_private_network"),
            "the policy error (not a transport error) must surface, got: {err}"
        );
    }

    /// The webhook client never follows redirects: a 3xx from an allowed
    /// endpoint surfaces as `HookFailed` instead of bouncing the POST to the
    /// Location target (the classic guard bypass).
    #[tokio::test]
    async fn redirects_are_not_followed() {
        let server = MockServer::start().await;
        let location = format!("{}/private", server.uri());
        Mock::given(method("POST"))
            .and(path("/bounce"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", location.as_str()))
            .mount(&server)
            .await;
        // The bounce target must never be hit (verified by `.expect(0)` on drop).
        Mock::given(method("POST"))
            .and(path("/private"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let err = client
            .post(
                &format!("{}/bounce", server.uri()),
                Duration::from_secs(5),
                &sample_payload(),
                &WebhookConfig::default(),
            )
            .await
            .expect_err("a redirect answer must fail the webhook");
        match err {
            Error::HookFailed { code, .. } => {
                assert_eq!(code, 302, "the 3xx surfaces instead of being followed");
            }
            other => panic!("expected HookFailed(302), got {other:?}"),
        }
    }
}

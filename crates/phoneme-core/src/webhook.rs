//! Outbound webhook delivery, behind an SSRF guard.
//!
//! This module owns [`WebhookClient`], which POSTs a recording's [`HookPayload`]
//! as JSON to `hook.webhook_url`. The daemon's pipeline fires it alongside the
//! local hook ([`crate::hook`]) when one is configured.
//!
//! The load-bearing part is the guard, not the POST. Phoneme is local-first, so
//! the policy is three-tiered (`HostClass`): a webhook into this machine
//! (loopback) is the primary use case and always allowed; the LAN needs an
//! explicit opt-in; the public internet requires TLS. The target is validated
//! before a single byte leaves the machine — that includes resolving a DNS name
//! and classifying every address it yields — and redirects are never followed,
//! so a mistyped or hostile URL can't bounce transcripts at an internal service
//! (S-H1).

use crate::config::WebhookConfig;
use crate::error::{Error, Result};
use crate::types::HookPayload;
use hmac::{Hmac, Mac};
use secrecy::ExposeSecret;
use sha2::Sha256;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// The header carrying the HMAC-SHA256 signature of the request body. The value
/// is `sha256=<lowercase-hex>`, the de-facto standard shape (GitHub, Stripe,
/// etc.), so receivers can verify with off-the-shelf logic.
const SIGNATURE_HEADER: &str = "X-Phoneme-Signature";

/// Headers Phoneme owns and a `custom_headers` entry must never override: the
/// JSON content type (set by the body builder) and the signature header (forged
/// otherwise). Compared case-insensitively. A collision is skipped, Phoneme's
/// value wins, and a warning is logged.
const RESERVED_HEADERS: &[&str] = &["content-type", SIGNATURE_HEADER];

/// Compute the `sha256=<lowercase-hex>` signature value for `body` under
/// `secret` (HMAC-SHA256). The secret is the raw key bytes; an empty secret is
/// the caller's signal that signing is off, so this is only called for a
/// non-empty one.
fn sign_body(secret: &[u8], body: &[u8]) -> String {
    // HMAC accepts a key of any length, so `expect` cannot fire here.
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(7 + digest.len() * 2);
    hex.push_str("sha256=");
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// SSRF classification of a webhook target address (S-H1).
///
/// Phoneme is local-first, so the policy is deliberately three-tiered rather
/// than a blanket private-range block: webhooks into this machine are the
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
    // CGNAT 100.64.0.0/10 (RFC 6598): carrier-grade NAT space that routes inside
    // the operator's network, not the public internet — treat it as private so a
    // webhook can't reach a neighbouring CGNAT host.
    let is_cgnat = ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64;
    if ip.is_loopback() {
        HostClass::Loopback
    } else if ip.is_private() || ip.is_link_local() || ip.is_unspecified() || is_cgnat {
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
            // NAT64 well-known prefix 64:ff9b::/96 (RFC 6052) embeds a v4 address
            // in the low 32 bits; a NAT64 gateway translates it to that v4 host.
            // Classify by the embedded v4 so e.g. 64:ff9b::169.254.169.254 can't
            // smuggle the cloud metadata endpoint past the guard.
            let segs = v6.segments();
            if segs[0] == 0x0064 && segs[1] == 0xff9b && segs[2..6].iter().all(|&s| s == 0) {
                let o = v6.octets();
                return classify_v4(Ipv4Addr::new(o[12], o[13], o[14], o[15]));
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
/// addresses (`Private` > `Public` > `Loopback`). A name that resolves to any
/// private address is treated as private, and a name mixing loopback with
/// public addresses still gets the public-tier HTTPS check, since the
/// connection could land on either.
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

/// Validate a webhook target against the `[webhook]` network policy before any
/// bytes leave the machine. The rules:
///
/// - **Loopback** (127.0.0.0/8, `::1`, the literal `localhost`) — always
///   allowed, any scheme, no knob can break this.
/// - **Private** (see [`HostClass::Private`]) — blocked unless
///   `[webhook] allow_private_network = true`.
/// - **Public** — must be `https` unless `[webhook] allow_http = true`.
///
/// A DNS hostname is resolved here and every address it yields is classified
/// ([`classify_resolved`]), so a name pointing at a private IP is private.
/// `localhost` short-circuits to loopback without touching the resolver, and
/// the `url` parser canonicalizes IPv4 trickery (decimal/octal literals) into
/// dotted-quad form before classification.
///
/// On success returns the connection pin: `Some((host, addrs))` for a resolved
/// DNS name — the exact validated socket addresses the POST must connect to —
/// or `None` when no pin is needed (an IP literal, which reqwest dials directly,
/// or `localhost`, hardcoded to loopback). Pinning closes a TOCTOU window: if
/// the POST re-resolved the hostname, a hostile or rebinding DNS server could
/// answer the guard's lookup with a public IP and the send's lookup with an
/// internal one (S-H1).
async fn check_target(
    url: &str,
    policy: &WebhookConfig,
) -> Result<Option<(String, Vec<SocketAddr>)>> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| Error::InvalidConfig(format!("webhook URL {url:?} is invalid: {e}")))?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed
        .host_str()
        .ok_or_else(|| Error::InvalidConfig(format!("webhook URL {url:?} has no host")))?
        .to_string();

    // `host_str` wraps IPv6 literals in brackets; strip them for parsing.
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let (class, resolved, pin) = if host.eq_ignore_ascii_case("localhost") {
        (HostClass::Loopback, None, None)
    } else if let Ok(ip) = bare.parse::<IpAddr>() {
        (classify_ip(ip), None, None)
    } else {
        // The port only matters to the resolver call's shape, not the verdict.
        let port = parsed.port_or_known_default().unwrap_or(443);
        let socket_addrs: Vec<SocketAddr> = tokio::net::lookup_host((bare, port))
            .await
            .map_err(|e| {
                Error::InvalidConfig(format!("webhook host {host:?} did not resolve: {e}"))
            })?
            .collect();
        if socket_addrs.is_empty() {
            return Err(Error::InvalidConfig(format!(
                "webhook host {host:?} did not resolve to any address"
            )));
        }
        let addrs: Vec<IpAddr> = socket_addrs.iter().map(|sa| sa.ip()).collect();
        // Pin the host name reqwest will look up (the URL host, already
        // lowercased by the `url` parser) to the exact addresses we just
        // classified, so the send can't connect anywhere we didn't validate.
        (
            classify_resolved(&addrs),
            Some(addrs),
            Some((host.clone(), socket_addrs)),
        )
    };

    match class {
        HostClass::Loopback => Ok(pin),
        HostClass::Private => {
            if policy.allow_private_network {
                Ok(pin)
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
                Ok(pin)
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
    ///
    /// When `policy.hmac_secret` is non-empty the request carries an
    /// `X-Phoneme-Signature: sha256=<hex>` header computed over the exact body
    /// bytes (HMAC-SHA256), and every `policy.custom_headers` entry is attached —
    /// except ones colliding with a header Phoneme owns (`Content-Type`, the
    /// signature header), which are skipped so they can't break the content type
    /// or forge the signature.
    ///
    /// Returns [`Error::InvalidConfig`] when the target is disallowed by policy,
    /// [`Error::HookTimeout`] on a slow response, and [`Error::HookFailed`]
    /// (carrying the status and body) on a non-2xx answer, a 3xx included, since
    /// redirects are deliberately not followed.
    ///
    /// **Delivery is at-least-once.** A transient retry (timeout / connection /
    /// 429 / 5xx) re-sends the identical body, so a receiver that committed the
    /// request but whose response was lost (or that 5xx'd after committing) can
    /// see the same event twice. A non-idempotent receiver (say, an "append to
    /// note" webhook) should dedupe on the payload's recording id.
    pub async fn post(
        &self,
        url: &str,
        timeout: Duration,
        payload: &HookPayload,
        policy: &WebhookConfig,
    ) -> Result<()> {
        // The SSRF guard runs first so a blocked target never sees a packet. It
        // also hands back the validated addresses to pin the connection to.
        let pin = check_target(url, policy).await?;

        // Serialize the body ourselves so the signature covers the exact bytes
        // that go on the wire (rather than trusting `.json()` to round-trip
        // identically), and set the JSON content type to match.
        let body = serde_json::to_vec(payload)
            .map_err(|e| Error::Internal(format!("webhook payload serialization failed: {e}")))?;

        // For a resolved DNS name, send through a client that resolves the host
        // only to the addresses the guard already validated, so reqwest's own
        // second resolution at send time can't rebind the name to an internal
        // IP. IP-literal / localhost targets need no pin and reuse the shared
        // client. The pinned client keeps the same no-redirect policy.
        let http = match &pin {
            Some((host, addrs)) => reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .resolve_to_addrs(host, addrs)
                .build()
                .map_err(|e| {
                    Error::Internal(format!("failed to build pinned webhook client: {e}"))
                })?,
            None => self.http.clone(),
        };

        // Pre-validate the custom headers once (so a reserved-header collision is
        // logged once, not per retry — the warning is about the user's config) and
        // precompute the signature header. Phoneme's own headers are set after
        // these on each attempt, so they always win.
        let valid_headers: Vec<(&String, &String)> = policy
            .custom_headers
            .iter()
            .filter(|(name, _)| {
                let collides = RESERVED_HEADERS
                    .iter()
                    .any(|r| r.eq_ignore_ascii_case(name));
                if collides {
                    tracing::warn!(
                        header = %name,
                        "ignoring webhook custom_headers entry: it collides with a header Phoneme controls"
                    );
                }
                !collides
            })
            .collect();
        let secret = policy.hmac_secret.expose_secret();
        let signature = (!secret.is_empty()).then(|| sign_body(secret.as_bytes(), &body));

        // Deliver with bounded exponential backoff. The SSRF guard, body, and pin
        // are settled above; each attempt only rebuilds the request (reqwest's
        // builder is consumed by `send`). Only a transient failure retries — a
        // timeout, a connection error, an HTTP 429, or a 5xx — up to
        // `policy.max_retries` extra tries; a 4xx (the receiver refusing us) and
        // an SSRF block fail immediately.
        let mut attempt = 0u32;
        loop {
            let mut request = http
                .post(url)
                .timeout(timeout)
                .header("content-type", "application/json")
                .body(body.clone());
            for (name, value) in &valid_headers {
                request = request.header(*name, *value);
            }
            // Sign last so the signature header can never be shadowed by a custom one.
            if let Some(sig) = &signature {
                request = request.header(SIGNATURE_HEADER, sig);
            }

            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return Ok(());
                    }
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    if retryable && attempt < policy.max_retries {
                        attempt += 1;
                        tokio::time::sleep(webhook_backoff(attempt)).await;
                        continue;
                    }
                    return Err(Error::HookFailed {
                        code: status.as_u16() as i32,
                        stderr_tail: response.text().await.unwrap_or_default(),
                    });
                }
                Err(e) => {
                    if attempt < policy.max_retries {
                        attempt += 1;
                        tokio::time::sleep(webhook_backoff(attempt)).await;
                        continue;
                    }
                    return Err(if e.is_timeout() {
                        Error::HookTimeout {
                            secs: timeout.as_secs(),
                        }
                    } else {
                        Error::Internal(format!("webhook send failed: {e}"))
                    });
                }
            }
        }
    }
}

/// Exponential backoff before webhook retry attempt `n` (1-based): 250 ms,
/// 500 ms, 1 s, then capped at 2 s.
fn webhook_backoff(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(3);
    Duration::from_millis((250u64 << shift).min(2000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HookMetadata;
    use crate::RecordingId;
    use chrono::Local;
    use secrecy::SecretString;
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

    /// A policy with retries OFF, for tests asserting single-attempt behaviour
    /// (failure mapping, the SSRF guard) without backoff delays.
    fn no_retry() -> WebhookConfig {
        WebhookConfig {
            max_retries: 0,
            ..Default::default()
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
                &no_retry(),
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
                &no_retry(),
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
            max_retries: 0,
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

    // ── Retry / backoff ────────────────────────────────────────────────────

    /// A 4xx is the receiver refusing the request — it is not retried even with
    /// retries enabled. `.expect(1)` proves exactly one POST was sent.
    #[tokio::test]
    async fn post_does_not_retry_4xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .expect(1)
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let err = client
            .post(
                &server.uri(),
                Duration::from_secs(5),
                &sample_payload(),
                &WebhookConfig::default(), // retries enabled — but a 4xx mustn't retry
            )
            .await
            .expect_err("400 must fail");
        assert!(
            matches!(err, Error::HookFailed { code: 400, .. }),
            "expected HookFailed 400, got {err:?}"
        );
    }

    /// A persistent 5xx is retried up to `max_retries` extra times, then fails.
    /// With `max_retries = 2` that's exactly 3 POSTs (verified by `.expect(3)`),
    /// proving transient server faults are retried and the cap is honoured.
    #[tokio::test]
    async fn post_retries_5xx_up_to_max() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3)
            .mount(&server)
            .await;

        let client = WebhookClient::new().unwrap();
        let policy = WebhookConfig {
            max_retries: 2,
            ..Default::default()
        };
        let err = client
            .post(
                &server.uri(),
                Duration::from_secs(5),
                &sample_payload(),
                &policy,
            )
            .await
            .expect_err("a persistent 503 must fail after exhausting retries");
        assert!(
            matches!(err, Error::HookFailed { code: 503, .. }),
            "expected HookFailed 503, got {err:?}"
        );
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
            ("100.64.0.1", Private),      // CGNAT 100.64.0.0/10, low edge
            ("100.127.255.254", Private), // CGNAT, high edge
            ("fc00::1", Private),         // ULA, low half
            ("fd12:3456::1", Private),    // ULA, high half
            ("fe80::1", Private),         // IPv6 link-local
            ("::ffff:192.168.0.1", Private), // v4-mapped must not bypass
            ("64:ff9b::a9fe:a9fe", Private), // NAT64-embedded 169.254.169.254
            ("0.0.0.0", Private),         // unspecified is never a sane target
            ("::", Private),
            ("8.8.8.8", Public),
            ("1.1.1.1", Public),
            ("172.32.0.1", Public),     // one past 172.16/12
            ("192.169.0.1", Public),    // one past 192.168/16
            ("169.255.0.1", Public),    // one past 169.254/16
            ("100.63.255.255", Public), // one below CGNAT 100.64/10
            ("100.128.0.0", Public),    // one past CGNAT 100.64/10
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
    /// round-trip) is always allowed, any scheme, with both knobs off — local
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

    /// `allow_private_network = true` opens private targets, and only private
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

    /// The connection-pin contract: an IP-literal target needs no pin (reqwest
    /// dials the literal directly, no second resolution) and `localhost` is
    /// hardcoded to loopback, so both return `None`. Only a resolved DNS name —
    /// the path exposed to a second resolution at send time — yields a pin, so
    /// `post` can lock the connection to the validated addresses. Every case
    /// here classifies without touching the resolver, so the test is
    /// deterministic.
    #[tokio::test]
    async fn pin_is_none_for_ip_literals_and_localhost() {
        let policy = WebhookConfig {
            allow_http: true,
            allow_private_network: true,
            ..Default::default()
        };
        for url in [
            "http://127.0.0.1:9999/hook",
            "http://[::1]:8123/hook",
            "http://10.0.0.5/hook",
            "https://8.8.8.8/hook",
            "http://localhost:5678/hook",
        ] {
            let pin = check_target(url, &policy)
                .await
                .unwrap_or_else(|e| panic!("{url} must pass the guard: {e}"));
            assert!(pin.is_none(), "{url} must not need a connection pin");
        }
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

    // ── HMAC signing + custom headers ──────────────────────────────────────

    /// Pull the single request a mock server received. Asserts exactly one
    /// request landed, and hands back the full [`wiremock::Request`] so callers
    /// can read its `body` (bytes) and `headers`.
    async fn single_request(server: &MockServer) -> wiremock::Request {
        let mut reqs = server.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1, "expected exactly one delivered request");
        reqs.remove(0)
    }

    /// `sign_body` matches a known (key, body) → HMAC-SHA256 vector. The expected
    /// hex is the canonical RFC-4231-style value for this key+message (verified
    /// against an independent implementation), so a future refactor that changes
    /// the algorithm or hex casing is caught.
    #[test]
    fn sign_body_matches_known_vector() {
        // HMAC-SHA256(key="key", msg="The quick brown fox jumps over the lazy dog")
        let got = sign_body(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            got,
            "sha256=f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    /// With a non-empty `hmac_secret`, the POST carries
    /// `X-Phoneme-Signature: sha256=<hex>` whose value is the HMAC over the exact
    /// body bytes the server received — recomputing it from the body matches.
    #[tokio::test]
    async fn signs_body_when_secret_set() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let policy = WebhookConfig {
            hmac_secret: SecretString::from("topsecret".to_string()),
            ..Default::default()
        };
        WebhookClient::new()
            .unwrap()
            .post(
                &server.uri(),
                Duration::from_secs(5),
                &sample_payload(),
                &policy,
            )
            .await
            .expect("signed POST to a 2xx is Ok");

        let req = single_request(&server).await;
        let sig = req
            .headers
            .get("x-phoneme-signature")
            .expect("signature header present when secret set")
            .to_str()
            .unwrap();
        assert_eq!(
            sig,
            sign_body(b"topsecret", &req.body),
            "header signature must be the HMAC over the exact body bytes"
        );
        assert!(sig.starts_with("sha256="), "de-facto-standard prefix");
    }

    /// With an empty `hmac_secret` (the default), no signature header is attached —
    /// signing is opt-in.
    #[tokio::test]
    async fn no_signature_when_secret_empty() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        WebhookClient::new()
            .unwrap()
            .post(
                &server.uri(),
                Duration::from_secs(5),
                &sample_payload(),
                &WebhookConfig::default(),
            )
            .await
            .expect("unsigned POST to a 2xx is Ok");

        let req = single_request(&server).await;
        assert!(
            req.headers.get("x-phoneme-signature").is_none(),
            "no signature header when the secret is empty"
        );
    }

    /// Every `custom_headers` entry is attached to the outgoing request, and a
    /// custom entry colliding with a reserved header (`Content-Type`) is ignored
    /// so it can't override Phoneme's `application/json`.
    #[tokio::test]
    async fn custom_headers_are_attached_reserved_skipped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut custom_headers = std::collections::BTreeMap::new();
        custom_headers.insert("X-Webhook-Source".to_string(), "phoneme".to_string());
        custom_headers.insert("Authorization".to_string(), "Bearer abc123".to_string());
        // Reserved: must be ignored, Phoneme's application/json must win.
        custom_headers.insert("Content-Type".to_string(), "text/plain".to_string());
        let policy = WebhookConfig {
            custom_headers,
            ..Default::default()
        };

        WebhookClient::new()
            .unwrap()
            .post(
                &server.uri(),
                Duration::from_secs(5),
                &sample_payload(),
                &policy,
            )
            .await
            .expect("POST with custom headers to a 2xx is Ok");

        let req = single_request(&server).await;
        assert_eq!(
            req.headers
                .get("x-webhook-source")
                .unwrap()
                .to_str()
                .unwrap(),
            "phoneme"
        );
        assert_eq!(
            req.headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer abc123"
        );
        assert_eq!(
            req.headers.get("content-type").unwrap().to_str().unwrap(),
            "application/json",
            "a custom Content-Type must not override Phoneme's JSON type"
        );
    }

    /// The `hmac_secret` redacts in `Debug` exactly like an `api_key`: a stray
    /// `{:?}` on the config (or `WebhookConfig`) never prints the plaintext.
    #[test]
    fn hmac_secret_redacted_in_debug() {
        let policy = WebhookConfig {
            hmac_secret: SecretString::from("supersecretsigningkey".to_string()),
            ..Default::default()
        };
        let dump = format!("{policy:?}");
        assert!(
            !dump.contains("supersecretsigningkey"),
            "Debug leaked the plaintext HMAC secret: {dump}"
        );
        assert!(
            dump.contains("<redacted>"),
            "expected the redaction marker, got: {dump}"
        );
    }

    /// The `hmac_secret` round-trips through TOML like an `api_key`: a non-empty
    /// value survives a serialize → deserialize cycle (encrypted at rest, never
    /// stored in plaintext), and reloads equal to the original.
    #[test]
    fn hmac_secret_round_trips_through_toml() {
        let policy = WebhookConfig {
            hmac_secret: SecretString::from("a-signing-secret".to_string()),
            ..Default::default()
        };
        let serialized = toml::to_string(&policy).unwrap();
        assert!(
            !serialized.contains("a-signing-secret"),
            "the secret must not be written in plaintext: {serialized}"
        );
        let parsed: WebhookConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed.hmac_secret.expose_secret(), "a-signing-secret");
        assert_eq!(parsed, policy);
    }
}

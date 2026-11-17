//! `GET /nachrechnen/{token}` — the public Rechnung-nachrechnen page.
//!
//! **portald's one route that serves invoice data without a customer session.**
//! Every other route obtains a [`crate::auth::PortalAuthCtx`] first, so it
//! cannot serve customer data without having asked. This one is the deliberate
//! exception: the verify token printed on the invoice *is* the credential, and
//! all it can reach is billingd's masked projection of that one invoice — no
//! name, no address, the MaLo-ID masked to its ends. An unknown, malformed or
//! revoked token answers the same 404, so the page says nothing about whether
//! a token ever existed, and each client address may ask [`LIMIT_PER_MINUTE`]
//! times a minute ([`GLOBAL_LIMIT_PER_MINUTE`] for all together); behind a
//! reverse proxy the client address is taken from `X-Forwarded-For` only when
//! the peer is one of the configured `trusted_proxies` ([`client_key`]).
//!
//! The page is server-rendered HTML that needs no script, mobile first, light
//! and dark from the reader's system setting, German, and addresses the reader
//! as `[nachrechnen] anrede` says — the same choice outputd's letters make.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    Extension,
    extract::{ConnectInfo, Path},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::{clients::PortalClients, config::PortaldConfig};

/// Requests one client address may make per minute.
pub const LIMIT_PER_MINUTE: u32 = 30;

/// Requests all clients together may make per minute — what bounds a caller
/// rotating addresses.
pub const GLOBAL_LIMIT_PER_MINUTE: u32 = 600;

/// The window key every request also counts against.
const GLOBAL: &str = "*";

/// How the page addresses the reader.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Anrede {
    /// „Sie" — the default, like outputd's letters.
    #[default]
    Sie,
    /// „du".
    Du,
}

/// A fixed-window counter per peer address.
#[derive(Default)]
pub struct Limiter {
    windows: Mutex<HashMap<String, (Instant, u32)>>,
}

impl Limiter {
    /// Count one request from `key` and against the global window; `false`
    /// once either is over its limit.
    pub fn admit(&self, key: &str, now: Instant) -> bool {
        let global = self.count(GLOBAL, now) <= GLOBAL_LIMIT_PER_MINUTE;
        let own = self.count(key, now) <= LIMIT_PER_MINUTE;
        global && own
    }

    fn count(&self, key: &str, now: Instant) -> u32 {
        let mut w = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if w.len() > 10_000 {
            w.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
        }
        let entry = w.entry(key.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(60) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1
    }
}

/// The client address a request is counted under.
///
/// The TCP peer, which the runner hands every handler. Only when that peer is
/// one of the operator's `trusted_proxies` is `X-Forwarded-For` read, and then
/// its **last** entry — the one that proxy appended; every entry before it was
/// written by the client and is the client's choice. Without a peer address,
/// one shared window: too coarse rather than absent.
#[must_use]
pub fn client_key(
    peer: Option<SocketAddr>,
    headers: &axum::http::HeaderMap,
    trusted_proxies: &[std::net::IpAddr],
) -> String {
    let Some(peer) = peer else {
        return "anonymous".to_owned();
    };
    if trusted_proxies.contains(&peer.ip())
        && let Some(forwarded) = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .rfind(|v| !v.is_empty())
            .and_then(|v| v.parse::<std::net::IpAddr>().ok())
    {
        return forwarded.to_string();
    }
    peer.ip().to_string()
}

/// The public page in one of its four states. The status follows from the
/// state, chosen once in [`IntoResponse`], so no call site builds a failure.
enum Seite {
    /// The recomputed invoice.
    Ergebnis(String),
    /// No invoice under this token — the same answer for an unknown, revoked or
    /// malformed one.
    NichtGefunden(Anrede),
    /// The per-client or global limit is spent.
    ZuViele(Anrede),
    /// billingd did not answer.
    NichtErreichbar,
}

impl IntoResponse for Seite {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::Ergebnis(body) => (StatusCode::OK, body),
            Self::NichtGefunden(anrede) => {
                let text = match anrede {
                    Anrede::Sie => {
                        "Zu diesem Link gibt es keine Rechnung. Bitte prüfen Sie den Link auf Ihrer Rechnung."
                    }
                    Anrede::Du => {
                        "Zu diesem Link gibt es keine Rechnung. Bitte prüf den Link auf deiner Rechnung."
                    }
                };
                let status = StatusCode::NOT_FOUND;
                (
                    status,
                    page("Rechnung nicht gefunden", &format!("<p>{text}</p>")),
                )
            }
            Self::ZuViele(anrede) => {
                let text = match anrede {
                    Anrede::Sie => "Bitte versuchen Sie es in einer Minute erneut.",
                    Anrede::Du => "Bitte versuch es in einer Minute erneut.",
                };
                let status = StatusCode::TOO_MANY_REQUESTS;
                (status, page("Zu viele Anfragen", &format!("<p>{text}</p>")))
            }
            Self::NichtErreichbar => {
                let status = StatusCode::SERVICE_UNAVAILABLE;
                (
                    status,
                    page(
                        "Gerade nicht erreichbar",
                        "<p>Die Nachrechnung ist gerade nicht erreichbar. Bitte später erneut versuchen.</p>",
                    ),
                )
            }
        };
        html(status, &body)
    }
}

/// The handler.
pub async fn get_page(
    Extension(cfg): Extension<Arc<PortaldConfig>>,
    Extension(clients): Extension<Arc<PortalClients>>,
    Extension(limiter): Extension<Arc<Limiter>>,
    Path(token): Path<String>,
    req: axum::extract::Request,
) -> Response {
    let anrede = cfg.nachrechnen_anrede;
    let key = client_key(
        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0),
        req.headers(),
        &cfg.trusted_proxies,
    );
    if !limiter.admit(&key, Instant::now()) {
        return Seite::ZuViele(anrede).into_response();
    }
    if token.len() != 22
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Seite::NichtGefunden(anrede).into_response();
    }
    let Some(billingd) = clients.billingd.as_ref() else {
        return Seite::NichtErreichbar.into_response();
    };
    // The token goes to billingd in the body, never in a path a request log or
    // a metrics label could keep.
    let seite = match billingd
        .post_json(
            "/api/v1/billing/nachrechnen",
            &serde_json::json!({ "token": token }),
        )
        .await
    {
        Ok((200, view)) => Seite::Ergebnis(render(&view, anrede)),
        Ok((404, _)) => Seite::NichtGefunden(anrede),
        Ok((status, _)) => {
            tracing::warn!(status, "portald: billingd refused the Nachrechnen page");
            Seite::NichtErreichbar
        }
        Err(e) => {
            tracing::warn!(error = %e, "portald: billingd did not answer the Nachrechnen page");
            Seite::NichtErreichbar
        }
    };
    seite.into_response()
}

fn html(status: StatusCode, body: &str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::HeaderName::from_static("x-robots-tag"), "noindex"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'",
            ),
        ],
        body.to_owned(),
    )
        .into_response()
}

/// Escape text for HTML.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A decimal string as German money: `1234.5` → `1.234,50 €`.
#[must_use]
pub fn euro(v: &str) -> String {
    let (neg, v) = v.strip_prefix('-').map_or((false, v), |r| (true, r));
    let (int, frac) = v.split_once('.').unwrap_or((v, ""));
    let mut cents: String = frac.chars().take(2).collect();
    while cents.len() < 2 {
        cents.push('0');
    }
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(c);
    }
    format!("{}{grouped},{cents} €", if neg { "−" } else { "" })
}

/// A decimal string with a German decimal comma, digits unchanged.
fn zahl(v: &str) -> String {
    v.replace('.', ",")
}

/// `2026-01-31` → `31.01.2026`.
#[must_use]
pub fn datum(iso: &str) -> String {
    let p: Vec<&str> = iso.split('-').collect();
    match p.as_slice() {
        [y, m, d] => format!("{d}.{m}.{y}"),
        _ => iso.to_owned(),
    }
}

fn s<'a>(v: &'a serde_json::Value, k: &str) -> &'a str {
    v.get(k).and_then(serde_json::Value::as_str).unwrap_or("")
}

/// Render billingd's masked view.
#[must_use]
pub fn render(view: &serde_json::Value, anrede: Anrede) -> String {
    let verdict = &view["verdict"];
    let matches = verdict["matches"].as_bool().unwrap_or(false);
    let storniert = verdict["storniert"].as_bool().unwrap_or(false);
    let mut body = String::new();
    let (cls, title, sub) = if matches {
        (
            "ok",
            "Stimmt überein ✓",
            match anrede {
                Anrede::Sie => {
                    "Jede Zeile und jede Summe Ihrer Rechnung ergibt sich auf den Cent aus ihren Angaben."
                }
                Anrede::Du => {
                    "Jede Zeile und jede Summe deiner Rechnung ergibt sich auf den Cent aus ihren Angaben."
                }
            },
        )
    } else {
        (
            "bad",
            "Weicht ab",
            match anrede {
                Anrede::Sie => {
                    "Mindestens eine Zeile oder Summe ergibt sich nicht aus den Angaben. Bitte wenden Sie sich an uns."
                }
                Anrede::Du => {
                    "Mindestens eine Zeile oder Summe ergibt sich nicht aus den Angaben. Bitte wende dich an uns."
                }
            },
        )
    };
    body.push_str(&format!(
        "<section class=\"verdict {cls}\"><h1>{title}</h1><p>{sub}</p>"
    ));
    if storniert {
        body.push_str("<p class=\"note\">Diese Rechnung wurde storniert.</p>");
    }
    if verdict["seal_holds"] == serde_json::Value::Bool(false) {
        body.push_str("<p class=\"note\">Die Rechnung wurde nach ihrer Ausstellung verändert.</p>");
    }
    body.push_str(&format!(
        "<dl><dt>Rechnungsnummer</dt><dd>{}</dd><dt>Zeitraum</dt><dd>{} – {}</dd>\
         <dt>Marktlokation</dt><dd>{}</dd><dt>Gesamtbetrag</dt><dd class=\"num\">{}</dd>",
        esc(s(view, "rechnungsnummer")),
        datum(s(view, "period_from")),
        datum(s(view, "period_to")),
        esc(s(view, "malo_id_masked")),
        euro(s(view, "total_brutto_eur")),
    ));
    if let Some(z) = view["zu_zahlen_eur"].as_str() {
        let label = if z.starts_with('-') {
            "Guthaben"
        } else {
            "Zu zahlen"
        };
        body.push_str(&format!(
            "<dt>{label}</dt><dd class=\"num\">{}</dd>",
            euro(z.trim_start_matches('-'))
        ));
    }
    body.push_str("</dl></section><h2>Die Zeilen</h2><ol class=\"lines\">");
    for l in view["lines"].as_array().into_iter().flatten() {
        let ok = l["matches"].as_bool().unwrap_or(false);
        body.push_str(&format!(
            "<li class=\"{}\"><div class=\"row\"><span>{}</span><span class=\"num\">{}</span></div>",
            if ok { "ok" } else { "bad" },
            esc(s(l, "text")),
            euro(s(l, "stated_eur")),
        ));
        if let (Some(q), Some(p)) = (l["quantity"].as_str(), l["unit_price_eur"].as_str())
            && q != "0"
        {
            body.push_str(&format!(
                "<p class=\"calc\">{} {} × {} € = {}</p>",
                zahl(q),
                esc(s(l, "unit")),
                zahl(p),
                euro(s(l, "recomputed_eur")),
            ));
        }
        if !ok {
            body.push_str(&format!(
                "<p class=\"note\">Nachgerechnet: {} — Abweichung {}</p>",
                euro(s(l, "recomputed_eur")),
                euro(s(l, "difference_eur")),
            ));
        }
        if let Some(src) = l["quantity_source"].as_str() {
            body.push_str(&format!("<p class=\"meta\">Menge: {}</p>", esc(src)));
        }
        if l["estimated"].as_bool().unwrap_or(false) {
            body.push_str(
                "<p class=\"note\">Diese Menge ist geschätzt oder ein Ersatzwert, kein abgelesener Wert (§ 40a EnWG).</p>",
            );
        }
        let rules: Vec<String> = l["rules"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str().map(esc))
            .collect();
        if !rules.is_empty() {
            body.push_str(&format!(
                "<p class=\"meta\">Grundlage: {}</p>",
                rules.join(", ")
            ));
        }
        body.push_str("</li>");
    }
    body.push_str("</ol><h2>Die Summen</h2><table><tbody>");
    for t in view["totals"].as_array().into_iter().flatten() {
        let label = match s(t, "kind") {
            "NETTO" => "Nettobetrag".to_owned(),
            "STEUER" => format!("Umsatzsteuer {} %", zahl(s(t, "rate_percent"))),
            "STEUERBASIS" => "Bemessungsgrundlage".to_owned(),
            "GESAMTSTEUER" => "Umsatzsteuer gesamt".to_owned(),
            "BRUTTO" => "Bruttobetrag".to_owned(),
            "ZU_ZAHLEN" => "Nach Abzug der Abschläge".to_owned(),
            other => esc(other),
        };
        let ok = t["matches"].as_bool().unwrap_or(false);
        body.push_str(&format!(
            "<tr class=\"{}\"><th scope=\"row\">{label}</th><td class=\"num\">{}</td><td aria-label=\"{}\">{}</td></tr>",
            if ok { "ok" } else { "bad" },
            euro(s(t, "recomputed_eur")),
            if ok { "stimmt" } else { "weicht ab" },
            if ok { "✓" } else { "✗" },
        ));
    }
    body.push_str("</tbody></table>");
    body.push_str(match anrede {
        Anrede::Sie => "<p class=\"meta\">Nachgerechnet wird die Rechnung aus ihren eigenen Angaben. Ob Zählerstand und Preis stimmen, können Sie an den Angaben oben selbst prüfen.</p>",
        Anrede::Du => "<p class=\"meta\">Nachgerechnet wird die Rechnung aus ihren eigenen Angaben. Ob Zählerstand und Preis stimmen, kannst du an den Angaben oben selbst prüfen.</p>",
    });
    page("Rechnung nachrechnen", &body)
}

/// The page skeleton: tokens on `:root`, dark mode from the system setting.
fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"robots\" content=\"noindex\"><title>{title}</title><style>\
:root{{--bg:#fff;--fg:#1a1a1a;--muted:#5c5c5c;--line:#e2e2e2;--ok:#1d6b3a;--ok-bg:#e8f4ec;--bad:#9b1c1c;--bad-bg:#fbeaea}}\
@media (prefers-color-scheme:dark){{:root{{--bg:#121212;--fg:#ededed;--muted:#b0b0b0;--line:#333;--ok:#7bd69a;--ok-bg:#16301f;--bad:#ff9b9b;--bad-bg:#3a1616}}}}\
body{{margin:0;background:var(--bg);color:var(--fg);font:16px/1.5 system-ui,sans-serif}}\
main{{max-width:40rem;margin:0 auto;padding:16px}}\
.verdict{{border-radius:12px;padding:16px;margin-bottom:24px}}.verdict.ok{{background:var(--ok-bg)}}.verdict.bad{{background:var(--bad-bg)}}\
.verdict h1{{margin:0 0 8px;font-size:1.5rem}}.ok h1{{color:var(--ok)}}.bad h1{{color:var(--bad)}}\
dl{{display:grid;grid-template-columns:auto 1fr;gap:4px 16px;margin:16px 0 0}}dt{{color:var(--muted)}}dd{{margin:0}}\
.num{{font-variant-numeric:tabular-nums;text-align:right;white-space:nowrap}}\
.lines{{list-style:none;padding:0;margin:0}}.lines li{{border-top:1px solid var(--line);padding:12px 0}}\
.row{{display:flex;justify-content:space-between;gap:16px}}.calc,.meta{{color:var(--muted);margin:4px 0 0;font-size:.9rem}}\
.note{{color:var(--bad);margin:4px 0 0;font-size:.9rem}}table{{width:100%;border-collapse:collapse}}\
th,td{{border-top:1px solid var(--line);padding:8px 0;text-align:left;font-weight:normal}}td:last-child{{width:2rem;text-align:center}}\
</style></head><body><main>{body}</main></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_and_dates_are_german() {
        assert_eq!(euro("132.48"), "132,48 €");
        assert_eq!(euro("1234.5"), "1.234,50 €");
        assert_eq!(euro("-12"), "−12,00 €");
        assert_eq!(datum("2026-12-31"), "31.12.2026");
    }

    #[test]
    fn the_limiter_admits_thirty_a_minute_per_peer() {
        let l = Limiter::default();
        let t = Instant::now();
        assert!((0..LIMIT_PER_MINUTE).all(|_| l.admit("203.0.113.7", t)));
        assert!(!l.admit("203.0.113.7", t), "the 31st is refused");
        assert!(
            l.admit("198.51.100.9", t),
            "another peer has its own window"
        );
        assert!(l.admit("203.0.113.7", t + Duration::from_secs(61)));
    }

    #[test]
    fn the_global_window_bounds_rotating_addresses() {
        let l = Limiter::default();
        let t = Instant::now();
        let admitted = (0..GLOBAL_LIMIT_PER_MINUTE + 10)
            .filter(|i| l.admit(&format!("10.0.{}.{}", i / 250, i % 250), t))
            .count();
        assert_eq!(admitted, GLOBAL_LIMIT_PER_MINUTE as usize);
    }

    #[test]
    fn forwarded_for_is_read_only_behind_a_trusted_proxy() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            "1.2.3.4, 198.51.100.9".parse().expect("header"),
        );
        let proxy: SocketAddr = "10.0.0.2:443".parse().expect("addr");
        let trusted = ["10.0.0.2".parse().expect("ip")];
        assert_eq!(
            client_key(Some(proxy), &h, &trusted),
            "198.51.100.9",
            "the proxy's entry"
        );
        assert_eq!(
            client_key(Some(proxy), &h, &[]),
            "10.0.0.2",
            "untrusted: the peer"
        );
        let direct: SocketAddr = "203.0.113.7:5000".parse().expect("addr");
        assert_eq!(client_key(Some(direct), &h, &trusted), "203.0.113.7");
    }

    #[test]
    fn the_page_addresses_the_reader_as_configured_and_escapes_text() {
        let view = serde_json::json!({
            "rechnungsnummer": "RE-2026-000001",
            "malo_id_masked": "512*****012",
            "period_from": "2026-01-01", "period_to": "2026-01-31",
            "total_brutto_eur": "132.48", "zu_zahlen_eur": "132.48",
            "verdict": { "matches": true, "seal_holds": true, "storniert": false },
            "lines": [{ "text": "<b>Arbeitspreis</b>", "quantity": "250", "unit": "KWH",
                        "unit_price_eur": "0.3187", "stated_eur": "79.68",
                        "recomputed_eur": "79.68", "matches": true, "rules": ["§ 40 EnWG"],
                        "estimated": false }],
            "totals": []
        });
        let sie = render(&view, Anrede::Sie);
        assert!(sie.contains("Stimmt überein"));
        assert!(sie.contains("132,48 €"));
        assert!(sie.contains("Ihrer Rechnung"));
        assert!(
            sie.contains("&lt;b&gt;Arbeitspreis"),
            "line text is escaped"
        );
        assert!(!sie.contains("51238696012"));
        assert!(render(&view, Anrede::Du).contains("deiner Rechnung"));
    }
}

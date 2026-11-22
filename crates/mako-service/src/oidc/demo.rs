//! The demo sign-in: one development key pair, the personas it signs for, and
//! the tokens `just token` prints.
//!
//! **Public on purpose.** The private key below is a development literal that
//! anyone can read, so a token it signs proves nothing about who holds it. It
//! is trusted only by a service whose `[oidc]` says `mode = "demo"` — what
//! `deploy/demo/<service>.toml` sets — and a production verifier refuses a
//! JWKS that carries its public half ([`is_demo_key`]), so an issuer that ever
//! imported it cannot make these tokens valid in production.
//!
//! The tokens are deterministic (RS256 is PKCS#1 v1.5, no `iat`, a fixed
//! `exp`), so the copies in `deploy/demo/` are generated once and held equal to
//! [`mint`] by a test (`MAKO_BLESS_PERSONAS=1` rewrites them).

use std::fmt::Write as _;

use jsonwebtoken::{
    Algorithm, EncodingKey, Header,
    jwk::{Jwk, JwkSet},
};
use serde::Serialize;

/// The `iss` of every demo token.
pub const ISSUER: &str = "urn:mako:demo";
/// The `aud` of every demo token.
pub const AUDIENCE: &str = "mako";
/// The `kid` of the demo key.
pub const KID: &str = "mako-demo-1";
/// The tenant the demo estate serves (`deploy/demo/*.toml`).
pub const TENANT: &str = "stadtwerke-musterstadt";
/// `exp` of every demo token: 2100-01-01T00:00:00Z. Fixed, so a token is a
/// pure function of its persona.
pub const EXP: u64 = 4_102_444_800;

/// The demo key's public modulus (JWK `n`, base64url, no padding).
pub const JWK_N: &str = "4IgPQldCgWop8g4-CZrXCuVC-t3B2X1y0A3gESIlJE5lwtKv6OJ8sOjklCTt4FwaEYHb3sxIwXNrJ035WfyfKrlYqd2yGwIJc9A8KoLXssvzqSEQ5lU_Xpjp1vzowdTe4TftOpvQAa8kpvA3nAf_h9hcMJNrvkZaswnjdJPceO9E4YBfxoXbfXGuSwNC0DqFy09dA8mvi94_sDQLMGL8OeBX-cQ45vffxmHySNbUXWj0oxibEGR5iOhG7JCodvJvXIoc4FYbezsF451FHnAyB85WFzpKpWGmj1IvclitYHDBXVa0ItC11kg5bYz7iiRf6ghFok1KEwyoYWxQ6vASDQ";

/// The demo key's private half — a **public development literal**, see the
/// module documentation. Never configure it anywhere but a demo.
pub const SIGNING_KEY_PEM: &str = concat!(
    "-----BEGIN PRIVATE KEY-----\n",
    "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDgiA9CV0KBainy\n",
    "Dj4JmtcK5UL63cHZfXLQDeARIiUkTmXC0q/o4nyw6OSUJO3gXBoRgdvezEjBc2sn\n",
    "TflZ/J8quVip3bIbAglz0Dwqgteyy/OpIRDmVT9emOnW/OjB1N7hN+06m9ABrySm\n",
    "8DecB/+H2Fwwk2u+RlqzCeN0k9x470ThgF/Ghdt9ca5LA0LQOoXLT10Dya+L3j+w\n",
    "NAswYvw54Ff5xDjm99/GYfJI1tRdaPSjGJsQZHmI6EbskKh28m9cihzgVht7OwXj\n",
    "nUUecDIHzlYXOkqlYaaPUi9yWK1gcMFdVrQi0LXWSDltjPuKJF/qCEWiTUoTDKhh\n",
    "bFDq8BINAgMBAAECggEADpN2niFpDVq1f2PCTkBRjoEEoKGNyI82yV/lOGeFjr5S\n",
    "J9wilFVDr/5mfGGLWcHQ5WsPOX78iCZ0gdgzhE58jLr8kQZ13f/7WhveFAprc01/\n",
    "M28UmEdXdk4p/A1uFMs7btaBHYlo7vsOP5Ifog743TpD/dlMKM2LbM0LCZHhwa3z\n",
    "nTIo7iotFUtk0qm+F5WgajCaIabAJuBLMmfgd5rzDVtm6C/G1jjl7aYht/P1aIpj\n",
    "fqSdd03HBhnm9DptANtjjLg0xuL01lm0M2EG/a6lVeN7/NIs1u3pmK+3U+9cnqyq\n",
    "QKz1N4bh9fl3Mhfzdd8+yhZNtK6ZqBsWJcGMiKwVlwKBgQD5aHTb+ECccYbk3/Fo\n",
    "aIWjScVJNhifWpZU5hShsNe9OilBAm9etleiqEAFtj8GV+Fz2p4PlUzbckQIqLfV\n",
    "8W4E4M2npStm2bdcggyx/cKlrp7HzKdwzTK5E00hjT6vyNJ94ZuiOCPTwdkIqO7l\n",
    "D0RD+/wovSZe0AZ/0D9H72vikwKBgQDmd0iU2EhcO0YHqY3SQyOhcQVwmlehSeT1\n",
    "6MaTjynZUhARTf9gjZtiyEOZHAtXsbj2RCefMSBxaxjK0WhobAqDkYC849BQfKgf\n",
    "X2KFbQ65ajHwp0nLAYGKrUBaPlrKxnHUzqojXgxgTF5FpN5B7zwhvxuemIkCJQtB\n",
    "/8Jsdn/83wKBgFe5GNlB4R/lgmfAKL3rfY2oZd7RSynJh/TGlF0mwUsKSPZXr+pl\n",
    "Qj+w5AveRR6rDF5ed5fNkKHi9/D/vuFeMK9AjrU3kCN10luUVnjIY4049Robbo1r\n",
    "bzrPgiKZ9oGHphmmn8IX8uoJVZgJKzWgAAfVeqStmvBgRNjx20AnAxBHAoGBAJKL\n",
    "3QpQsUp8ol4y8ax25sw8evIMIpa7zyyTNNoVmc3HwdHPIAl8YgchQuK0E35S00M9\n",
    "OVifu3qDWEN3fwWPVewtdIgn8ImNirZQR+yB2XALV/An3+OHdKEC472+kqsIHs/s\n",
    "DgHpRsNhLDzqxi10+26V//EMG2rStWLlR/MCMwZPAoGAf/pu8rGStNpIzAWaby8Z\n",
    "crp6i3nqWxlq1P4U2QBqkrZmOGRGZ/SPSH8Apq4vHRpC/aiWUbFJ98dUAbUYMpob\n",
    "9iG1BQhdjJX4S0cBbAPlp9lAE1jqSLV/ekP4WthFYP1sB5EBAMGHefF+A8mawDYf\n",
    "6JEdjp3ZdBynd8OwS/46K/w=\n",
    "-----END PRIVATE KEY-----\n",
);

/// A named principal of the demo sign-in: who works at the Desk and with which
/// market roles Cedar answers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Persona {
    /// The short id `just token <id>` and the `mako_persona` cookie name.
    pub id: &'static str,
    /// The persona's name as the Desk shows it.
    pub name: &'static str,
    /// The token's `sub` — the Cedar principal.
    pub sub: &'static str,
    /// The token's `mako_roles`.
    pub roles: &'static [&'static str],
}

/// Every persona of the demo sign-in. `admin` is the default.
pub const PERSONAS: &[Persona] = &[
    Persona {
        id: "nb",
        name: "Sachbearbeitung Netz",
        sub: "nb-sachbearbeitung",
        roles: &["NB"],
    },
    Persona {
        id: "lf",
        name: "Sachbearbeitung Vertrieb",
        sub: "lf-sachbearbeitung",
        roles: &["LF"],
    },
    Persona {
        id: "admin",
        name: "Administration",
        sub: "admin",
        roles: &["NB", "LF", "MSB", "ESA", "ADMIN"],
    },
    Persona {
        id: "agent",
        name: "Agent",
        sub: "agent",
        roles: &["NB", "LF", "MSB"],
    },
];

/// The persona a request without a `mako_persona` cookie works as.
pub const DEFAULT_PERSONA: &str = "admin";

/// The persona named `id`.
#[must_use]
pub fn persona(id: &str) -> Option<&'static Persona> {
    PERSONAS.iter().find(|p| p.id == id)
}

/// The JWKS a demo verifier trusts: the demo key alone.
///
/// # Panics
///
/// Never: the JWK is a literal of this module, held parseable by a test.
#[must_use]
pub fn jwks() -> JwkSet {
    serde_json::from_value(serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "kid": KID,
            "use": "sig",
            "alg": "RS256",
            "n": JWK_N,
            "e": "AQAB",
        }]
    }))
    .expect("the demo JWK literal parses")
}

/// Whether `jwk` is the demo key — by `kid` or by its public modulus, so a
/// copy imported under another `kid` is recognised too.
#[must_use]
pub fn is_demo_key(jwk: &Jwk) -> bool {
    if jwk.common.key_id.as_deref() == Some(KID) {
        return true;
    }
    matches!(
        &jwk.algorithm,
        jsonwebtoken::jwk::AlgorithmParameters::RSA(rsa) if rsa.n == JWK_N
    )
}

#[derive(Serialize)]
struct DemoClaims<'a> {
    iss: &'a str,
    aud: &'a str,
    sub: &'a str,
    exp: u64,
    mako_tenant: &'a str,
    mako_roles: &'a [&'a str],
    mako_sparte: [&'a str; 2],
}

/// The demo token for `persona`, signed with the demo key.
///
/// # Panics
///
/// Never: the key is a literal of this module, held loadable by a test.
#[must_use]
pub fn mint(persona: &Persona) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(KID.to_owned());
    let key = EncodingKey::from_rsa_pem(SIGNING_KEY_PEM.as_bytes())
        .expect("the demo signing key literal parses");
    let claims = DemoClaims {
        iss: ISSUER,
        aud: AUDIENCE,
        sub: persona.sub,
        exp: EXP,
        mako_tenant: TENANT,
        mako_roles: persona.roles,
        mako_sparte: ["STROM", "GAS"],
    };
    jsonwebtoken::encode(&header, &claims, &key).expect("RS256 over a literal key signs")
}

/// `deploy/demo/personas.json`: every persona with its token, which `just
/// token` prints.
#[must_use]
pub fn personas_json() -> String {
    let personas: Vec<serde_json::Value> = PERSONAS
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "sub": p.sub,
                "roles": p.roles,
                "token": mint(p),
            })
        })
        .collect();
    let mut s = serde_json::to_string_pretty(&serde_json::json!({
        "issuer": ISSUER,
        "audience": AUDIENCE,
        "tenant": TENANT,
        "personas": personas,
    }))
    .unwrap_or_default();
    s.push('\n');
    s
}

/// `deploy/demo/desk/personas.conf`: the demo edge's map from the Desk's
/// `mako_persona` cookie to the `Authorization` it forwards.
#[must_use]
pub fn nginx_map() -> String {
    let admin = persona(DEFAULT_PERSONA).map(mint).unwrap_or_default();
    let mut s = String::from(
        "# Generated by mako-service (oidc::demo); `MAKO_BLESS_PERSONAS=1 cargo test -p\n\
         # mako-service --all-features demo_files` rewrites it. The demo edge forwards\n\
         # the token of the persona the Desk's `mako_persona` cookie names.\n\
         map $cookie_mako_persona $mako_authorization {\n",
    );
    let _ = writeln!(s, "    default \"Bearer {admin}\";");
    for p in PERSONAS {
        let _ = writeln!(s, "    {} \"Bearer {}\";", p.id, mint(p));
    }
    s.push_str("}\n");
    s
}

/// `deploy/demo/desk/config.json`: the Desk's runtime configuration in the
/// demo — `base` (desk/public/config.json) with the personas it offers.
#[must_use]
pub fn desk_config(base: &serde_json::Value) -> String {
    let mut cfg = base.clone();
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert(
            "demo".to_owned(),
            serde_json::json!({
                "personas": PERSONAS
                    .iter()
                    .map(|p| serde_json::json!({ "id": p.id, "name": p.name, "rollen": p.roles }))
                    .collect::<Vec<_>>(),
                "standard": DEFAULT_PERSONA,
            }),
        );
    }
    let mut s = serde_json::to_string_pretty(&cfg).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oidc::{AuthError, Claims, OidcConfig, OidcError, OidcMode, OidcVerifier};
    use axum::extract::FromRequestParts as _;
    use axum::http::{Request, header};

    fn persona_named(id: &str) -> &'static Persona {
        persona(id).expect("a known persona")
    }

    #[test]
    fn the_key_literals_load_and_agree() {
        // The private PEM signs, and the JWK verifies what it signs.
        let token = mint(persona_named("nb"));
        let claims = OidcVerifier::demo(TENANT).verify(&token).expect("verifies");
        assert_eq!(claims.sub, "nb-sachbearbeitung");
        assert!(jwks().keys.iter().all(is_demo_key));
    }

    #[test]
    fn a_minted_token_carries_its_personas_roles_and_the_demo_tenant() {
        let verifier = OidcVerifier::demo(TENANT);
        for p in PERSONAS {
            let claims = verifier.verify(&mint(p)).expect("a persona token verifies");
            assert_eq!(claims.sub, p.sub);
            assert_eq!(claims.mako_roles, p.roles);
            assert_eq!(claims.tenant(), Some(TENANT));
        }
        assert_eq!(
            mint(persona_named("lf")),
            mint(persona_named("lf")),
            "deterministic"
        );
    }

    /// The demo verifier trusts the demo key and nothing else: a token naming
    /// the demo `kid` but signed by another key is refused.
    #[test]
    fn a_token_signed_by_another_key_is_refused() {
        #[derive(Serialize)]
        struct C<'a> {
            iss: &'a str,
            aud: &'a str,
            sub: &'a str,
            exp: u64,
            mako_tenant: &'a str,
            mako_roles: [&'a str; 1],
        }
        let mut h = Header::new(Algorithm::RS256);
        h.kid = Some(KID.to_owned());
        let forged = jsonwebtoken::encode(
            &h,
            &C {
                iss: ISSUER,
                aud: AUDIENCE,
                sub: "mallory",
                exp: EXP,
                mako_tenant: TENANT,
                mako_roles: ["ADMIN"],
            },
            &EncodingKey::from_rsa_pem(crate::oidc::tests::TEST_RSA_PRIVATE_KEY_PEM.as_bytes())
                .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            OidcVerifier::demo(TENANT).verify(&forged),
            Err(OidcError::TokenInvalid(_))
        ));
    }

    async fn extract(verifier: OidcVerifier, bearer: Option<&str>) -> Result<Claims, AuthError> {
        let mut b = Request::builder().uri("/").extension(verifier);
        if let Some(t) = bearer {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let (mut parts, ()) = b.body(()).unwrap().into_parts();
        Claims::from_request_parts(&mut parts, &()).await
    }

    /// The persona is who Cedar sees; a request without a persona token — the
    /// seeder, a peer service's opaque key — is the dev principal; a token
    /// that does not verify is a 401, never the dev principal.
    #[tokio::test]
    async fn the_extractor_answers_as_the_persona_and_refuses_a_bad_token() {
        let v = OidcVerifier::demo(TENANT);
        let lf = extract(v.clone(), Some(&mint(persona_named("lf"))))
            .await
            .unwrap();
        assert_eq!(lf.sub(), "lf-sachbearbeitung");
        assert!(lf.has_role("LF") && !lf.has_role("ADMIN"));

        assert_eq!(extract(v.clone(), None).await.unwrap().sub(), "dev-admin");
        assert_eq!(
            extract(v.clone(), Some("dev-marktd-key"))
                .await
                .unwrap()
                .sub(),
            "dev-admin"
        );

        let mut tampered = mint(persona_named("lf"));
        tampered.push('x');
        assert!(extract(v, Some(&tampered)).await.is_err());
    }

    /// A configured service key is that service's principal in the demo as in
    /// production, so a policy that admits one service never has to admit the
    /// dev principal; any other opaque key is still the dev principal.
    #[tokio::test]
    async fn a_configured_service_key_is_its_service_in_the_demo() {
        let mut c = cfg(OidcMode::Demo, "");
        c.service_keys = vec![crate::oidc::ServiceKeyConfig {
            name: "portald".to_owned(),
            key: "dev-portald-key".to_owned(),
            roles: Vec::new(),
            sparte: Vec::new(),
        }];
        let v = OidcConfig::build_verifier(
            Some(&c),
            &reqwest::Client::new(),
            TENANT,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("demo mode takes service keys");
        assert_eq!(
            extract(v.clone(), Some("dev-portald-key"))
                .await
                .unwrap()
                .sub(),
            "portald"
        );
        assert_eq!(
            extract(v, Some("dev-marktd-key")).await.unwrap().sub(),
            "dev-admin"
        );
    }

    fn cfg(mode: OidcMode, issuer: &str) -> OidcConfig {
        OidcConfig {
            mode,
            issuer: issuer.to_owned(),
            audience: "mako".to_owned(),
            jwks_refresh_secs: 300,
            service_keys: Vec::new(),
        }
    }

    #[test]
    fn demo_mode_is_reported_and_a_production_block_without_an_issuer_blocks() {
        let demo = cfg(OidcMode::Demo, "").befunde("oidc");
        assert_eq!(demo.len(), 1);
        assert!(!demo[0].ist_blockiert(), "demo mode is a warning");
        assert_eq!(demo[0].pfad, "oidc.mode");

        let leer = cfg(OidcMode::Production, "").befunde("oidc");
        assert!(
            leer.iter()
                .any(|b| b.ist_blockiert() && b.pfad == "oidc.issuer")
        );

        let geliehen = cfg(OidcMode::Production, ISSUER).befunde("oidc");
        assert!(
            geliehen
                .iter()
                .any(|b| b.ist_blockiert() && b.pfad == "oidc.issuer")
        );

        assert!(
            cfg(OidcMode::Production, "https://idp.example.com")
                .befunde("oidc")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn production_with_the_demo_issuer_refuses_to_boot() {
        let err = OidcConfig::build_verifier(
            Some(&cfg(OidcMode::Production, ISSUER)),
            &reqwest::Client::new(),
            TENANT,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .err()
        .expect("the demo issuer is trusted only in demo mode");
        assert!(err.to_string().contains("demo issuer"), "{err:#}");
    }

    /// An issuer on loopback serving discovery and `jwks`.
    async fn issuer_serving(jwks: JwkSet) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let disc = serde_json::json!({ "issuer": base, "jwks_uri": format!("{base}/jwks") });
        let app = axum::Router::new()
            .route(
                "/.well-known/openid-configuration",
                axum::routing::get(move || async move { axum::Json(disc) }),
            )
            .route(
                "/jwks",
                axum::routing::get(move || async move { axum::Json(jwks) }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await });
        base
    }

    async fn boot(issuer: &str) -> anyhow::Result<OidcVerifier> {
        OidcConfig::build_verifier(
            Some(&cfg(OidcMode::Production, issuer)),
            &reqwest::Client::new(),
            TENANT,
            tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    /// The control: the same issuer with an ordinary key boots, so the refusal
    /// below is about the key and nothing else.
    #[tokio::test]
    async fn a_production_issuer_with_its_own_key_boots() {
        let issuer = issuer_serving(crate::oidc::tests::test_jwks()).await;
        let v = boot(&issuer).await.expect("an ordinary issuer boots");
        assert!(!v.is_demo() && !v.is_disabled());
    }

    /// A production deployment whose issuer publishes the demo key — under its
    /// own `kid` or another — refuses to boot: anyone can sign with it.
    #[tokio::test]
    async fn a_production_issuer_publishing_the_demo_key_refuses_to_boot() {
        let issuer = issuer_serving(jwks()).await;
        let err = boot(&issuer)
            .await
            .err()
            .expect("the demo key must not be trusted");
        assert!(format!("{err:#}").contains("demo key"), "{err:#}");

        let mut renamed = jwks();
        renamed.keys[0].common.key_id = Some("rotated-7".to_owned());
        let mut both = crate::oidc::tests::test_jwks();
        both.keys.extend(renamed.keys);
        let issuer = issuer_serving(both).await;
        assert!(
            boot(&issuer).await.is_err(),
            "recognised by its modulus too"
        );
    }

    /// The committed copies in `deploy/demo/` are what [`mint`] produces.
    /// `MAKO_BLESS_PERSONAS=1` rewrites them.
    #[test]
    fn demo_files_match_the_minted_tokens() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let base: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("desk/public/config.json")).unwrap(),
        )
        .unwrap();
        let files = [
            ("deploy/demo/personas.json", personas_json()),
            ("deploy/demo/desk/personas.conf", nginx_map()),
            ("deploy/demo/desk/config.json", desk_config(&base)),
        ];
        let bless = std::env::var_os("MAKO_BLESS_PERSONAS").is_some();
        for (rel, want) in files {
            let path = root.join(rel);
            if bless {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &want).unwrap();
                continue;
            }
            let have = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                have == want,
                "{rel} is not what oidc::demo mints — run \
                 `MAKO_BLESS_PERSONAS=1 cargo test -p mako-service --all-features demo_files`"
            );
        }
    }
}

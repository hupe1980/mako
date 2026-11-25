//! The [`Processd`] daemon: what `mako_service::run` starts.
//!
//! `mako_service::run` owns the lifecycle (tracing, tuned pool with
//! `application_name`, real DB-ping readiness, graceful shutdown, `--check`);
//! this supplies the migrations and the domain router + background workers via
//! [`crate::server::build_router`].

use secrecy::SecretString;
use std::sync::Arc;

use anyhow::Context as _;
use mako_service::{Daemon, ServiceContext};

use crate::config::{self, Config};

/// Allgemeine Festlegungen 6.1d Kap. 2.13 (S. 17), the rule the per-role
/// `[identity]` lists rest on.
const KAP_2_13: &str = "Allgemeine Festlegungen 6.1d Kap. 2.13, S. 17";

/// Every finding about `[identity]`: an empty tenant, no role at all, an entry
/// that is no 13-digit MP-ID or a GS1 GLN where a BDEW- (`99…`) or
/// DVGW-Codenummer (`98…`) identifies a Marktrolle, an MP-ID claimed by two
/// roles, and an affiliated Lieferant that is the deployment's own LF.
fn identitaet(id: &config::IdentityConfig) -> Vec<mako_service::Befund> {
    use mako_service::befund::{Befund, Bereich};
    use std::collections::HashMap;

    let mut out = Vec::new();
    if id.tenant.trim().is_empty() {
        out.push(Befund::blockiert(
            "identity.tenant",
            "the tenant is empty: it is the data-isolation key every row is written \
             under, and no MP-ID stands in for it",
        ));
    }
    if config::EigeneRolle::ALLE
        .iter()
        .all(|r| id.mp_ids(*r).is_empty())
    {
        out.push(
            Befund::blockiert(
                "identity",
                "no role names an MP-ID — set nb_mp_ids, lf_mp_ids or msb_mp_ids for \
                 the Marktrollen this deployment answers as",
            )
            .quelle(KAP_2_13),
        );
    }

    let mut rolle_von: HashMap<&str, config::EigeneRolle> = HashMap::new();
    for rolle in config::EigeneRolle::ALLE {
        let schluessel = rolle.schluessel();
        for mp_id in id.mp_ids(rolle) {
            let pfad = format!("identity.{schluessel}");
            match mp_id.parse::<mako_markt::domain::MarktpartnerId>() {
                Err(e) => out.push(Befund::blockiert(
                    pfad.clone(),
                    format!(
                        "{mp_id:?} is not a 13-digit MarktpartnerId ({e}): no message \
                         addressed to the role would ever match it"
                    ),
                )),
                Ok(mp) if mako_markt::domain::nad_agency_code(&mp) == "9" => out.push(
                    Befund::warnung(
                        pfad.clone(),
                        format!(
                            "{mp_id} is a GS1 GLN, not a BDEW-Codenummer (99…, Strom) or a \
                             DVGW-Codenummer (98…, Gas)"
                        ),
                    )
                    .quelle("BDEW-AWH Identifikatoren V1.2, Kap. 2.2"),
                ),
                Ok(_) => {}
            }
            match rolle_von.get(mp_id.as_str()) {
                Some(andere) if *andere != rolle => out.push(
                    Befund::blockiert(
                        pfad,
                        format!(
                            "{mp_id} is named in both {} and {schluessel}: each \
                             Marktrolle has its own Codenummer",
                            andere.schluessel()
                        ),
                    )
                    .quelle(KAP_2_13),
                ),
                Some(_) => {}
                None => {
                    rolle_von.insert(mp_id.as_str(), rolle);
                }
            }
        }
    }

    out.into_iter()
        .map(|b| b.bereich(Bereich::Identitaeten))
        .collect()
}

/// The `processd` daemon.
pub struct Processd;

impl Daemon for Processd {
    type Config = Config;
    const NAME: &'static str = "processd";

    async fn migrate(_cfg: &Self::Config, pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run processd migrations")?;
        // Transactional outbox for the de.mako.sperr.* notices, and the webhook
        // endpoints and deliveries it fans out to.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure outbox schema")?;
        mako_service::subscriptions::ensure_schema(pool)
            .await
            .context("ensure webhook subscription schema")?;
        Ok(())
    }

    fn validate(cfg: &Self::Config) -> Vec<mako_service::Befund> {
        if let Some(review) = &cfg.shadow_review {
            return review.validate(cfg);
        }
        let mut out = cfg.lf.validate();
        for (configured, path) in [
            (cfg.makod.is_some(), "makod"),
            (cfg.marktd.is_some(), "marktd"),
        ] {
            if !configured {
                out.push(mako_service::Befund::blockiert(
                    path,
                    "Configure the upstream service for market operation.",
                ));
            }
        }
        out.extend(identitaet(&cfg.identity));
        out.extend(cfg.subscriptions.befunde());
        if let Err(e) = cfg.check_auth_posture() {
            // The refusal names every unconfigured door; the path is the first.
            let pfad = if cfg.oidc.is_none() {
                "oidc"
            } else if cfg.webhook.inbound_secret.is_none() {
                "webhook.inbound_secret"
            } else {
                "sperr.erp_hmac_secret"
            };
            out.push(
                mako_service::Befund::aus_fehler(pfad, e)
                    .bereich(mako_service::befund::Bereich::Zugriff),
            );
        }
        out
    }

    fn policy(cfg: &Self::Config) -> mako_service::policy::PolicyReport {
        cfg.policy
            .report()
            .posture_flag("allow_insecure_no_auth", cfg.allow_insecure_no_auth)
    }

    async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<axum::Router> {
        if let Some(review) = &cfg.shadow_review {
            return crate::shadow::install(review, &cfg, ctx).await;
        }
        // Fail closed on both doors before anything else: the REST/MCP surfaces
        // and the inbound webhook each decide what this operator answers a
        // market partner.
        cfg.check_auth_posture()?;

        // ── Resolve env-var references ────────────────────────────────────────
        let makod = cfg.makod.as_ref().context("missing makod config")?;
        let marktd = cfg.marktd.as_ref().context("missing marktd config")?;
        let makod_api_key = config::resolve_env_secret(&makod.api_key).context("makod.api_key")?;
        let marktd_api_key =
            config::resolve_env_secret(&marktd.api_key).context("marktd.api_key")?;
        let inbound_secret = cfg
            .webhook
            .inbound_secret
            .as_deref()
            .map(config::resolve_env_secret)
            .transpose()
            .context("webhook.inbound_secret")?;
        // Resolved, not passed through: the field documents `"env:VAR_NAME"`
        // support, and an unresolved reference would be used as the HMAC key
        // itself — signing every §38 expiry notification with the ASCII bytes
        // `env:VAR_NAME`, which any verifying receiver rejects.
        let eog_notify_webhook_secret = cfg
            .eog
            .notify_webhook_secret
            .as_deref()
            .map(config::resolve_env)
            .transpose()
            .context("eog.notify_webhook_secret")?;

        let obs_outbound_url = cfg
            .obs
            .outbound_url
            .as_deref()
            .map(config::resolve_env)
            .transpose()
            .context("obs.outbound_url")?;
        let obs_outbound_secret = cfg
            .obs
            .outbound_secret
            .as_deref()
            .map(config::resolve_env_secret)
            .transpose()
            .context("obs.outbound_secret")?;
        let sperr_erp_hmac_secret = cfg
            .sperr
            .erp_hmac_secret
            .as_deref()
            .map(config::resolve_env_secret)
            .transpose()
            .context("sperr.erp_hmac_secret")?;

        let tenant = cfg.identity.tenant.clone();

        // ── OIDC ──────────────────────────────────────────────────────────────
        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &tenant,
            ctx.shutdown.clone(),
        )
        .await?;

        // ── Cedar ABAC ────────────────────────────────────────────────────────
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/processd.cedar"
            ))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
        );

        crate::server::build_router(
            crate::server::RunConfig {
                inbound_secret,
                makod_url: makod.url.clone(),
                makod_api_key,
                marktd_url: marktd.url.clone(),
                marktd_api_key,
                identitaet: cfg.identity.identitaet(),
                tenant,
                nb_auto_accept: cfg.nb.auto_accept,
                nb_gas_bearbeitungsfrist_wt: cfg.nb.gas_bearbeitungsfrist_wt,
                nb_einsd_url: cfg.nb.einsd_url.clone(),
                nb_einsd_api_key: cfg.nb.einsd_api_key.clone().map(SecretString::from),
                lf_auto_respond: cfg.lf.auto_respond,
                lf_bilanzkreise: cfg.lf.bilanzkreise.clone(),
                lf_grundversorgungs_netzgebiete: cfg.lf.grundversorgungs_netzgebiete.clone(),
                lf_eog_auto_respond: cfg.lf.eog_auto_respond,
                lf_eog_versorgungsart: cfg.lf.eog_versorgungsart.clone(),
                lf_eog_abrechnungsintervall_monate: cfg.lf.eog_abrechnungsintervall_monate,
                vertragd_url: cfg.vertragd.as_ref().map(|v| v.url.clone()),
                vertragd_api_key: cfg
                    .vertragd
                    .as_ref()
                    .map(|v| SecretString::from(v.api_key.clone())),
                msb_auto_accept: cfg.msb.auto_accept,
                msb_auto_preisanfrage: cfg.msb.auto_preisanfrage,
                esa_auto_accept: cfg.esa.auto_accept,
                esa_auto_reject: cfg.esa.auto_reject,
                esa_accept_after_bindungsfrist: cfg.esa.accept_after_bindungsfrist,
                eog_auto_activate: cfg.eog.auto_activate,
                eog_default_transaktionsgrund: cfg.eog.default_transaktionsgrund.clone(),
                eog_warn_days_before_expiry: cfg.eog.warn_days_before_expiry,
                eog_notify_webhook_url: cfg.eog.notify_webhook_url.clone(),
                eog_notify_webhook_secret,
                verbundene_lieferanten: cfg.identity.verbunden(),
                obs_outbound_url,
                obs_outbound_secret,
                obs_worker: config::ObsConfig {
                    outbound_url: None,
                    outbound_secret: None,
                    ..cfg.obs.clone()
                },
                sperr_erp_webhook_url: cfg.sperr.erp_webhook_url.clone(),
                sperr_erp_hmac_secret,
                sperr_msb_vorlauf_lesart: cfg.policy.msb_vorlauf_lesart,
                oidc,
                cedar,
                mcp: cfg.mcp.clone(),
                subscriptions: cfg.subscriptions.clone(),
            },
            ctx,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::identitaet;
    use crate::config::{Identitaet, IdentityConfig};
    use mako_markt::domain::Sparte;

    fn stadtwerk() -> IdentityConfig {
        IdentityConfig {
            tenant: "stadtwerke-musterstadt".to_owned(),
            nb_mp_ids: vec!["9900357000004".to_owned(), "9800357000004".to_owned()],
            lf_mp_ids: vec!["9900357000028".to_owned(), "9800357000028".to_owned()],
            msb_mp_ids: vec!["9900357000035".to_owned(), "9800357000035".to_owned()],
            verbundene_lieferanten: vec!["9912345000005".to_owned()],
        }
    }

    fn pfade(id: &IdentityConfig) -> Vec<String> {
        identitaet(id).into_iter().map(|b| b.pfad).collect()
    }

    #[test]
    fn one_mp_id_per_role_and_sparte_is_accepted() {
        assert!(identitaet(&stadtwerk()).is_empty());
    }

    #[test]
    fn an_empty_tenant_is_refused_and_never_derived_from_an_mp_id() {
        let mut id = stadtwerk();
        id.tenant = "  ".to_owned();
        let befunde = identitaet(&id);
        assert!(
            befunde
                .iter()
                .any(|b| b.pfad == "identity.tenant" && b.ist_blockiert())
        );
    }

    #[test]
    fn a_deployment_with_no_role_is_refused() {
        let id = IdentityConfig {
            nb_mp_ids: vec![],
            lf_mp_ids: vec![],
            msb_mp_ids: vec![],
            ..stadtwerk()
        };
        assert!(
            identitaet(&id)
                .iter()
                .any(|b| b.pfad == "identity" && b.ist_blockiert())
        );
    }

    /// Allgemeine Festlegungen 6.1d Kap. 2.13: each Marktrolle has its own
    /// Codenummer, so one MP-ID in two role lists is refused.
    #[test]
    fn an_mp_id_in_two_roles_is_refused() {
        let mut id = stadtwerk();
        id.lf_mp_ids.push("9900357000004".to_owned());
        let befunde = identitaet(&id);
        assert!(befunde.iter().any(|b| b.pfad == "identity.lf_mp_ids"
            && b.ist_blockiert()
            && b.text.contains("nb_mp_ids")));
    }

    /// The deployment's own Lieferant belongs to the same undertaking as its
    /// NB, so its Anmeldungen are an affiliate's whether listed or not.
    #[test]
    fn the_own_lieferant_is_always_affiliated() {
        let mut id = stadtwerk();
        assert!(id.verbunden().contains(&"9900357000028".to_owned()));
        id.verbundene_lieferanten.push("9900357000028".to_owned());
        assert!(pfade(&id).is_empty());
        assert_eq!(
            id.verbunden()
                .iter()
                .filter(|m| *m == "9900357000028")
                .count(),
            1
        );
    }

    #[test]
    fn a_malformed_entry_blocks_and_a_gln_warns() {
        let mut id = stadtwerk();
        id.msb_mp_ids.push("99003570".to_owned());
        id.nb_mp_ids.push("4006381333931".to_owned());
        let befunde = identitaet(&id);
        assert!(
            befunde
                .iter()
                .any(|b| b.pfad == "identity.msb_mp_ids" && b.ist_blockiert())
        );
        assert!(
            befunde
                .iter()
                .any(|b| b.pfad == "identity.nb_mp_ids" && !b.ist_blockiert())
        );
    }

    #[test]
    fn a_role_s_mp_id_is_picked_by_sparte_and_never_guessed() {
        let id = stadtwerk().identitaet();
        assert_eq!(id.nb_fuer(Sparte::Strom), Some("9900357000004"));
        assert_eq!(id.nb_fuer(Sparte::Gas), Some("9800357000004"));
        assert_eq!(id.msb_fuer(Sparte::Gas), Some("9800357000035"));
        assert!(id.is_lf("9800357000028") && !id.is_nb("9800357000028"));

        let zwei = Identitaet {
            nb: vec!["9900357000004".to_owned(), "9900357000011".to_owned()],
            ..Default::default()
        };
        assert_eq!(
            zwei.nb_fuer(Sparte::Strom),
            None,
            "two Strom codes: no guess"
        );
        assert_eq!(zwei.nb_fuer(Sparte::Gas), None);
    }

    #[test]
    fn an_unaddressed_message_is_the_nb_s_only_when_it_is_an_nb() {
        let nb = stadtwerk().identitaet();
        assert!(nb.nb_adressiert(""));
        assert!(nb.nb_adressiert("9800357000004"));
        assert!(
            !nb.nb_adressiert("9800357000028"),
            "the LF's Gas code is not the NB's"
        );
        let lf_only = Identitaet {
            lf: vec!["9900357000028".to_owned()],
            ..Default::default()
        };
        assert!(!lf_only.nb_adressiert(""));
    }
}

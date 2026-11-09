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

/// The finding about `[identity] own_mp_id`, if any: a value that is no
/// 13-digit MP-ID, or a GS1 GLN where a BDEW- (`99…`) or DVGW-Codenummer
/// (`98…`) identifies a Marktrolle. Warnings, as the Gleichbehandlung evidence
/// is what reads it.
fn identitaet(own_mp_id: &str) -> Option<mako_service::Befund> {
    use mako_service::befund::{Befund, Bereich};
    let befund = match own_mp_id.parse::<mako_markt::domain::MarktpartnerId>() {
        Err(e) => Befund::warnung(
            "identity.own_mp_id",
            format!(
                "{own_mp_id:?} is not a 13-digit MarktpartnerId ({e}): the \
                 Gleichbehandlungsbericht's comparisons fail silently"
            ),
        ),
        Ok(id) if mako_markt::domain::nad_agency_code(&id) == "9" => Befund::warnung(
            "identity.own_mp_id",
            format!(
                "{own_mp_id} is a GS1 GLN, not a BDEW-Codenummer (99…, Strom) or a \
                 DVGW-Codenummer (98…, Gas)"
            ),
        )
        .quelle("BDEW-AWH Identifikatoren V1.2, Kap. 2.2"),
        Ok(_) => return None,
    };
    Some(befund.bereich(Bereich::Identitaeten))
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
        // Transactional outbox for the de.sperr.* notices.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure outbox schema")?;
        Ok(())
    }

    fn validate(cfg: &Self::Config) -> Vec<mako_service::Befund> {
        let mut out = cfg.lf.validate();
        out.extend(identitaet(&cfg.identity.own_mp_id));
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
        // Fail closed on both doors before anything else: the REST/MCP surfaces
        // and the inbound webhook each decide what this operator answers a
        // market partner.
        cfg.check_auth_posture()?;

        // ── Resolve env-var references ────────────────────────────────────────
        let makod_api_key =
            config::resolve_env_secret(&cfg.makod.api_key).context("makod.api_key")?;
        let marktd_api_key =
            config::resolve_env_secret(&cfg.marktd.api_key).context("marktd.api_key")?;
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

        let tenant = if cfg.identity.tenant.is_empty() {
            cfg.identity.own_mp_id.clone()
        } else {
            cfg.identity.tenant.clone()
        };

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
                makod_url: cfg.makod.url.clone(),
                makod_api_key,
                marktd_url: cfg.marktd.url.clone(),
                marktd_api_key,
                own_mp_id: cfg.identity.own_mp_id.clone(),
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
                self_register_webhook_url: cfg.subscription.webhook_url.clone(),
                subscriber_id: cfg.subscription.subscriber_id.clone(),
                subscriber_event_types: cfg.subscription.event_types.clone(),
                verbundene_lieferanten: cfg.identity.verbundene_lieferanten.clone(),
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
            },
            ctx,
        )
        .await
    }
}

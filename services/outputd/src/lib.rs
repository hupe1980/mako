#![deny(unsafe_code)]
//! `outputd` — Customer-Communications daemon.
//!
//! Port: `:9880`
//!
//! The document half of every service that owes a customer a piece of paper:
//! operator-owned Typst templates, the ZUGFeRD PDF/A-3 carrier, the publish
//! gates and the append-only template store live here. Data stays with the
//! services that compute it — `billingd` sends an invoice view + its CII,
//! `accountingd` will send dunning data — and this daemon turns it into the
//! document a customer receives. See `document` for the layering and
//! `document::mahnung` for why view contracts live with the renderer.
//!
//! | Module | Purpose |
//! |---|---|
//! | `config` | TOML + env configuration |
//! | `document` | Renderer sandbox, ZUGFeRD carrier, publish gates, view contracts |
//! | `handlers` | Axum HTTP handlers: template CRUD + render |
//! | `template_store` | Content-addressed, append-only templates |

pub mod config;
pub mod document;
pub mod handlers;
pub mod template_store;

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
//! | `delivery` | Issued documents, the channels they go out on, and the evidence |
//! | `error` | `OutputError` — the one coded JSON envelope every route answers with |
//! | `handlers` | Axum HTTP handlers: template CRUD, render, issue, delivery |
//! | `template_store` | Content-addressed, append-only templates |
//!
//! # Rendering is not communicating
//!
//! `POST /render/{kind}` produces bytes and forgets them — the right shape for a
//! preview, a re-print, or a caller with its own archive.
//! `POST /documents/{kind}` is the same render **recorded and queued**, which is
//! what makes two regulated questions answerable: reproduce the invoice exactly
//! as issued (§ 14 Abs. 1 UStG, § 147 AO — eight years, unverändert), and show
//! that the § 41f EnWG notice actually reached the customer. See `delivery`.

pub mod config;
pub mod delivery;
pub mod document;
pub mod error;
pub mod handlers;
pub mod template_store;

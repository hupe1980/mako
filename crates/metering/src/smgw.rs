//! BSI TR-03109 Smart Meter Gateway (SMGW) domain model.
//!
//! Models the lifecycle, certificate management, and Consumer Local System (CLS)
//! channel architecture of SMGW installations per BSI TR-03109 and MsbG/MessEG.
//!
//! ## Legal basis
//!
//! - **BSI TR-03109-1**: Smart Meter Gateway — Architecture
//! - **BSI TR-03109-4**: Smart Meter Gateway — Management (SMGW Admin Protocol)
//! - **§21 MsbG**: Anforderungen an Smart-Meter-Gateways
//! - **§22 MsbG**: Zulassung und Einbau
//! - **§29 MsbG**: Remote read-out obligation for iMSys
//! - **§14a EnWG**: Steuerbare Verbrauchseinrichtungen — CLS channel mandatory
//!
//! ## Architecture overview
//!
//! ```text
//! WAN ─────────────────────────────────────────┐
//!  │                                            │
//!  │  AS4 (SMGW Admin)                         │
//!  ▼                                            ▼
//! SMGW                                       MSB Cloud
//!  │
//!  ├── HAN (Home Area Network) ──────────────► Smart Meter
//!  │     DLMS/COSEM, ZigBee, M-Bus            (electricity)
//!  │
//!  ├── HAN ─────────────────────────────────► Sub-meters
//!  │     (gas, heat, water)
//!  │
//!  └── CLS (Consumer Local System) ──────────► §14a devices
//!        (dynamic tariff, load control)
//! ```
//!
//! ## What this module does NOT contain
//!
//! - Actual X.509 certificate parsing (use `x509-cert` or `rcgen` crates)
//! - TLS session management (use `rustls`)
//! - DLMS/COSEM protocol implementation
//! - AS4 transport (see `mako-as4`)

use time::{Date, OffsetDateTime};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// ── GatewayStatus ─────────────────────────────────────────────────────────────

/// Operational status of a Smart Meter Gateway.
///
/// Lifecycle: Provisioned → Commissioned → Operational → (Revoked | Replaced)
///
/// Source: BSI TR-03109-1 §3.2, §29 MsbG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum GatewayStatus {
    /// Factory-fresh, not yet installed at a metering point.
    Provisioned,
    /// Installed and connected to MSB backend — initial key ceremonies done.
    Commissioned,
    /// Fully operational — sending meter data and reachable for remote commands.
    Operational,
    /// Security incident — certificate revoked, pending replacement.
    Revoked,
    /// Replaced by a new gateway — historical record only.
    Replaced,
    /// Temporarily unreachable (communication fault, § 60 Abs. 2 MsbG substitution required).
    CommunicationFault,
}

impl GatewayStatus {
    /// `true` when the gateway can send metered data.
    #[must_use]
    pub fn is_data_delivering(self) -> bool {
        matches!(self, Self::Operational)
    }

    /// `true` when § 60 Abs. 2 MsbG substitute values are required (no data delivery).
    #[must_use]
    pub fn requires_substitute_values(self) -> bool {
        matches!(
            self,
            Self::Revoked | Self::Replaced | Self::CommunicationFault
        )
    }

    /// `true` when a Sonderablesung (emergency read-out) should be triggered.
    #[must_use]
    pub fn triggers_sonderablesung(self) -> bool {
        matches!(self, Self::CommunicationFault | Self::Revoked)
    }
}

// ── GatewayCertificate ────────────────────────────────────────────────────────

/// BSI TR-03109-4 certificate metadata for a Smart Meter Gateway.
///
/// The actual DER/PEM bytes are stored in the MSB PKI system, not here.
/// This struct holds the tracking metadata needed for lifecycle management.
///
/// ## Certificate types (BSI TR-03109-4 §4)
///
/// | Type | Purpose |
/// |---|---|
/// | TLS | WAN communication (SMGW ↔ MSB backend) |
/// | SIG | Data signing (SMGW signs metering data) |
/// | ENC | Data encryption (meter data at rest) |
/// | KEY_AGREEMENT | SMGW Admin Protocol session keys |
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GatewayCertificate {
    /// Certificate serial number (hex string).
    pub serial_number: String,
    /// Certificate type.
    pub cert_type: CertificateType,
    /// Subject CommonName (typically the SMGW device ID).
    pub subject_cn: String,
    /// Issuing CA (BSI-approved Smart Meter CA).
    pub issuer_cn: String,
    /// Certificate validity start.
    pub valid_from: Date,
    /// Certificate validity end.
    pub valid_to: Date,
    /// Whether this certificate has been revoked.
    pub is_revoked: bool,
    /// Revocation date (when `is_revoked = true`).
    pub revoked_at: Option<OffsetDateTime>,
}

impl GatewayCertificate {
    /// `true` when the certificate is currently valid (not expired, not revoked).
    #[must_use]
    pub fn is_valid(&self, today: Date) -> bool {
        !self.is_revoked && today >= self.valid_from && today <= self.valid_to
    }

    /// Number of days until expiry (negative if already expired).
    #[must_use]
    pub fn days_to_expiry(&self, today: Date) -> i32 {
        (self.valid_to - today).whole_days() as i32
    }

    /// `true` when the certificate expires within `warning_days`.
    #[must_use]
    pub fn is_expiring_soon(&self, today: Date, warning_days: i32) -> bool {
        !self.is_revoked && self.days_to_expiry(today) <= warning_days
    }
}

/// BSI TR-03109-4 certificate type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum CertificateType {
    /// WAN TLS certificate — SMGW ↔ MSB communication.
    Tls,
    /// Data signing certificate — SMGW signs metered values.
    Sig,
    /// Data encryption certificate.
    Enc,
    /// Key agreement certificate for SMGW Admin Protocol sessions.
    KeyAgreement,
}

// ── ClsChannel ────────────────────────────────────────────────────────────────

/// A Consumer Local System (CLS) channel on a Smart Meter Gateway.
///
/// CLS channels connect the SMGW to controllable loads per §14a EnWG:
/// heat pumps, EV chargers, air conditioning, controllable PV inverters.
///
/// ## §14a EnWG (Steuerbare Verbrauchseinrichtungen)
///
/// From 2024, MSBs and DSOs can install CLS channels to dimm/control
/// loads ≥ 4.2 kW for grid stability. The SMGW orchestrates the control
/// signals via the CLS interface.
///
/// ## Data flow
///
/// ```text
/// DSO Control Center ─→ market communication (ORDERS 17134/17135) ─→ SMGW Admin
///                                                              │
///                                                              └─→ CLS channel
///                                                                    │
///                                                                    └─→ Device
/// ```
///
/// Source: BK6-24-174 §4 Konfigurationsprozesse; BSI TR-03109-1 §5.3 CLS.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClsChannel {
    /// CLS channel ID (alphanumeric, assigned by MSB).
    pub channel_id: String,
    /// 11-digit MaLo-ID at which this CLS channel is installed.
    pub malo_id: String,
    /// Type of controllable device connected to this channel.
    pub device_type: ClsDeviceType,
    /// Maximum controllable power in kW.
    pub max_power_kw: rust_decimal::Decimal,
    /// Current operational status of this channel.
    pub channel_status: ClsChannelStatus,
    /// §14a Konfigurationsprodukt-Code (BDEW product code for control profile).
    ///
    /// Assigned by the DSO/TSO per BK6-24-174 §4.3. Mandatory for §14a compliance.
    pub produktcode: Option<String>,
    /// Date from which this channel configuration is active.
    pub valid_from: Date,
    /// Date until which this channel configuration is active (None = open).
    pub valid_to: Option<Date>,
}

impl ClsChannel {
    /// `true` when this channel is actively delivering load control.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self.channel_status, ClsChannelStatus::Active)
    }

    /// `true` when §14a Konfigurationsprodukt has been assigned.
    #[must_use]
    pub fn is_section_14a_compliant(&self) -> bool {
        self.produktcode.is_some()
    }
}

/// Type of device connected to a CLS channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ClsDeviceType {
    /// Heat pump (Wärmepumpe) — §14a EnWG Modul 1.
    HeatPump,
    /// Electric vehicle charger (Wallbox) — §14a EnWG Modul 2.
    EvCharger,
    /// Air conditioning (Klimaanlage) — §14a EnWG.
    AirConditioning,
    /// Night storage heating (Nachtspeicherheizung).
    NightStorage,
    /// Controllable PV inverter.
    PvInverter,
    /// Battery storage system.
    BatteryStorage,
    /// Generic controllable load.
    GenericLoad,
}

impl ClsDeviceType {
    /// `true` when this device type triggers §14a regulatory obligations.
    #[must_use]
    pub fn is_section_14a_relevant(self) -> bool {
        matches!(
            self,
            Self::HeatPump | Self::EvCharger | Self::AirConditioning | Self::NightStorage
        )
    }

    /// Human-readable German label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::HeatPump => "Wärmepumpe",
            Self::EvCharger => "Wallbox / E-Mobilität",
            Self::AirConditioning => "Klimaanlage",
            Self::NightStorage => "Nachtspeicherheizung",
            Self::PvInverter => "PV-Wechselrichter",
            Self::BatteryStorage => "Batteriespeicher",
            Self::GenericLoad => "Steuerbare Last",
        }
    }
}

/// Operational status of a CLS channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ClsChannelStatus {
    /// Channel configured and load control active.
    Active,
    /// Channel configured but temporarily suspended (e.g. owner opt-out).
    Suspended,
    /// Channel configured but not yet commissioned.
    Pending,
    /// Channel removed — configuration no longer valid.
    Decommissioned,
    /// Communication fault — control signals not reaching device.
    Fault,
}

// ── SmgwSession ───────────────────────────────────────────────────────────────

/// An SMGW Admin Protocol session — tracks gateway lifecycle and channels.
///
/// One `SmgwSession` corresponds to one physical SMGW device at a metering point.
/// A gateway can have multiple HAN meters and multiple CLS channels.
///
/// ## SMGW Admin Protocol (BSI TR-03109-4)
///
/// The MSB communicates with the SMGW via TLS-secured HTTPS (AS4 wrapper in MaKo
/// context). Commands: `RemoteReadout`, `SetTariff`, `ConfigureCls`, `UpdateFirmware`.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SmgwSession {
    /// SMGW device ID (assigned by manufacturer, used as Zählpunkt-ID component).
    pub device_id: String,
    /// Currently installed firmware version string.
    ///
    /// Only the current version is modeled here; the per-device firmware
    /// *history* (MessEG §22 traceability) is service-layer data — `edmd`
    /// keeps it in the `smgw_sessions` JSONB audit columns.
    pub firmware_version: String,
    /// 13-digit BDEW Codenummer of the responsible MSB.
    pub msb_mp_id: String,
    /// 11-digit MaLo-ID of the primary metering point.
    pub malo_id: String,
    /// Current operational status.
    pub status: GatewayStatus,
    /// All certificates installed on this gateway.
    pub certificates: Vec<GatewayCertificate>,
    /// CLS channels configured on this gateway.
    pub cls_channels: Vec<ClsChannel>,
    /// Last successful data transmission (None = never).
    pub last_contact_at: Option<OffsetDateTime>,
    /// Date of installation.
    pub installed_at: Date,
}

impl SmgwSession {
    /// `true` when the gateway has a valid, non-expiring TLS certificate.
    ///
    /// BSI TR-03109-4 §4: TLS cert renewal must happen ≥ 30 days before expiry.
    #[must_use]
    pub fn has_valid_tls_cert(&self, today: Date) -> bool {
        self.certificates
            .iter()
            .any(|c| matches!(c.cert_type, CertificateType::Tls) && c.is_valid(today))
    }

    /// Find certificates expiring within `warning_days` (any type).
    ///
    /// MSBs must renew certificates before expiry — BSI TR-03109-4 §6.3.
    #[must_use]
    pub fn expiring_certificates(
        &self,
        today: Date,
        warning_days: i32,
    ) -> Vec<&GatewayCertificate> {
        self.certificates
            .iter()
            .filter(|c| !c.is_revoked && c.is_expiring_soon(today, warning_days))
            .collect()
    }

    /// Active CLS channels on this gateway.
    #[must_use]
    pub fn active_cls_channels(&self) -> Vec<&ClsChannel> {
        self.cls_channels
            .iter()
            .filter(|ch| ch.is_active())
            .collect()
    }

    /// `true` when this gateway has any §14a-relevant CLS channels that are active.
    ///
    /// Determines whether DSO load control is possible at this metering point.
    #[must_use]
    pub fn has_section_14a_cls(&self) -> bool {
        self.cls_channels
            .iter()
            .any(|ch| ch.is_active() && ch.device_type.is_section_14a_relevant())
    }

    /// Hours since last contact. `None` if never contacted.
    #[must_use]
    pub fn hours_since_last_contact(&self) -> Option<i64> {
        let last = self.last_contact_at?;
        let now = OffsetDateTime::now_utc();
        Some((now - last).whole_hours())
    }

    /// `true` when the gateway has not been heard from in more than `threshold_hours`.
    ///
    /// Per BSI TR-03109 and § 60 Abs. 2 MsbG: after 2 hours of silence, substitute values
    /// must be generated and a Sonderablesung order should be created.
    #[must_use]
    pub fn is_communication_fault(&self, threshold_hours: i64) -> bool {
        self.hours_since_last_contact()
            .is_none_or(|h| h > threshold_hours)
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    fn valid_cert(cert_type: CertificateType) -> GatewayCertificate {
        GatewayCertificate {
            serial_number: "AA:BB:CC".to_owned(),
            cert_type,
            subject_cn: "SMGW-001".to_owned(),
            issuer_cn: "BSI-Smart-Meter-CA".to_owned(),
            valid_from: date!(2025 - 01 - 01),
            valid_to: date!(2027 - 12 - 31),
            is_revoked: false,
            revoked_at: None,
        }
    }

    fn basic_gateway() -> SmgwSession {
        SmgwSession {
            device_id: "SMGW-2026-001".to_owned(),
            firmware_version: "2.1.4".to_owned(),
            msb_mp_id: "9900357000004".to_owned(),
            malo_id: "51238696780".to_owned(),
            status: GatewayStatus::Operational,
            certificates: vec![valid_cert(CertificateType::Tls)],
            cls_channels: Vec::new(),
            last_contact_at: Some(OffsetDateTime::now_utc()),
            installed_at: date!(2025 - 06 - 01),
        }
    }

    #[test]
    fn operational_gateway_delivers_data() {
        assert!(GatewayStatus::Operational.is_data_delivering());
        assert!(!GatewayStatus::CommunicationFault.is_data_delivering());
    }

    #[test]
    fn revoked_requires_substitute() {
        assert!(GatewayStatus::Revoked.requires_substitute_values());
        assert!(GatewayStatus::CommunicationFault.requires_substitute_values());
        assert!(!GatewayStatus::Operational.requires_substitute_values());
    }

    #[test]
    fn certificate_validity_check() {
        let cert = valid_cert(CertificateType::Tls);
        let today = date!(2026 - 07 - 15);
        assert!(cert.is_valid(today));
        assert!(!cert.is_expiring_soon(today, 30)); // expires 2027-12-31 — far away
    }

    #[test]
    fn certificate_expiry_warning() {
        let mut cert = valid_cert(CertificateType::Tls);
        cert.valid_to = date!(2026 - 07 - 25); // expires in 10 days
        let today = date!(2026 - 07 - 15);
        assert!(cert.is_expiring_soon(today, 30)); // within 30 day warning window
        assert_eq!(cert.days_to_expiry(today), 10);
    }

    #[test]
    fn revoked_certificate_not_valid() {
        let mut cert = valid_cert(CertificateType::Sig);
        cert.is_revoked = true;
        assert!(!cert.is_valid(date!(2026 - 07 - 15)));
        assert!(!cert.is_expiring_soon(date!(2026 - 07 - 15), 30));
    }

    #[test]
    fn gateway_has_valid_tls() {
        let gw = basic_gateway();
        assert!(gw.has_valid_tls_cert(date!(2026 - 07 - 15)));
    }

    #[test]
    fn gateway_no_tls_false() {
        let mut gw = basic_gateway();
        gw.certificates.clear();
        assert!(!gw.has_valid_tls_cert(date!(2026 - 07 - 15)));
    }

    #[test]
    fn cls_channel_section_14a() {
        let channel = ClsChannel {
            channel_id: "CLS-01".to_owned(),
            malo_id: "51238696780".to_owned(),
            device_type: ClsDeviceType::HeatPump,
            max_power_kw: dec!(8.5),
            channel_status: ClsChannelStatus::Active,
            produktcode: Some("BDEW-14A-HP".to_owned()),
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
        };
        assert!(channel.is_active());
        assert!(channel.is_section_14a_compliant());
        assert!(channel.device_type.is_section_14a_relevant());
    }

    #[test]
    fn gateway_with_cls_has_14a() {
        let mut gw = basic_gateway();
        gw.cls_channels.push(ClsChannel {
            channel_id: "CLS-01".to_owned(),
            malo_id: gw.malo_id.clone(),
            device_type: ClsDeviceType::EvCharger,
            max_power_kw: dec!(11.0),
            channel_status: ClsChannelStatus::Active,
            produktcode: Some("BDEW-14A-EV".to_owned()),
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
        });
        assert!(gw.has_section_14a_cls());
    }

    #[test]
    fn expiring_certs_found() {
        let mut gw = basic_gateway();
        let mut expiring = valid_cert(CertificateType::Sig);
        expiring.valid_to = date!(2026 - 07 - 20); // 5 days away
        gw.certificates.push(expiring);

        let today = date!(2026 - 07 - 15);
        let expiring = gw.expiring_certificates(today, 30);
        assert_eq!(expiring.len(), 1);
        assert!(matches!(expiring[0].cert_type, CertificateType::Sig));
    }

    #[test]
    fn device_type_labels_non_empty() {
        for dt in [
            ClsDeviceType::HeatPump,
            ClsDeviceType::EvCharger,
            ClsDeviceType::AirConditioning,
            ClsDeviceType::NightStorage,
            ClsDeviceType::PvInverter,
            ClsDeviceType::BatteryStorage,
            ClsDeviceType::GenericLoad,
        ] {
            assert!(!dt.label().is_empty());
        }
    }
}

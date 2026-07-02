use std::fmt;

/// DVGW EDIFACT message type codes.
///
/// Each variant maps to the exact type-code string that appears in the
/// UNH segment (DE 0065, element 1, component 0).
///
/// All variants are always present regardless of enabled features; feature
/// gates control which concrete message structs and profile data are compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DvgwMessageType {
    /// ALOCAT — Allokationsnachricht (gas quantity allocation).
    ///
    /// Communicates allocated gas quantities per exit zone, entry point, or
    /// measurement point. Used between FNB, VNB, MGV, and BKV.
    Alocat,

    /// NOMINT — Nominierungsintegration (nomination integration).
    ///
    /// Aggregated nomination submitted by a BKV to FNB or MGV.
    Nomint,

    /// NOMRES — Nominierungsantwort (nomination response).
    ///
    /// FNB or MGV response confirming or rejecting a NOMINT nomination.
    Nomres,

    /// SCHEDL — Schedulingnachricht (transport schedule).
    ///
    /// Transport schedule for a gas day (Phase 2).
    Schedl,

    /// IMBNOT — Imbalance notification.
    ///
    /// Intraday balance status communicated by MGV or BKV (Phase 2).
    Imbnot,

    /// TRANOT — Transport notification.
    ///
    /// Transport notification from FNB to BKV (Phase 2).
    Tranot,

    /// DELORD — Delivery order.
    ///
    /// Delivery order from BKV to FNB (Phase 3).
    Delord,

    /// DELRES — Delivery response.
    ///
    /// Delivery response from FNB to BKV (Phase 3).
    Delres,

    /// CHACAP — Capacity change notification.
    ///
    /// Capacity change notification (Phase 3).
    Chacap,
}

impl DvgwMessageType {
    /// Returns the EDIFACT type code as it appears in the UNH segment.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alocat => "ALOCAT",
            Self::Nomint => "NOMINT",
            Self::Nomres => "NOMRES",
            Self::Schedl => "SCHEDL",
            Self::Imbnot => "IMBNOT",
            Self::Tranot => "TRANOT",
            Self::Delord => "DELORD",
            Self::Delres => "DELRES",
            Self::Chacap => "CHACAP",
        }
    }

    /// Parses the type code from a UNH segment string slice.
    ///
    /// Returns `None` for codes that are not recognised DVGW message types.
    /// Note: `CONTRL` and `APERAK` are handled by the `edi-energy` crate.
    #[must_use]
    pub fn from_unh_code(code: &str) -> Option<Self> {
        match code {
            "ALOCAT" => Some(Self::Alocat),
            "NOMINT" => Some(Self::Nomint),
            "NOMRES" => Some(Self::Nomres),
            "SCHEDL" => Some(Self::Schedl),
            "IMBNOT" => Some(Self::Imbnot),
            "TRANOT" => Some(Self::Tranot),
            "DELORD" => Some(Self::Delord),
            "DELRES" => Some(Self::Delres),
            "CHACAP" => Some(Self::Chacap),
            _ => None,
        }
    }

    /// The Cargo feature name that must be enabled to parse this message type.
    #[must_use]
    pub(crate) fn required_feature(self) -> &'static str {
        match self {
            Self::Alocat => "alocat",
            Self::Nomint => "nomint",
            Self::Nomres => "nomres",
            Self::Schedl => "schedl",
            Self::Imbnot => "imbnot",
            Self::Tranot => "tranot",
            Self::Delord => "delord",
            Self::Delres => "delres",
            Self::Chacap => "chacap",
        }
    }

    /// Returns the synthetic Prüfidentifikator (PID) for this message type and
    /// a given direction qualifier, or `None` if no synthetic PID is defined.
    ///
    /// DVGW messages do not carry a BGM DE 1004 Prüfidentifikator for routing.
    /// Instead, the synthetic PID range `90000–90999` encodes
    /// `(message_type, role_qualifier)` for uniform registration in the
    /// `mako-engine` PID router.
    ///
    /// # Synthetic PID table (range `90000–90999`)
    ///
    /// | PID   | Message | Role qualifier | Direction |
    /// |-------|---------|----------------|-----------|
    /// | 90001 | ALOCAT  | Z15 / FNB→BKV  | Daily allocation |
    /// | 90002 | ALOCAT  | Z16 / MGV→BKV  | Monthly allocation |
    /// | 90003 | ALOCAT  | Z17 / VNB→FNB  | Sub-daily allocation |
    /// | 90011 | NOMINT  | Z01 / BKV→FNB  | Nomination |
    /// | 90012 | NOMINT  | Z02 / BKV→MGV  | Nomination |
    /// | 90021 | NOMRES  | Z01 / FNB→BKV  | Nomination response |
    /// | 90022 | NOMRES  | Z02 / MGV→BKV  | Nomination response |
    /// | 90031 | SCHEDL  | —              | Schedule |
    /// | 90041 | IMBNOT  | —              | Imbalance notification |
    /// | 90051 | TRANOT  | —              | Transport notification |
    /// | 90061 | DELORD  | —              | Delivery order |
    /// | 90062 | DELRES  | —              | Delivery response |
    ///
    /// Pass `None` as `role_qualifier` to get the first/primary PID for the type.
    #[must_use]
    pub fn synthetic_pid(self, role_qualifier: Option<&str>) -> Option<u32> {
        match (self, role_qualifier) {
            (Self::Alocat, None | Some("Z15")) => Some(90001),
            (Self::Alocat, Some("Z16")) => Some(90002),
            (Self::Alocat, Some("Z17")) => Some(90003),
            (Self::Nomint, None | Some("Z01")) => Some(90011),
            (Self::Nomint, Some("Z02")) => Some(90012),
            (Self::Nomres, None | Some("Z01")) => Some(90021),
            (Self::Nomres, Some("Z02")) => Some(90022),
            (Self::Schedl, _) => Some(90031),
            (Self::Imbnot, _) => Some(90041),
            (Self::Tranot, _) => Some(90051),
            (Self::Delord, _) => Some(90061),
            (Self::Delres, _) => Some(90062),
            _ => None,
        }
    }
}

impl fmt::Display for DvgwMessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

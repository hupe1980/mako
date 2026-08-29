//! Redispatch 2.0 deadlines, separated by whether a published source still
//! carries them.
//!
//! # Why this module exists
//!
//! Redispatch 2.0 shipped with four deadlines everyone quotes — a 6-hour
//! acknowledgement, a 24-hour Statusanfrage answer, a 5-minute activation
//! response and a `Kostenblatt` due on the 15th. Three of them are no longer
//! published anywhere, and the fourth was never 6 hours.
//!
//! **BK6-23-241 (Beschluss 07.05.2026) repealed the decisions they came from.**
//! Tenorziffer 3 repeals BK6-20-061, Tenorziffer 4 repeals BK6-20-060, and
//! Tenorziffer 1 repeals BK6-20-059 Tenorziffer 1 with the end of 30.06.2026.
//! What replaces them is not a new table of Fristen: Tenorziffer 7 obliges the
//! ÜNB to *develop* bundesweit einheitliche Prozessbeschreibungen with the
//! industry and submit them to the Beschlusskammer, which then publishes them.
//! Until that happens the concrete windows are a matter of the operator's own
//! Prozessbeschreibung, not of a Festlegung.
//!
//! Citing a repealed paragraph for a hard-coded number is worse than having no
//! number: it reads as authority, so nobody re-checks it. This module therefore
//! splits the two cases:
//!
//! - **[Sourced constants](#sourced)** — a published document states the value,
//!   and the doc comment names document, section and wording.
//! - **[`Betreiberfristen`]** — no published source under the current regime.
//!   The value is configuration, the historical BK6-20-05x figure is offered as
//!   a documented default, and the type says so.
//!
//! # The acknowledgement is three minutes, not six hours
//!
//! The one deadline that *is* published sits in the format documentation rather
//! than in a Festlegung, which is presumably how it got missed:
//!
//! > „Der Empfänger der Übertragungsdatei teilt dem Absender **unverzüglich,
//! > jedoch spätestens 3 Minuten** nach Erhalt der Übertragungsdatei das
//! > Ergebnis seiner syntaktischen Prüfung mittels der
//! > `AcknowledgementDocument`-Nachricht mit."
//! >
//! > — EDI@Energy *`AcknowledgementDocument`* Formatbeschreibung 1.0g
//! > (Stand 01.10.2025), Abschnitt „Fristen zur Übermittlung der
//! > `AcknowledgementDocument`-Nachricht"
//!
//! Six hours and three minutes are not the same obligation with a different
//! number. A 6-hour window is something a batch job satisfies; a 3-minute one
//! has to be answered by the receiving process itself. See
//! [`ACK_FRIST`] and the protocol rules in [`ack_regeln`].

use time::{Date, Duration, Month};

// ── Sourced ──────────────────────────────────────────────────────────────────

/// **Sourced.** „unverzüglich, jedoch spätestens 3 Minuten nach Erhalt der
/// Übertragungsdatei" — `AcknowledgementDocument` FB 1.0g, Abschnitt „Fristen zur
/// Übermittlung der `AcknowledgementDocument`-Nachricht".
///
/// The clock runs from receipt of the **Übertragungsdatei**, and exactly one
/// `AcknowledgementDocument` answers each one.
pub const ACK_FRIST: Duration = Duration::minutes(3);

/// **Sourced.** „in der Regel spätestens 30 Minuten vor Beginn der Gültigkeit
/// einer Redispatch-Maßnahme" — `BilAReM` Kap. 6.3.1, for the **Prognosemodell**.
///
/// The Planwertmodell has no fixed figure: its Abrufprozesse „müssen einen
/// Zeitpunkt definieren", which is a
/// [`Betreiberfristen::vorabinformation_planwertmodell`].
///
/// Kap. 6.3.1 also states the rule that makes a late Vorab-Information still
/// mandatory: „Die Information muss auch dann erfolgen, wenn die Frist nicht
/// eingehalten wurde."
pub const VORABINFORMATION_PROGNOSEMODELL: Duration = Duration::minutes(30);

/// **Sourced.** Months after the end of a Redispatch-Maßnahme by which the
/// Ausfallarbeit must be settled or the disagreement formally established —
/// `BilAReM` Kap. 6.4.3.
///
/// > „Die Fristen der Prozesse zur Abstimmung der Ausfallarbeit sind so zu
/// > gestalten, dass spätestens zum Ende des dritten Folgemonats nach Ende der
/// > Redispatch-Maßnahme die Ausfallarbeit feststeht oder aber die Uneinigkeit
/// > über die Höhe der Ausfallarbeit nach dem Clearing festgestellt wird.
/// > **Danach dürfen die Prozesse zur Abstimmung der Ausfallarbeit nicht erneut
/// > gestartet werden.**"
///
/// The second sentence is the load-bearing one: this is a hard stop, not a
/// target. Use [`ausfallarbeit_endet_am`] to get the date.
pub const AUSFALLARBEIT_FOLGEMONATE: u32 = 3;

/// **Sourced.** Werktage of the following month by which the Anlagenbetreiber
/// must supply Wetterdaten or Referenzanlagen-Messdaten for a Spitz- or
/// vereinfachte Spitzabrechnung — `BilAReM` Kap. 3.2.1.
///
/// After that the ANB forms Ersatzwerte itself; the obligation does not lapse,
/// it changes hands.
pub const WETTERDATEN_WERKTAGE: u32 = 4;

/// **Sourced.** A Stammdatum's `gueltig_ab` must lie at least this many
/// Werktage in the future, counted from receipt — `Stammdaten` AWT 1.4b Fußnote 27.
pub const STAMMDATEN_GUELTIG_AB_MIN_WERKTAGE: u32 = 5;

/// **Sourced.** The longer variant of the same rule, for the `Stammdaten` the AWT
/// marks with Fußnote 33.
pub const STAMMDATEN_GUELTIG_AB_MIN_WERKTAGE_LANG: u32 = 10;

/// **Sourced.** „Das `Gueltig_ab` darf maximal zwei Jahre nach dem Wert aus
/// Erstellungszeitpunkt liegen" — `Stammdaten` AWT 1.4b Fußnoten 31 und 32.
pub const STAMMDATEN_GUELTIG_AB_MAX_JAHRE: i32 = 2;

/// **Sourced.** Notice the ANB must give before moving an existing SR from the
/// Prognose- into the Planwertmodell — `BilAReM` Kap. 2.3.2, „spätestens sechs
/// Monate vor der Wirksamkeit der Zuordnung".
pub const PLANWERT_UEBERFUEHRUNG_VORLAUF_MONATE: u32 = 6;

/// **Sourced.** Werktage before the planned Inbetriebnahme by which the ANB must
/// notify the Bilanzierungsmodell of a **newly created** SR — `BilAReM` Kap. 2.3.2.
///
/// Conditional: only if the BTR or EIV gave the ANB everything it needed at
/// least [`PLANWERT_NEUE_SR_INFORMATION_WERKTAGE`] Werktage before that date.
/// Otherwise the ANB has five Werktage from the day the information was complete.
pub const PLANWERT_NEUE_SR_MITTEILUNG_WERKTAGE: u32 = 5;

/// **Sourced.** Werktage before the planned Inbetriebnahme by which the BTR or
/// EIV must have given the ANB the information — `BilAReM` Kap. 2.3.2.
pub const PLANWERT_NEUE_SR_INFORMATION_WERKTAGE: u32 = 10;

/// **Sourced.** The only days a Planwertmodell-Zuordnung may take effect —
/// `BilAReM` Kap. 2.3.2, „nur zum 01.01., 01.04., 01.07. oder 01.10. eines
/// Jahres".
pub const PLANWERT_WIRKSAMKEITSTERMINE: [(Month, u8); 4] = [
    (Month::January, 1),
    (Month::April, 1),
    (Month::July, 1),
    (Month::October, 1),
];

/// Whether `date` is one of the four days a Planwertmodell-Zuordnung may take
/// effect (`BilAReM` Kap. 2.3.2).
#[must_use]
pub fn ist_wirksamkeitstermin(date: Date) -> bool {
    PLANWERT_WIRKSAMKEITSTERMINE
        .iter()
        .any(|&(m, d)| date.month() == m && date.day() == d)
}

/// Last day the Ausfallarbeit of a Maßnahme ending on `massnahme_ende` may be
/// settled — the end of the third following month (`BilAReM` Kap. 6.4.3).
///
/// # Panics
///
/// Never for a date in the representable calendar range.
#[must_use]
pub fn ausfallarbeit_endet_am(massnahme_ende: Date) -> Date {
    let mut d = massnahme_ende;
    for _ in 0..AUSFALLARBEIT_FOLGEMONATE {
        d = d
            .replace_day(1)
            .expect("day 1 exists in every month")
            .checked_add(Duration::days(32))
            .expect("date overflow")
            .replace_day(1)
            .expect("day 1 exists in every month");
    }
    let letzter = time::util::days_in_month(d.month(), d.year());
    d.replace_day(letzter).expect("last day of a real month")
}

// ── Acknowledgement protocol ────────────────────────────────────────────────

/// The four protocol rules that come with the `AcknowledgementDocument`, all from
/// the FB 1.0g section „Grundsätze bei der Syntaxprüfung und der Rückmeldung des
/// Ergebnisses über das `AcknowledgementDocument`".
///
/// They are stated as constants rather than prose because each one is a branch
/// an implementation either has or silently does not:
///
/// 1. **Exactly one ACK per Übertragungsdatei** — „Auf jede eingegangene
///    Übertragungsdatei ist immer genau eine `AcknowledgementDocument`-Nachricht
///    … zu senden", and it either confirms or rejects the file **as a whole**.
/// 2. **No ACK means not processed** — „Eine nicht empfangene
///    `AcknowledgementDocument`-Nachricht bedeutet, dass die Ursprungsnachricht
///    beim Empfänger nicht bearbeitet wird." Silence is a negative answer, so a
///    sender that treats a missing ACK as success loses the message.
/// 3. **Never acknowledge an acknowledgement** — „Auf eine erhaltene
///    `AcknowledgementDocument`-Nachricht ist keine
///    `AcknowledgementDocument`-Nachricht zu senden."
/// 4. **A late ACK does not breach the business Frist** — „Syntaxfehler-
///    meldungen, welche außerhalb der Frist beim Absender der Übertragungsdatei
///    eingehen, dürfen nicht zu einer Fristverletzung des eigentlichen
///    Geschäftsvorfalles führen." The transport clock and the process clock are
///    separate; a slow ACK must not fail the Geschäftsvorfall it carried.
pub mod ack_regeln {
    /// Exactly one `AcknowledgementDocument` answers each Übertragungsdatei.
    pub const EINE_ACK_JE_UEBERTRAGUNGSDATEI: bool = true;

    /// A missing `AcknowledgementDocument` means the message was **not**
    /// processed by the receiver.
    pub const FEHLENDE_ACK_BEDEUTET_NICHT_BEARBEITET: bool = true;

    /// An `AcknowledgementDocument` is never itself acknowledged.
    pub const ACK_WIRD_NICHT_QUITTIERT: bool = true;

    /// A late `AcknowledgementDocument` must not cause a Fristverletzung of the
    /// Geschäftsvorfall it carried.
    pub const VERSPAETETE_ACK_BRICHT_KEINE_GESCHAEFTSFRIST: bool = true;
}

/// Outcome the `AcknowledgementDocument` reports (FB 1.0g, „ACK / Aussage /
/// Bedeutung").
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckErgebnis {
    /// `A01` — Anerkennungsmeldung; the receiver will process the message.
    Positiv,
    /// `A02` + `Z12` — the XSD did not validate: a syntax error. Not processed.
    SyntaxfehlerXsd,
    /// `A02` + one of `Z13`–`Z18` — syntactically valid but not processable
    /// (wrong format version, Meldezeitraum out of range, missing element per a
    /// dependency, …). Not processed.
    NichtVerarbeitbar,
}

impl AckErgebnis {
    /// Whether the receiver will process the message.
    #[must_use]
    pub fn wird_verarbeitet(self) -> bool {
        self == Self::Positiv
    }

    /// The `ReasonCode` the `AcknowledgementDocument` carries.
    ///
    /// Both negative outcomes ride `A02 Message fully rejected`; they differ in
    /// the accompanying code, which the caller supplies from the AWT.
    #[must_use]
    pub fn reason_code(self) -> &'static str {
        match self {
            Self::Positiv => "A01",
            Self::SyntaxfehlerXsd | Self::NichtVerarbeitbar => "A02",
        }
    }
}

// ── Betreiberfristen ────────────────────────────────────────────────────────

/// Deadlines that have **no published source** under the current regime.
///
/// BK6-20-059 Tenorziffer 1, BK6-20-060 and BK6-20-061 are repealed
/// (BK6-23-241 Tenorziffern 1, 3, 4), and their replacements — the bundesweit
/// einheitliche Prozessbeschreibungen of Tenorziffer 7 — are not published yet.
/// These windows are therefore the operator's own, and a deployment must be
/// able to state them rather than inherit a number from a decision that no
/// longer exists.
///
/// [`Betreiberfristen::historisch`] returns the BK6-20-05x figures as a
/// starting point, clearly labelled as historical rather than binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Betreiberfristen {
    /// Window in which the anweisende Netzbetreiber expects an ACR/AAR to an
    /// `ActivationDocument` (ACO).
    ///
    /// Historically five minutes (BK6-20-060, repealed). Whatever the value, it
    /// is a real-time constraint: the scheduler that watches it must tick well
    /// inside the window.
    pub aktivierung_antwort: Duration,

    /// The point before a Maßnahme's validity at which the Planwertmodell
    /// Vorab-Information goes out.
    ///
    /// `BilAReM` Kap. 6.3.1 requires the Abrufprozesse to *define* one but names
    /// no figure — unlike the Prognosemodell, where
    /// [`VORABINFORMATION_PROGNOSEMODELL`] is stated.
    pub vorabinformation_planwertmodell: Duration,

    /// Day of the following month by which the `Kostenblatt` is submitted.
    ///
    /// Historically the 15th (BK6-20-061, repealed).
    pub kostenblatt_stichtag: u8,

    /// Werktage in which a VNB forwards received `Stammdaten` to the upstream ÜNB.
    ///
    /// Historically one Werktag (BK6-20-060, repealed). `BilAReM` Kap. 6.2.1.1
    /// keeps the *obligation* — the responsible party sends a changed value
    /// „unverzüglich nach Bekanntwerden" — but attaches no countable window.
    pub stammdaten_weiterleitung_werktage: u32,
}

impl Betreiberfristen {
    /// The BK6-20-05x figures, as a documented starting point.
    ///
    /// These are **not** binding: the decisions that set them are repealed. They
    /// are what the market ran on until 30.06.2026 and are therefore the least
    /// surprising default, but a deployment should replace them with the values
    /// from its own Prozessbeschreibung.
    #[must_use]
    pub const fn historisch() -> Self {
        Self {
            aktivierung_antwort: Duration::minutes(5),
            vorabinformation_planwertmodell: Duration::minutes(30),
            kostenblatt_stichtag: 15,
            stammdaten_weiterleitung_werktage: 1,
        }
    }

    /// Whether every window is positive and the `Kostenblatt` day is a real day
    /// of the month.
    ///
    /// # Errors
    ///
    /// A message naming the field that is out of range.
    pub fn validate(self) -> Result<(), String> {
        if !self.aktivierung_antwort.is_positive() {
            return Err("aktivierung_antwort muss positiv sein".to_owned());
        }
        if !self.vorabinformation_planwertmodell.is_positive() {
            return Err("vorabinformation_planwertmodell muss positiv sein".to_owned());
        }
        if !(1..=28).contains(&self.kostenblatt_stichtag) {
            return Err(format!(
                "kostenblatt_stichtag {} liegt außerhalb von 1..=28 — ein späterer Tag \
                 existiert im Februar nicht",
                self.kostenblatt_stichtag
            ));
        }
        if self.stammdaten_weiterleitung_werktage == 0 {
            return Err("stammdaten_weiterleitung_werktage muss mindestens 1 sein".to_owned());
        }
        Ok(())
    }
}

impl Default for Betreiberfristen {
    fn default() -> Self {
        Self::historisch()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    #[test]
    fn the_ack_frist_is_three_minutes() {
        // AcknowledgementDocument FB 1.0g. Six hours was never published; it
        // came from a section of BK6-20-059 that no longer applies.
        assert_eq!(ACK_FRIST, Duration::minutes(3));
        assert!(ACK_FRIST < Duration::hours(6));
    }

    #[test]
    fn ausfallarbeit_closes_at_the_end_of_the_third_following_month() {
        // A Maßnahme ending mid-January closes at the end of April.
        assert_eq!(
            ausfallarbeit_endet_am(d(2026, Month::January, 14)),
            d(2026, Month::April, 30)
        );
        // The last day of a month must not spill into the month after next.
        assert_eq!(
            ausfallarbeit_endet_am(d(2026, Month::January, 31)),
            d(2026, Month::April, 30)
        );
        // Across a year boundary, into a leap February.
        assert_eq!(
            ausfallarbeit_endet_am(d(2027, Month::November, 30)),
            d(2028, Month::February, 29)
        );
    }

    #[test]
    fn only_the_four_quarter_days_are_wirksamkeitstermine() {
        for (m, day) in PLANWERT_WIRKSAMKEITSTERMINE {
            assert!(ist_wirksamkeitstermin(d(2027, m, day)));
        }
        for date in [
            d(2027, Month::January, 2),
            d(2027, Month::February, 1),
            d(2027, Month::June, 1),
            d(2027, Month::December, 31),
        ] {
            assert!(!ist_wirksamkeitstermin(date), "{date}");
        }
    }

    #[test]
    fn a_positive_ack_is_the_only_one_that_processes() {
        assert!(AckErgebnis::Positiv.wird_verarbeitet());
        assert_eq!(AckErgebnis::Positiv.reason_code(), "A01");
        for e in [AckErgebnis::SyntaxfehlerXsd, AckErgebnis::NichtVerarbeitbar] {
            assert!(!e.wird_verarbeitet());
            assert_eq!(e.reason_code(), "A02", "both negatives ride A02");
        }
    }

    #[test]
    fn the_historical_defaults_are_the_bk6_20_05x_figures() {
        let f = Betreiberfristen::historisch();
        assert_eq!(f.aktivierung_antwort, Duration::minutes(5));
        assert_eq!(f.kostenblatt_stichtag, 15);
        assert_eq!(f.stammdaten_weiterleitung_werktage, 1);
        f.validate().expect("the historical figures are in range");
    }

    #[test]
    fn a_kostenblatt_stichtag_that_february_does_not_have_is_refused() {
        let mut f = Betreiberfristen::historisch();
        f.kostenblatt_stichtag = 30;
        assert!(f.validate().is_err());
        f.kostenblatt_stichtag = 0;
        assert!(f.validate().is_err());
        f.kostenblatt_stichtag = 28;
        assert!(f.validate().is_ok());
    }

    #[test]
    fn a_non_positive_window_is_refused() {
        let mut f = Betreiberfristen::historisch();
        f.aktivierung_antwort = Duration::ZERO;
        assert!(f.validate().is_err());
        let mut f = Betreiberfristen::historisch();
        f.stammdaten_weiterleitung_werktage = 0;
        assert!(f.validate().is_err());
    }

    #[test]
    fn the_prognosemodell_vorabinformation_is_the_only_sourced_abruf_window() {
        // Kap. 6.3.1 states 30 minutes for the Prognosemodell and requires the
        // Planwertmodell processes to define one without naming a figure.
        assert_eq!(VORABINFORMATION_PROGNOSEMODELL, Duration::minutes(30));
    }
}

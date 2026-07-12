//! SEPA pain.008.003.02 XML builder.
//!
//! Generates a minimal but standards-compliant ISO 20022 pain.008 direct-debit
//! initiation message for the active SEPA mandates with outstanding Abschlag.
//!
//! This is a simplified implementation covering the mandatory fields.
//! Production deployments should use a validated ISO 20022 library.

use crate::pg::SepaMandateRow;
use time::OffsetDateTime;

pub fn build_pain_008(creditor_iban_or_name: &str, entries: &[(&SepaMandateRow, i64)]) -> String {
    let now = OffsetDateTime::now_utc();
    let msg_id = format!("ACCT-{}", now.unix_timestamp());
    let creation = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let total_ct: i64 = entries.iter().map(|(_, ct)| ct).sum();
    let total_eur = format!("{:.2}", total_ct as f64 / 100.0);

    let transactions: String = entries
        .iter()
        .map(|(mandate, amount_ct)| {
            let amount_eur = format!("{:.2}", *amount_ct as f64 / 100.0);
            format!(
                r#"    <DrctDbtTxInf>
      <PmtId><InstrId>{}</InstrId><EndToEndId>{}</EndToEndId></PmtId>
      <InstdAmt Ccy="EUR">{}</InstdAmt>
      <DrctDbtTx>
        <MndtRltdInf>
          <MndtId>{}</MndtId>
          <DtOfSgntr>{}</DtOfSgntr>
        </MndtRltdInf>
      </DrctDbtTx>
      <DbtrAgt><FinInstnId><BIC>{}</BIC></FinInstnId></DbtrAgt>
      <Dbtr><Nm>{}</Nm></Dbtr>
      <DbtrAcct><Id><IBAN>{}</IBAN></Id></DbtrAcct>
    </DrctDbtTxInf>"#,
                mandate.mandate_id,
                mandate.mandatsref,
                amount_eur,
                mandate.mandatsref,
                mandate.signed_at,
                mandate.bic.as_deref().unwrap_or("NOTPROVIDED"),
                mandate.kontoinhaber.as_deref().unwrap_or("Kunde"),
                mandate.iban,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:pain.008.003.02">
  <CstmrDrctDbtInitn>
    <GrpHdr>
      <MsgId>{msg_id}</MsgId>
      <CreDtTm>{creation}</CreDtTm>
      <NbOfTxs>{nb}</NbOfTxs>
      <CtrlSum>{total_eur}</CtrlSum>
      <InitgPty><Nm>{creditor}</Nm></InitgPty>
    </GrpHdr>
    <PmtInf>
      <PmtInfId>{msg_id}-1</PmtInfId>
      <PmtMtd>DD</PmtMtd>
      <NbOfTxs>{nb}</NbOfTxs>
      <CtrlSum>{total_eur}</CtrlSum>
      <PmtTpInf>
        <SvcLvl><Cd>SEPA</Cd></SvcLvl>
        <LclInstrm><Cd>CORE</Cd></LclInstrm>
        <SeqTp>RCUR</SeqTp>
      </PmtTpInf>
      <ReqdColltnDt>{collection_date}</ReqdColltnDt>
      <Cdtr><Nm>{creditor}</Nm></Cdtr>
      <CdtrAcct><Id><Othr><Id>{creditor}</Id></Othr></Id></CdtrAcct>
      <CdtrAgt><FinInstnId><BIC>NOTPROVIDED</BIC></FinInstnId></CdtrAgt>
{transactions}
    </PmtInf>
  </CstmrDrctDbtInitn>
</Document>"#,
        msg_id = msg_id,
        creation = creation,
        nb = entries.len(),
        total_eur = total_eur,
        creditor = creditor_iban_or_name,
        collection_date = {
            let d = now + time::Duration::days(5); // T+5 business days
            format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day())
        },
        transactions = transactions,
    )
}

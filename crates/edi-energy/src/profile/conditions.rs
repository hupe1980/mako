//! AHB statuses and the Bedingungen they cite.
//!
//! A status is `Muss`, `Soll` or `Kann` (on a segment or group) or the operand
//! `X`, `M`, `S` or `K` (on a data element or code), optionally followed by a
//! Bedingung expression such as `[10]`, `[7] ∧ ([577] ⊻ [UB1])`. The
//! expression's operands are Bedingungen numbers whose meaning Allgemeine
//! Festlegungen 6.1d Kap. 6.4 fixes by range:
//!
//! | range | kind | receiver-checkable |
//! |---|---|---|
//! | `[1]`–`[499]` | Voraussetzung | yes — from the message content |
//! | `[500]`–`[899]` | Hinweis | never binds |
//! | `[901]`–`[999]` | Formatbedingung | does not gate presence |
//! | `[2000]`–`[2499]` | Wiederholbarkeit | does not gate presence |
//! | `[UB1]`–`[UB3]` | Zeitpunktangabe | does not gate presence |
//! | `[nPa..b]` | Paket | yes — through its Paketvoraussetzung |
//!
//! A Paket citation is a macro: `[2P0..1]` stands for the Paketvoraussetzung
//! the AHB's Paketübersicht prints for `2P`, which is an expression of the
//! same shape (Kap. 6.9.1). The `a..b` suffix is the Paketmerkmal, the
//! minimal and maximal repetition of the marked Qualifier/Code within the
//! Paket (Kap. 6.9.2); [`Paket`] reads it.
//!
//! A Voraussetzung is checkable by definition ("werden nur Informationen
//! verwendet, die an anderer Stelle im Anwendungsfall vorhanden sind", Kap.
//! 6.5); [`Voraussetzung::parse`] reads the shapes the AHBs use — the
//! presence or absence of a segment with given qualifiers, a data element with
//! a given value, a repetition count — and anything it cannot read evaluates
//! to [`Truth::Unknown`], which never causes a rejection.

use std::fmt;

/// The AHB status word or operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// The information is always to be given.
    Muss,
    /// Needed for business reasons; the sender decides.
    Soll,
    /// At the sender's discretion.
    Kann,
    /// Operand `X` — used as the segment's status says.
    X,
    /// Operand `M` — Muss, with a condition.
    M,
    /// Operand `S` — Soll.
    S,
    /// Operand `K` — Kann.
    K,
}

impl StatusKind {
    /// Whether the receiver may reject on this status's condition.
    ///
    /// Only `Muss`/`M`/`X` conditions are checkable from the message; a
    /// Soll- or Kann-Bedingung depends on what the sender knows (Kap. 6.5).
    #[must_use]
    pub fn is_receiver_checkable(self) -> bool {
        matches!(self, Self::Muss | Self::M | Self::X)
    }

    /// The word as printed.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Muss => "Muss",
            Self::Soll => "Soll",
            Self::Kann => "Kann",
            Self::X => "X",
            Self::M => "M",
            Self::S => "S",
            Self::K => "K",
        }
    }
}

/// A parsed status: the word and its expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The status word or operand.
    pub kind: StatusKind,
    /// The Bedingung expression, if any.
    pub expr: Option<Expr>,
}

impl Status {
    /// Parse `Muss [10] ∧ [11]`, `X`, `M [7]` …
    ///
    /// Returns `None` for text that does not start with a status word and for
    /// an expression that does not read — both extraction artefacts, which the
    /// validator treats as unknown rather than as an unconditional status.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut it = text.split_whitespace();
        let word = it.next()?;
        let kind = match word {
            "Muss" => StatusKind::Muss,
            "Soll" => StatusKind::Soll,
            "Kann" => StatusKind::Kann,
            "X" => StatusKind::X,
            "M" => StatusKind::M,
            "S" => StatusKind::S,
            "K" => StatusKind::K,
            _ => return None,
        };
        let rest: Vec<&str> = it.collect();
        let expr = if rest.is_empty() {
            None
        } else {
            Some(Expr::parse(&rest.join(" ")).ok()?)
        };
        Some(Self { kind, expr })
    }
}

/// A Bedingung expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// `[10]`, `[UB1]`, `[1P0..1]`
    Cond(String),
    /// Juxtaposition — `[931] [494]`. Allgemeine Festlegungen 6.1d Kap. 6.4.6:
    /// „Zwischen Formatbedingungen und Voraussetzungen wird kein Operator
    /// genutzt“, and the Formatbedingung applies when the Voraussetzungen
    /// right of it are met. It binds tighter than `∧`, so the `[901]` of
    /// `[2] ∧ ([3] ∨ [4])[901] ∧ [555]` attaches to the bracketed group.
    Then(Vec<Expr>),
    /// `∧` — all.
    And(Vec<Expr>),
    /// `∨` — at least one.
    Or(Vec<Expr>),
    /// `⊻` — exactly one.
    Xor(Vec<Expr>),
}

/// Why an expression does not read.
///
/// The AHB tables are extracted from a PDF, where a column break can cut an
/// expression in half. Such a fragment parses into a different, valid-looking
/// expression unless it is refused here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprError {
    /// A character that is neither a Bedingung, an operator nor a parenthesis.
    Stray(char),
    /// `[` with no `]`.
    Unterminated,
    /// `[]`.
    EmptyCondition,
    /// An operator with nothing to join, or two operators in a row.
    MissingOperand,
    /// The parentheses do not balance.
    Parentheses,
    /// No expression at all.
    Empty,
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stray(c) => write!(f, "stray character {c:?}"),
            Self::Unterminated => f.write_str("a `[` with no `]`"),
            Self::EmptyCondition => f.write_str("an empty `[]`"),
            Self::MissingOperand => f.write_str("an operator with no operand"),
            Self::Parentheses => f.write_str("unbalanced parentheses"),
            Self::Empty => f.write_str("no expression"),
        }
    }
}

/// Why an expression that reads cannot be evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// `∨` or `⊻` between a Voraussetzung and a Formatbedingung, a
    /// Wiederholbarkeit or a Zeitpunktangabe. Allgemeine Festlegungen 6.1d
    /// Kap. 6.4.6 joins a Formatbedingung to a Voraussetzung by juxtaposition
    /// and a Zeitpunktangabe by `∧`; a choice between the two has no reading.
    ChoiceOverNeutral,
    /// A Paketvoraussetzung that leads back to its own Paket.
    PaketCycle(String),
    /// A Paketvoraussetzung that does not read.
    PaketExpression(String, ExprError),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChoiceOverNeutral => f.write_str(
                "∨/⊻ between a Voraussetzung and a Formatbedingung, Wiederholbarkeit or Zeitpunktangabe",
            ),
            Self::PaketCycle(p) => write!(f, "the Paketvoraussetzung of [{p}] cites [{p}]"),
            Self::PaketExpression(p, e) => {
                write!(f, "the Paketvoraussetzung of [{p}] does not read: {e}")
            }
        }
    }
}

/// A Paket citation `[kPn..m]` (Allgemeine Festlegungen 6.1d Kap. 6.9.2).
///
/// `k` is the Paketkennzeichen, whose Paketvoraussetzung says whether the
/// Paket applies at all; `n..m` is the Paketmerkmal, the minimal and maximal
/// repetition of the marked Qualifier/Code within the Paket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paket {
    /// The Paketkennzeichen, e.g. `1P`.
    pub id: String,
    /// Minimal repetition of the Qualifier/Code.
    pub min: usize,
    /// Maximal repetition; `None` where the AHB prints `n` for „nicht exakt
    /// angegeben“.
    pub max: Option<usize>,
}

impl Paket {
    /// Read `1P0..1`, `10P1..1`, `2P0..n` — the text between the brackets.
    #[must_use]
    pub fn parse(cited: &str) -> Option<Self> {
        let p = cited.find('P')?;
        let (number, rest) = cited.split_at(p);
        if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let id = format!("{number}P");
        let merkmal = &rest[1..];
        if merkmal.is_empty() {
            return Some(Self {
                id,
                min: 0,
                max: None,
            });
        }
        let (low, high) = merkmal.split_once("..")?;
        Some(Self {
            id,
            min: low.parse().ok()?,
            max: if high == "n" {
                None
            } else {
                Some(high.parse().ok()?)
            },
        })
    }
}

impl Expr {
    /// Parse an expression of bracketed Bedingungen joined by `∧`, `∨`, `⊻`,
    /// by juxtaposition, and grouped by parentheses.
    ///
    /// Juxtaposition binds tighter than the three operators. Mixed operators
    /// group left to right; Allgemeine Festlegungen 6.1d Kap. 6.4.6 requires
    /// parentheses there („ist eine Gewichtung durch Nutzung runder Klammern
    /// vorgegeben“).
    ///
    /// # Errors
    ///
    /// When the text is not an expression — see [`ExprError`].
    pub fn parse(text: &str) -> Result<Self, ExprError> {
        let toks = lex(text)?;
        if toks.is_empty() {
            return Err(ExprError::Empty);
        }
        let mut pos = 0;
        let e = parse_chain(&toks, &mut pos)?;
        if pos != toks.len() {
            return Err(ExprError::Parentheses);
        }
        Ok(e)
    }

    /// Every Bedingung number the expression cites.
    #[must_use]
    pub fn cited(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    /// Every Paket the expression cites, with its Paketmerkmal.
    #[must_use]
    pub fn pakete(&self) -> Vec<Paket> {
        self.cited()
            .into_iter()
            .filter(|id| ConditionKind::of(id) == ConditionKind::Paket)
            .filter_map(Paket::parse)
            .collect()
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Expr::Cond(c) => out.push(c),
            Expr::Then(v) | Expr::And(v) | Expr::Or(v) | Expr::Xor(v) => {
                for e in v {
                    e.collect(out);
                }
            }
        }
    }

    /// Whether every Bedingung this cites is a Hinweis.
    ///
    /// Allgemeine Festlegungen 6.1d Kap. 6.4.6: the part of a `⊻`/`∨`/`∧`
    /// connection carrying a number between 500 and 899 „stellt … immer nur
    /// einen Hinweis als solchen dar und ist damit nicht Bestandteil der
    /// einzuhaltenden Voraussetzung“.
    fn is_hinweis(&self) -> bool {
        let cited = self.cited();
        !cited.is_empty()
            && cited
                .iter()
                .all(|id| ConditionKind::of(id) == ConditionKind::Hinweis)
    }

    /// Whether this cites anything that gates the place — a Voraussetzung or
    /// a Paket. A Formatbedingung, a Wiederholbarkeit and a Zeitpunktangabe
    /// do not (Kap. 6.4).
    fn gates(&self) -> bool {
        self.cited().iter().any(|id| {
            matches!(
                ConditionKind::of(id),
                ConditionKind::Voraussetzung | ConditionKind::Paket
            )
        })
    }

    /// Evaluate with `oracle` answering each Bedingung.
    ///
    /// `Neutral` is the neutral element of `∧` and of juxtaposition. A `∨` or
    /// `⊻` drops the Hinweis parts and then admits either all-`Neutral`
    /// operands or none.
    ///
    /// # Errors
    ///
    /// When the oracle fails, or the expression joins a Voraussetzung and a
    /// Formatbedingung, Wiederholbarkeit or Zeitpunktangabe by `∨`/`⊻`.
    pub fn eval(
        &self,
        oracle: &mut dyn FnMut(&str) -> Result<Truth, EvalError>,
    ) -> Result<Truth, EvalError> {
        match self {
            Expr::Cond(c) => oracle(c),
            // Kap. 6.4.6: a Formatbedingung stands before the Voraussetzungen
            // it applies under, and contributes nothing to whether the place
            // is required — the same reading `∧` gives a neutral operand.
            Expr::Then(v) | Expr::And(v) => {
                let mut vals = Vec::with_capacity(v.len());
                for e in v {
                    vals.push(e.eval(oracle)?);
                }
                Ok(if vals.contains(&Truth::False) {
                    Truth::False
                } else if vals.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else if vals.iter().all(|t| *t == Truth::Neutral) {
                    Truth::Neutral
                } else {
                    Truth::True
                })
            }
            Expr::Or(v) => {
                let vals = choice_values(v, oracle)?;
                Ok(if vals.is_empty() {
                    Truth::Neutral
                } else if vals.contains(&Truth::True) {
                    Truth::True
                } else if vals.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else {
                    Truth::False
                })
            }
            // Kap. 6.4.6: „genau nur eine Bedingung bzw. eine geklammerte
            // Aussage … darf erfüllt sein“ — two met operands settle it
            // whatever the rest is.
            Expr::Xor(v) => {
                let vals = choice_values(v, oracle)?;
                if vals.is_empty() {
                    return Ok(Truth::Neutral);
                }
                let trues = vals.iter().filter(|t| **t == Truth::True).count();
                Ok(if trues >= 2 {
                    Truth::False
                } else if vals.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else if trues == 1 {
                    Truth::True
                } else {
                    Truth::False
                })
            }
        }
    }
}

/// The operands of a `∨`/`⊻` that carry a truth value.
///
/// Kap. 6.4.6 drops the Hinweis parts. What is left has to agree on whether
/// it gates the place at all: a choice between a Voraussetzung and a
/// Formatbedingung, a Wiederholbarkeit or a Zeitpunktangabe is not a choice
/// the Allgemeine Festlegungen give a reading — they join a Formatbedingung
/// by juxtaposition and a Zeitpunktangabe by `∧`. An operand that gates
/// nothing and stands among its own kind is the neutral element.
fn choice_values(
    operands: &[Expr],
    oracle: &mut dyn FnMut(&str) -> Result<Truth, EvalError>,
) -> Result<Vec<Truth>, EvalError> {
    let kept: Vec<&Expr> = operands.iter().filter(|e| !e.is_hinweis()).collect();
    let gating = kept.iter().filter(|e| e.gates()).count();
    if gating > 0 && gating < kept.len() {
        return Err(EvalError::ChoiceOverNeutral);
    }
    let mut vals = Vec::with_capacity(kept.len());
    for e in kept {
        let t = e.eval(oracle)?;
        if t != Truth::Neutral {
            vals.push(t);
        }
    }
    Ok(vals)
}

/// Binding strength: a citation, then juxtaposition, then the three operators.
fn precedence(e: &Expr) -> u8 {
    match e {
        Expr::Cond(_) => 3,
        Expr::Then(_) => 2,
        Expr::And(_) | Expr::Or(_) | Expr::Xor(_) => 1,
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn operand(f: &mut fmt::Formatter<'_>, e: &Expr) -> fmt::Result {
            if precedence(e) < 2 {
                write!(f, "({e})")
            } else {
                write!(f, "{e}")
            }
        }
        fn join(f: &mut fmt::Formatter<'_>, v: &[Expr], op: &str) -> fmt::Result {
            for (i, e) in v.iter().enumerate() {
                if i > 0 {
                    write!(f, "{op}")?;
                }
                operand(f, e)?;
            }
            Ok(())
        }
        match self {
            Expr::Cond(c) => write!(f, "[{c}]"),
            Expr::Then(v) => join(f, v, " "),
            Expr::And(v) => join(f, v, " ∧ "),
            Expr::Or(v) => join(f, v, " ∨ "),
            Expr::Xor(v) => join(f, v, " ⊻ "),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Cond(String),
    And,
    Or,
    Xor,
    Open,
    Close,
}

fn lex(text: &str) -> Result<Vec<Tok>, ExprError> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                let start = i + 1;
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(ExprError::Unterminated);
                }
                let id: String = chars[start..i].iter().collect();
                if id.trim().is_empty() {
                    return Err(ExprError::EmptyCondition);
                }
                out.push(Tok::Cond(id));
                i += 1;
            }
            '∧' => {
                out.push(Tok::And);
                i += 1;
            }
            '∨' => {
                out.push(Tok::Or);
                i += 1;
            }
            '⊻' => {
                out.push(Tok::Xor);
                i += 1;
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            c => return Err(ExprError::Stray(c)),
        }
    }
    Ok(out)
}

fn parse_atom(toks: &[Tok], pos: &mut usize) -> Result<Expr, ExprError> {
    match toks.get(*pos) {
        Some(Tok::Cond(c)) => {
            *pos += 1;
            Ok(Expr::Cond(c.clone()))
        }
        Some(Tok::Open) => {
            *pos += 1;
            let inner = parse_chain(toks, pos)?;
            if toks.get(*pos) != Some(&Tok::Close) {
                return Err(ExprError::Parentheses);
            }
            *pos += 1;
            Ok(inner)
        }
        _ => Err(ExprError::MissingOperand),
    }
}

/// A maximal run of adjacent operands — `[931] [494]`, `([3] ∨ [4])[901]`.
fn parse_juxtaposition(toks: &[Tok], pos: &mut usize) -> Result<Expr, ExprError> {
    let mut items = vec![parse_atom(toks, pos)?];
    while matches!(toks.get(*pos), Some(Tok::Cond(_) | Tok::Open)) {
        items.push(parse_atom(toks, pos)?);
    }
    if items.len() == 1 {
        Ok(items.remove(0))
    } else {
        Ok(Expr::Then(items))
    }
}

fn parse_chain(toks: &[Tok], pos: &mut usize) -> Result<Expr, ExprError> {
    let mut acc = parse_juxtaposition(toks, pos)?;
    while let Some(op) = toks
        .get(*pos)
        .filter(|t| matches!(t, Tok::And | Tok::Or | Tok::Xor))
        .cloned()
    {
        *pos += 1;
        let rhs = parse_juxtaposition(toks, pos)?;
        acc = match (op, acc) {
            (Tok::And, Expr::And(mut v)) => {
                v.push(rhs);
                Expr::And(v)
            }
            (Tok::And, a) => Expr::And(vec![a, rhs]),
            (Tok::Or, Expr::Or(mut v)) => {
                v.push(rhs);
                Expr::Or(v)
            }
            (Tok::Or, a) => Expr::Or(vec![a, rhs]),
            (Tok::Xor, Expr::Xor(mut v)) => {
                v.push(rhs);
                Expr::Xor(v)
            }
            (_, a) => Expr::Xor(vec![a, rhs]),
        };
    }
    Ok(acc)
}

/// The value of a Bedingung.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    /// Met.
    True,
    /// Not met.
    False,
    /// Cannot be decided from the message.
    Unknown,
    /// Not a Voraussetzung — a Hinweis, Format, Wiederholbarkeit or
    /// Zeitpunkt condition that never gates presence.
    Neutral,
}

/// What kind of Bedingung a number denotes (Kap. 6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionKind {
    /// `[1]`–`[499]`.
    Voraussetzung,
    /// `[500]`–`[899]`.
    Hinweis,
    /// `[901]`–`[999]`.
    Format,
    /// `[2000]`–`[2499]`.
    Wiederholbarkeit,
    /// `[UB1]`–`[UB3]`.
    Zeitpunkt,
    /// `[nP…]`.
    Paket,
    /// Outside every published range.
    Other,
}

impl ConditionKind {
    /// Classify by number range.
    #[must_use]
    pub fn of(id: &str) -> Self {
        if id.starts_with("UB") {
            return Self::Zeitpunkt;
        }
        if id.contains('P') {
            return Self::Paket;
        }
        match id.parse::<u32>() {
            Ok(1..=499) => Self::Voraussetzung,
            Ok(500..=899) => Self::Hinweis,
            Ok(900..=999) => Self::Format,
            Ok(2000..=2499) => Self::Wiederholbarkeit,
            _ => Self::Other,
        }
    }
}

/// A pattern of one segment as the Bedingungen cite it: `STS+7++xxx+ZW4`,
/// `CCI+Z61++ZF9`, `BGM+E03`, `DTM+471`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPattern {
    /// The segment tag.
    pub tag: String,
    /// Per element, per component: the admitted values, or `None` for any.
    pub elements: Vec<Vec<Option<Vec<String>>>>,
}

impl SegmentPattern {
    /// Parse `TAG+e1+e2…`; `xxx` and an empty element match anything, `A/B`
    /// lists alternatives, `?+` and `?:` are release-escaped separators.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim_end_matches([',', '.', ';', ')']);
        let (tag, rest) = match text.find('+') {
            Some(i) => (&text[..i], &text[i + 1..]),
            None => (text, ""),
        };
        if tag.len() != 3 || !tag.chars().all(|c| c.is_ascii_uppercase()) {
            return None;
        }
        let elements = split_unescaped(rest, '+')
            .into_iter()
            .map(|el| {
                split_unescaped(&el, ':')
                    .into_iter()
                    .map(|comp| {
                        let comp = comp.replace("?+", "+").replace("?:", ":");
                        if comp.is_empty()
                            || comp.eq_ignore_ascii_case("xxx")
                            || comp.eq_ignore_ascii_case("xxxx")
                        {
                            None
                        } else {
                            Some(comp.split('/').map(str::to_owned).collect())
                        }
                    })
                    .collect()
            })
            .collect();
        Some(Self {
            tag: tag.to_owned(),
            elements,
        })
    }

    /// Whether `seg` matches this pattern.
    #[must_use]
    pub fn matches(&self, seg: &edifact_rs::Segment<'_>) -> bool {
        if seg.tag != self.tag {
            return false;
        }
        self.elements.iter().enumerate().all(|(ei, comps)| {
            comps
                .iter()
                .enumerate()
                .all(|(ci, alternatives)| match alternatives {
                    None => true,
                    Some(alts) => seg
                        .component_str(ei, ci)
                        .is_some_and(|v| alts.iter().any(|a| code_matches(a, v))),
                })
        })
    }
}

/// A pattern value against a wire value: a lowercase letter in the pattern
/// stands for any one character (`1-b:1.9.e` names a family of OBIS-Kennzahlen).
fn code_matches(pattern: &str, value: &str) -> bool {
    if pattern == value {
        return true;
    }
    if !pattern.chars().any(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let (p, v): (Vec<char>, Vec<char>) = (pattern.chars().collect(), value.chars().collect());
    p.len() == v.len()
        && p.iter()
            .zip(&v)
            .all(|(pc, vc)| pc.is_ascii_lowercase() || pc == vc)
}

fn split_unescaped(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?'
            && let Some(&n) = chars.peek()
        {
            cur.push('?');
            cur.push(n);
            chars.next();
            continue;
        }
        if c == sep {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    if s.is_empty() {
        out.clear();
    }
    out
}

/// Where a cited segment is looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The whole message.
    Message,
    /// The nearest enclosing occurrence of the named group, else the message.
    Group(String),
}

/// A receiver-checkable Voraussetzung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Voraussetzung {
    /// „Wenn SG4 STS+7++xxx+ZAP vorhanden" / „… nicht vorhanden".
    Present {
        /// Where to look.
        scope: Scope,
        /// What to look for.
        pattern: SegmentPattern,
        /// „nicht vorhanden“.
        negate: bool,
    },
    /// „Wenn SG10 QTY DE6063 mit Wert 220 vorhanden" — or, with `suffix`,
    /// „… DE7140 bei der die letzten beiden Stellen mit dem Wert "01" …".
    ElementValue {
        /// Where to look.
        scope: Scope,
        /// The segment tag.
        tag: String,
        /// The data element number.
        de: String,
        /// The value it must carry.
        value: String,
        /// „nicht vorhanden“.
        negate: bool,
        /// Compare the last two characters only.
        suffix: bool,
    },
    /// „Wenn SG8 SEQ+ZH0 mehr als einmal vorhanden".
    Count {
        /// Where to count.
        scope: Scope,
        /// What to count.
        pattern: SegmentPattern,
        /// The count must exceed this.
        more_than: usize,
    },
}

/// Re-join an EDIFACT pattern the AHB printed across a line break: a space
/// right after `+`, `-`, `:` or `/` inside a token that looks like `TAG+…` is
/// removed (`PIA+5+1- 1?:1.9.0` → `PIA+5+1-1?:1.9.0`).
///
/// `/` separates the alternatives of one code list (`SEQ+Z04/ ZF7`), and the
/// AHBs set a space after it for readability. Left in, the alternatives after
/// the space are read as prose and the Voraussetzung matches only the first.
fn join_wrapped_pattern(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut in_pattern = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '+' && i >= 3 && chars[i - 3..i].iter().all(char::is_ascii_uppercase) {
            in_pattern = true;
        }
        if c == ' ' && in_pattern {
            let prev = out.chars().last();
            let next = chars.get(i + 1).copied();
            if matches!(prev, Some('+' | '-' | ':' | '/'))
                && next.is_some_and(|n| n.is_ascii_alphanumeric() || n == '?')
            {
                i += 1;
                continue;
            }
            in_pattern = false;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Whether a Bedingung's text is a precondition — „Wenn …" — as opposed to a
/// constraint on what the place carries once it is there.
#[must_use]
pub fn is_precondition(text: &str) -> bool {
    let t = text.trim_start().to_lowercase();
    t.starts_with("wenn ") || t.starts_with("falls ") || t.starts_with("sofern ")
}

/// `SG9` — or the AHB's `S9` — as a group name.
fn group_name(word: &str) -> Option<String> {
    let n = word.strip_prefix("SG").or_else(|| word.strip_prefix('S'))?;
    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then(|| format!("SG{n}"))
}

/// The group a demonstrative points at — „in dieser SG8", „im selben SG12".
fn named_scope(words: &[&str]) -> Option<String> {
    words
        .windows(2)
        .filter(|w| {
            matches!(
                w[0],
                "dieser" | "diesem" | "diese" | "derselben" | "demselben" | "dieselbe" | "selben"
            )
        })
        .find_map(|w| group_name(w[1].trim_end_matches([',', '.', ';'])))
}

/// Whether a word names a qualified segment — `NAD+MS`, `PIA+5`,
/// `CCI+Z30++Z07`: three uppercase letters and at least one qualifier.
fn names_qualified_segment(word: &str) -> bool {
    let word = word.trim_end_matches([',', '.', ';', ')']);
    let Some((tag, rest)) = word.split_once('+') else {
        return false;
    };
    !rest.is_empty() && tag.len() == 3 && tag.chars().all(|c| c.is_ascii_uppercase())
}

/// The group a Voraussetzung names as its scope and the index of the word that
/// carries the segment pattern (`PIA+Z02`, `QTY`).
///
/// „SG9" — or the AHB's „S9" — names a group; the first one named is the scope
/// („in derselben SG9 LIN das SG10 DTM+7 …"), unless a demonstrative points at
/// another one („Wenn SG10 CCI+6++ZA8 … in dieser SG8 vorhanden"), which names
/// the scope wherever it stands. A bare tag right after a group that is
/// followed by an article („SG27 LIN ein PIA+Z02", „S9 LIN das SG10 DTM+7")
/// only names the group; the pattern comes later.
fn locate_pattern(words: &[&str]) -> (Scope, Option<usize>) {
    let mut scope = named_scope(words).map_or(Scope::Message, Scope::Group);
    let mut tag_idx: Option<usize> = None;
    let mut after_group = false;
    let is_tag = |w: &str| {
        let seg_part = w.split('+').next().unwrap_or("");
        seg_part.len() == 3 && seg_part.chars().all(|c| c.is_ascii_uppercase())
    };
    for (i, w) in words.iter().enumerate() {
        let w = w.trim_end_matches([',', '.', ';']);
        // „SG9" — or the AHB's „S9" — names a group; the first one named
        // is the scope („in derselben SG9 LIN das SG10 DTM+7 …").
        if let Some(n) = group_name(w) {
            if scope == Scope::Message {
                scope = Scope::Group(n);
            }
            after_group = true;
            continue;
        }
        if i > 0 && is_tag(w) {
            // „in derselben SG27 LIN ein PIA+Z02 …" / „S9 LIN das SG10
            // DTM+7": a bare tag after the group, followed by an article,
            // only names the group; the pattern comes later.
            let article = words.get(i + 1).is_some_and(|n| {
                matches!(
                    *n,
                    "das" | "der" | "die" | "ein" | "eine" | "einen" | "kein" | "keine"
                )
            });
            if after_group
                && !w.contains('+')
                && article
                && words[i + 1..].iter().any(|later| is_tag(later))
            {
                after_group = false;
                continue;
            }
            tag_idx = Some(i);
            break;
        }
        after_group = false;
    }
    (scope, tag_idx)
}

impl Voraussetzung {
    /// Read the Voraussetzung shapes the AHBs use. `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        // A pattern the AHB wraps across lines comes back as `1- 1?:1.9.0`;
        // the space after a separator inside it is the page's, not the wire's.
        let joined = join_wrapped_pattern(text);
        let t = joined.trim();
        let lower = t.to_lowercase();
        if !lower.starts_with("wenn ") {
            return None;
        }
        if !lower.contains("vorhanden") {
            return None;
        }
        let words: Vec<&str> = t.split_whitespace().collect();
        let negate = lower.contains(" nicht vorhanden")
            || words
                .get(1)
                .is_some_and(|w| matches!(*w, "kein" | "keine" | "keinen" | "nicht"));
        // Scope: „in dieser SG4", „in derselben SG8", „im selben SG12", or a
        // leading group name before the segment.
        let (scope, tag_idx) = locate_pattern(&words);
        let ti = tag_idx?;
        // A Voraussetzung that names a second qualified segment („Wenn eine
        // andere SG8 SEQ+Z27 …, mit dem RFF+Z18 … referenziert, mit
        // PIA+5+9991000000078:Z11 … vorhanden ist") states a join between
        // places, not the presence of one. Kap. 6.5 leaves what the reader
        // cannot read undecided rather than answering it from the first
        // pattern alone.
        if words[ti + 1..].iter().any(|w| names_qualified_segment(w)) {
            return None;
        }
        let seg_word = words[ti].trim_end_matches([',', '.', ';', ')']);
        // „TAG DExxxx mit Wert v" — also „TAG (…) das DExxxx mit dem Wert v".
        let de_idx = words
            .iter()
            .enumerate()
            .skip(ti + 1)
            .find(|(_, w)| {
                w.len() == 6 && w.starts_with("DE") && w[2..].chars().all(|c| c.is_ascii_digit())
            })
            .map(|(i, _)| i);
        if let Some(de_word) = de_idx.and_then(|i| words.get(i))
            && (lower.contains(" mit wert ") || lower.contains(" mit dem wert "))
        {
            let v_idx = words.iter().position(|w| *w == "Wert")?;
            let value = words
                .get(v_idx + 1)?
                .trim_end_matches([',', '.', ';', ')'])
                .trim_matches(['"', '„', '“', '”'])
                .to_owned();
            let suffix = lower.contains("letzten beiden stellen");
            return Some(Self::ElementValue {
                suffix,
                scope,
                tag: seg_word[..3].to_owned(),
                de: de_word[2..].to_owned(),
                value,
                negate,
            });
        }
        let pattern = SegmentPattern::parse(seg_word)?;
        // Repetition: „mehr als einmal/zweimal/dreimal/viermal vorhanden".
        if let Some(i) = lower.find("mehr als ") {
            let n = lower[i + 9..].split_whitespace().next().unwrap_or("");
            let more_than = match n {
                "einmal" => 1,
                "zweimal" | "zwei" => 2,
                "dreimal" | "drei" => 3,
                "viermal" | "vier" => 4,
                _ => return None,
            };
            return Some(Self::Count {
                scope,
                pattern,
                more_than,
            });
        }
        if lower.contains("mal vorhanden") && !lower.contains("einmal vorhanden") {
            return None;
        }
        Some(Self::Present {
            scope,
            pattern,
            negate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_parse() {
        let s = Status::parse("Muss [10] ∧ [11]").unwrap();
        assert_eq!(s.kind, StatusKind::Muss);
        assert_eq!(s.expr.unwrap().to_string(), "[10] ∧ [11]");
        assert_eq!(Status::parse("X").unwrap().expr, None);
        assert!(Status::parse("[10] ∧").is_none());
        let s = Status::parse("Muss [7] ∧ ([577] ⊻ [UB1])").unwrap();
        assert_eq!(s.expr.unwrap().to_string(), "[7] ∧ ([577] ⊻ [UB1])");
        let s = Status::parse("X [931] [494]").unwrap();
        assert_eq!(s.expr.unwrap().to_string(), "[931] [494]");
    }

    #[test]
    fn juxtaposition_binds_tighter_than_the_operators() {
        // Kap. 6.4.6: the Formatbedingung attaches to the bracketed group
        // right of it, not to the whole chain.
        let e = Expr::parse("[2] ∧ ([3] ∨ [4])[901] ∧ [555]").unwrap();
        assert_eq!(e.to_string(), "[2] ∧ ([3] ∨ [4]) [901] ∧ [555]");
        let Expr::And(v) = &e else { panic!("{e}") };
        assert_eq!(v.len(), 3);
        assert!(matches!(v[1], Expr::Then(_)), "{e}");
        // A juxtaposition inside a choice stays inside it.
        let e = Expr::parse("[5] ∨ [930][6]").unwrap();
        let Expr::Or(v) = &e else { panic!("{e}") };
        assert!(matches!(v[1], Expr::Then(_)), "{e}");
    }

    #[test]
    fn truncated_expressions_are_refused() {
        // A PDF column break cuts an expression in half; the fragment must
        // not parse into a different, valid-looking one.
        for text in [
            "[321]) ∨",
            "([67] ∧ ([529] ∨",
            "([UB1] ∧ ⊻ [272]",
            "[74]) ∨ ∧ [524]",
            "∨ [508]",
            "[492] ∧ [27] ∧",
        ] {
            assert!(Expr::parse(text).is_err(), "{text:?} must not read");
        }
        assert_eq!(Expr::parse("[10] & [11]"), Err(ExprError::Stray('&')));
        assert_eq!(Expr::parse("[10"), Err(ExprError::Unterminated));
        assert_eq!(Expr::parse("([10]"), Err(ExprError::Parentheses));
    }

    /// The oracle of a test: a fixed value per Bedingung, never an error.
    fn answers(f: impl Fn(&str) -> Truth) -> impl FnMut(&str) -> Result<Truth, EvalError> {
        move |c: &str| Ok(f(c))
    }

    #[test]
    fn expressions_evaluate_with_neutral_hinweise() {
        let e = Expr::parse("[10] ∧ [519]").unwrap();
        let mut o = answers(|c| match c {
            "10" => Truth::True,
            "519" => Truth::Neutral,
            _ => Truth::Unknown,
        });
        assert_eq!(e.eval(&mut o), Ok(Truth::True));
        let e = Expr::parse("[10] ⊻ [11]").unwrap();
        let mut o = answers(|c| if c == "10" { Truth::True } else { Truth::False });
        assert_eq!(e.eval(&mut o), Ok(Truth::True));
        let mut o = answers(|_| Truth::True);
        assert_eq!(e.eval(&mut o), Ok(Truth::False));
        let e = Expr::parse("[1] ∨ [2]").unwrap();
        let mut o = answers(|c| {
            if c == "1" {
                Truth::Unknown
            } else {
                Truth::False
            }
        });
        assert_eq!(e.eval(&mut o), Ok(Truth::Unknown));
        // Kap. 6.4.6: the Hinweis part of a choice is not part of the
        // Voraussetzung — it drops out instead of deciding the choice.
        let e = Expr::parse("[10] ∨ [519]").unwrap();
        let mut o = answers(|c| match c {
            "10" => Truth::False,
            _ => Truth::Neutral,
        });
        assert_eq!(e.eval(&mut o), Ok(Truth::False));
        // A Formatbedingung is joined by juxtaposition and a Zeitpunktangabe
        // by `∧`; a choice against one has no reading.
        let e = Expr::parse("[10] ∨ [931]").unwrap();
        let mut o = answers(|c| match c {
            "10" => Truth::True,
            _ => Truth::Neutral,
        });
        assert_eq!(e.eval(&mut o), Err(EvalError::ChoiceOverNeutral));
        // Two Formatbedingungen may exclude each other with `⊻`.
        let e = Expr::parse("[932] ⊻ [933]").unwrap();
        let mut o = answers(|_| Truth::Neutral);
        assert_eq!(e.eval(&mut o), Ok(Truth::Neutral));
    }

    #[test]
    fn and_keeps_the_reference_truth_table() {
        // The four-valued table Hochfrequenz's `ahbicht` states in
        // `ConditionFulfilledValue.__and__` (MIT): neutral is the identity,
        // and an unfulfilled operand settles the conjunction whatever the
        // rest is.
        let e = Expr::parse("[1] ∧ [2]").unwrap();
        for (a, b, want) in [
            (Truth::Unknown, Truth::False, Truth::False),
            (Truth::Unknown, Truth::True, Truth::Unknown),
            (Truth::Neutral, Truth::True, Truth::True),
            (Truth::Neutral, Truth::Neutral, Truth::Neutral),
            (Truth::True, Truth::True, Truth::True),
        ] {
            let mut o = answers(move |c| if c == "1" { a } else { b });
            assert_eq!(e.eval(&mut o), Ok(want), "{a:?} ∧ {b:?}");
        }
        let e = Expr::parse("[1] ∨ [2]").unwrap();
        for (a, b, want) in [
            (Truth::Unknown, Truth::False, Truth::Unknown),
            (Truth::Unknown, Truth::True, Truth::True),
            (Truth::False, Truth::False, Truth::False),
        ] {
            let mut o = answers(move |c| if c == "1" { a } else { b });
            assert_eq!(e.eval(&mut o), Ok(want), "{a:?} ∨ {b:?}");
        }
    }

    #[test]
    fn paketmerkmale_read() {
        assert_eq!(
            Paket::parse("1P0..1"),
            Some(Paket {
                id: "1P".into(),
                min: 0,
                max: Some(1)
            })
        );
        assert_eq!(
            Paket::parse("10P1..1"),
            Some(Paket {
                id: "10P".into(),
                min: 1,
                max: Some(1)
            })
        );
        assert_eq!(
            Paket::parse("2P0..n"),
            Some(Paket {
                id: "2P".into(),
                min: 0,
                max: None
            })
        );
        assert_eq!(Paket::parse("UB1"), None);
        let e = Expr::parse("[2P1..2] ⊻ [3P0..2]").unwrap();
        let ids: Vec<String> = e.pakete().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, ["2P", "3P"]);
    }

    #[test]
    fn condition_kinds_by_range() {
        assert_eq!(ConditionKind::of("10"), ConditionKind::Voraussetzung);
        assert_eq!(ConditionKind::of("519"), ConditionKind::Hinweis);
        assert_eq!(ConditionKind::of("931"), ConditionKind::Format);
        assert_eq!(ConditionKind::of("2061"), ConditionKind::Wiederholbarkeit);
        assert_eq!(ConditionKind::of("UB1"), ConditionKind::Zeitpunkt);
        assert_eq!(ConditionKind::of("1P0..1"), ConditionKind::Paket);
    }

    fn seg(edi: &str) -> edifact_rs::OwnedSegment {
        edifact_rs::from_bytes(edi.as_bytes())
            .next()
            .unwrap()
            .unwrap()
            .into_owned()
    }

    #[test]
    fn segment_patterns_match_the_wire() {
        let p = SegmentPattern::parse("STS+7++xxx+xxx+E01/E03").unwrap();
        assert!(p.matches(&seg("STS+7++E01+ZW4+E03'")));
        assert!(!p.matches(&seg("STS+7++E01+ZW4'")));
        let p = SegmentPattern::parse("STS+7++xxx+ZAP").unwrap();
        assert!(p.matches(&seg("STS+7++E01+ZAP'")));
        assert!(!p.matches(&seg("STS+7++E01+ZW4'")));
        let p = SegmentPattern::parse("CCI+Z61++ZF9").unwrap();
        assert!(p.matches(&seg("CCI+Z61++ZF9'")));
        let p = SegmentPattern::parse("PIA+5+1-0?:56.5.54").unwrap();
        assert!(p.matches(&seg("PIA+5+1-0?:56.5.54'")));
        let p = SegmentPattern::parse("SG4").is_none();
        assert!(p);
    }

    #[test]
    fn voraussetzungen_parse() {
        let v = Voraussetzung::parse(
            "Wenn SG4 STS+7++xxx+xxx+E01/E03 (Transaktionsgrund befristete Anmeldung) vorhanden",
        )
        .unwrap();
        assert!(
            matches!(v, Voraussetzung::Present { scope: Scope::Group(ref g), negate: false, .. } if g == "SG4")
        );
        let v = Voraussetzung::parse(
            "Wenn in derselben SG10 das CCI+Z61++ZF9 (Kunde erfüllt …) vorhanden",
        )
        .unwrap();
        assert!(
            matches!(v, Voraussetzung::Present { scope: Scope::Group(ref g), .. } if g == "SG10")
        );
        let v = Voraussetzung::parse("Wenn SG10 QTY DE6063 mit Wert 220 vorhanden").unwrap();
        assert!(
            matches!(v, Voraussetzung::ElementValue { ref de, ref value, .. } if de == "6063" && value == "220")
        );
        let v = Voraussetzung::parse("Wenn SG8 SEQ+ZH0 (Priorisierung erforderliches Produktpaket) mehr als einmal vorhanden").unwrap();
        assert!(matches!(v, Voraussetzung::Count { more_than: 1, .. }));
        let v =
            Voraussetzung::parse("Wenn kein SG6 RFF+Z47 (Verwendungszeitraum) vorhanden").unwrap();
        assert!(matches!(v, Voraussetzung::Present { negate: true, .. }));
        let v = Voraussetzung::parse("Wenn SG4 STS+E01++A06 (Status der Antwort) nicht vorhanden")
            .unwrap();
        assert!(matches!(v, Voraussetzung::Present { negate: true, .. }));
        assert!(Voraussetzung::parse("Wenn Anmeldung/Änderung befristet").is_none());
        assert!(
            Voraussetzung::parse("Hinweis: Wenn in der Anmeldung der Code ZAP vorhanden war")
                .is_none()
        );
    }
}

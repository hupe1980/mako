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
//! | `[nP…]` | Paket | a choice among alternatives |
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
    /// Returns `None` for text that does not start with a status word — an
    /// extraction artefact, which the validator treats as unknown.
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
            Expr::parse(&rest.join(" "))
        };
        Some(Self { kind, expr })
    }
}

/// A Bedingung expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// `[10]`, `[UB1]`, `[1P0..1]`
    Cond(String),
    /// `∧` — all.
    And(Vec<Expr>),
    /// `∨` — at least one.
    Or(Vec<Expr>),
    /// `⊻` — exactly one.
    Xor(Vec<Expr>),
}

impl Expr {
    /// Parse an expression of bracketed Bedingungen joined by `∧`, `∨`, `⊻`
    /// with parentheses. Mixed operators without parentheses group left to
    /// right (the AHBs parenthesise them).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let toks = lex(text);
        let mut pos = 0;
        let e = parse_chain(&toks, &mut pos)?;
        Some(e)
    }

    /// Every Bedingung number the expression cites.
    #[must_use]
    pub fn cited(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Expr::Cond(c) => out.push(c),
            Expr::And(v) | Expr::Or(v) | Expr::Xor(v) => v.iter().for_each(|e| e.collect(out)),
        }
    }

    /// Evaluate with `oracle` answering each Bedingung.
    pub fn eval(&self, oracle: &mut dyn FnMut(&str) -> Truth) -> Truth {
        match self {
            Expr::Cond(c) => oracle(c),
            Expr::And(v) => {
                let vals: Vec<Truth> = v.iter().map(|e| e.eval(oracle)).collect();
                if vals.contains(&Truth::False) {
                    Truth::False
                } else if vals.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else if vals.iter().all(|t| *t == Truth::Neutral) {
                    Truth::Neutral
                } else {
                    Truth::True
                }
            }
            Expr::Or(v) => {
                let vals: Vec<Truth> = v
                    .iter()
                    .map(|e| e.eval(oracle))
                    .filter(|t| *t != Truth::Neutral)
                    .collect();
                if vals.is_empty() {
                    Truth::Neutral
                } else if vals.contains(&Truth::True) {
                    Truth::True
                } else if vals.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else {
                    Truth::False
                }
            }
            Expr::Xor(v) => {
                let vals: Vec<Truth> = v
                    .iter()
                    .map(|e| e.eval(oracle))
                    .filter(|t| *t != Truth::Neutral)
                    .collect();
                if vals.is_empty() {
                    return Truth::Neutral;
                }
                let trues = vals.iter().filter(|t| **t == Truth::True).count();
                if trues >= 2 {
                    Truth::False
                } else if vals.contains(&Truth::Unknown) {
                    Truth::Unknown
                } else if trues == 1 {
                    Truth::True
                } else {
                    Truth::False
                }
            }
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn join(f: &mut fmt::Formatter<'_>, v: &[Expr], op: &str) -> fmt::Result {
            for (i, e) in v.iter().enumerate() {
                if i > 0 {
                    write!(f, " {op} ")?;
                }
                match e {
                    Expr::Cond(_) => write!(f, "{e}")?,
                    _ => write!(f, "({e})")?,
                }
            }
            Ok(())
        }
        match self {
            Expr::Cond(c) => write!(f, "[{c}]"),
            Expr::And(v) => join(f, v, "∧"),
            Expr::Or(v) => join(f, v, "∨"),
            Expr::Xor(v) => join(f, v, "⊻"),
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

fn lex(text: &str) -> Vec<Tok> {
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
                out.push(Tok::Cond(chars[start..i.min(chars.len())].iter().collect()));
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
            _ => i += 1,
        }
    }
    out
}

fn parse_atom(toks: &[Tok], pos: &mut usize) -> Option<Expr> {
    match toks.get(*pos)? {
        Tok::Cond(c) => {
            *pos += 1;
            Some(Expr::Cond(c.clone()))
        }
        Tok::Open => {
            *pos += 1;
            let inner = parse_chain(toks, pos)?;
            if toks.get(*pos) == Some(&Tok::Close) {
                *pos += 1;
            }
            Some(inner)
        }
        // A stray operator or close paren: skip it.
        _ => {
            *pos += 1;
            parse_atom(toks, pos)
        }
    }
}

fn parse_chain(toks: &[Tok], pos: &mut usize) -> Option<Expr> {
    let mut acc = parse_atom(toks, pos)?;
    while let Some(op) = toks.get(*pos) {
        let op = match op {
            Tok::And | Tok::Or | Tok::Xor => op.clone(),
            Tok::Close => break,
            // Two conditions with no operator between them (`X [931] [494]`)
            // are both required.
            Tok::Cond(_) | Tok::Open => Tok::And,
        };
        if matches!(op, Tok::And | Tok::Or | Tok::Xor)
            && toks.get(*pos) != Some(&Tok::Cond(String::new()))
            && matches!(toks.get(*pos), Some(Tok::And | Tok::Or | Tok::Xor))
        {
            *pos += 1;
        }
        let Some(rhs) = parse_atom(toks, pos) else {
            break;
        };
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
    Some(acc)
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
/// right after `+`, `-` or `:` inside a token that looks like `TAG+…` is
/// removed (`PIA+5+1- 1?:1.9.0` → `PIA+5+1-1?:1.9.0`).
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
            if matches!(prev, Some('+' | '-' | ':'))
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

/// The group a Voraussetzung names as its scope and the index of the word that
/// carries the segment pattern (`PIA+Z02`, `QTY`).
///
/// „SG9" — or the AHB's „S9" — names a group; the first one named is the scope
/// („in derselben SG9 LIN das SG10 DTM+7 …"). A bare tag right after a group
/// that is followed by an article („SG27 LIN ein PIA+Z02", „S9 LIN das SG10
/// DTM+7") only names the group; the pattern comes later.
fn locate_pattern(words: &[&str]) -> (Scope, Option<usize>) {
    let mut scope = Scope::Message;
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
        if let Some(n) = w.strip_prefix("SG").or_else(|| w.strip_prefix('S'))
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            if scope == Scope::Message {
                scope = Scope::Group(format!("SG{n}"));
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
        assert_eq!(s.expr.unwrap().to_string(), "[931] ∧ [494]");
    }

    #[test]
    fn expressions_evaluate_with_neutral_hinweise() {
        let e = Expr::parse("[10] ∧ [519]").unwrap();
        let mut o = |c: &str| match c {
            "10" => Truth::True,
            "519" => Truth::Neutral,
            _ => Truth::Unknown,
        };
        assert_eq!(e.eval(&mut o), Truth::True);
        let e = Expr::parse("[10] ⊻ [11]").unwrap();
        let mut o = |c: &str| if c == "10" { Truth::True } else { Truth::False };
        assert_eq!(e.eval(&mut o), Truth::True);
        let mut o = |_: &str| Truth::True;
        assert_eq!(e.eval(&mut o), Truth::False);
        let e = Expr::parse("[1] ∨ [2]").unwrap();
        let mut o = |c: &str| {
            if c == "1" {
                Truth::Unknown
            } else {
                Truth::False
            }
        };
        assert_eq!(e.eval(&mut o), Truth::Unknown);
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

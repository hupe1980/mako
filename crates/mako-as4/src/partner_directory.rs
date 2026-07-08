//! BDEW MaKo trading-partner AS4 endpoint directory.
//!
//! [`PartnerDirectory`] maps 13-digit GLN codes to HTTPS AS4 endpoint URLs.
//! It is populated at startup from CLI/config pairs and used by the outbound
//! AS4 sender to resolve the delivery endpoint for each outbox message.
//!
//! ## Example
//!
//! ```rust
//! use mako_as4::partner_directory::PartnerDirectory;
//!
//! let pairs = vec![
//!     "9900000000002=https://partner.example/as4/inbox".to_string(),
//! ];
//! let dir = PartnerDirectory::from_cli_pairs(&pairs).unwrap();
//! assert_eq!(dir.endpoint("9900000000002"), Some("https://partner.example/as4/inbox"));
//! ```

use std::collections::HashMap;

/// Registry mapping trading-partner GLN codes to their AS4 endpoint URLs.
///
/// A GLN must appear in this directory before an AS4 sender can deliver
/// messages to that trading partner.
#[derive(Debug, Default, Clone)]
pub struct PartnerDirectory {
    endpoints: HashMap<Box<str>, Box<str>>,
}

/// Error returned by [`PartnerDirectory::from_cli_pairs`].
#[derive(Debug)]
pub struct PartnerDirectoryParseError(pub String);

impl std::fmt::Display for PartnerDirectoryParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid AS4 partner entry: {}", self.0)
    }
}

impl std::error::Error for PartnerDirectoryParseError {}

impl PartnerDirectory {
    /// Parse `["GLN=HTTPS-URL", …]` CLI / config pairs into a directory.
    ///
    /// Each entry must be `<GLN>=<HTTPS-URL>`.  Returns an error on the first
    /// malformed or insecure (non-HTTPS) entry.
    ///
    /// # Errors
    ///
    /// - Entry does not contain `=`
    /// - GLN part is empty
    /// - URL does not start with `https://`
    pub fn from_cli_pairs(pairs: &[String]) -> Result<Self, PartnerDirectoryParseError> {
        let mut endpoints = HashMap::new();
        for pair in pairs {
            let (mp_id, url) = pair.split_once('=').ok_or_else(|| {
                PartnerDirectoryParseError(format!("{pair:?} — expected format <GLN>=<HTTPS-URL>"))
            })?;
            let mp_id = mp_id.trim();
            let url = url.trim();
            if mp_id.is_empty() {
                return Err(PartnerDirectoryParseError(format!(
                    "{pair:?} — GLN must not be empty"
                )));
            }
            if !url.starts_with("https://") {
                return Err(PartnerDirectoryParseError(format!(
                    "{pair:?} — endpoint URL must use HTTPS (got {url:?})"
                )));
            }
            endpoints.insert(mp_id.into(), url.into());
        }
        Ok(Self { endpoints })
    }

    /// Look up the AS4 endpoint URL for a trading partner identified by GLN.
    ///
    /// Returns `None` when no endpoint is registered for `gln`.
    pub fn endpoint(&self, mp_id: &str) -> Option<&str> {
        self.endpoints.get(mp_id).map(|s| s.as_ref())
    }

    /// Returns `true` if no partner endpoints are registered.
    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }

    /// Iterate over all registered `(GLN, endpoint_url)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.endpoints.iter().map(|(k, v)| (k.as_ref(), v.as_ref()))
    }

    /// Total number of registered partners.
    pub fn len(&self) -> usize {
        self.endpoints.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_single_pair() {
        let pairs = vec!["9900000000002=https://partner.example/as4/inbox".to_string()];
        let dir = PartnerDirectory::from_cli_pairs(&pairs).unwrap();
        assert_eq!(
            dir.endpoint("9900000000002"),
            Some("https://partner.example/as4/inbox")
        );
        assert_eq!(dir.len(), 1);
        assert!(!dir.is_empty());
    }

    #[test]
    fn whitespace_trimmed() {
        let pairs = vec!["  9900000000002  =  https://partner.example/as4  ".to_string()];
        let dir = PartnerDirectory::from_cli_pairs(&pairs).unwrap();
        assert_eq!(
            dir.endpoint("9900000000002"),
            Some("https://partner.example/as4")
        );
    }

    #[test]
    fn missing_equals_is_error() {
        let pairs = vec!["9900000000002_https://partner.example/as4".to_string()];
        assert!(PartnerDirectory::from_cli_pairs(&pairs).is_err());
    }

    #[test]
    fn empty_gln_is_error() {
        let pairs = vec!["=https://partner.example/as4".to_string()];
        assert!(PartnerDirectory::from_cli_pairs(&pairs).is_err());
    }

    #[test]
    fn non_https_url_is_error() {
        let pairs = vec!["9900000000002=http://partner.example/as4".to_string()];
        assert!(PartnerDirectory::from_cli_pairs(&pairs).is_err());
    }

    #[test]
    fn empty_input_gives_empty_directory() {
        let dir = PartnerDirectory::from_cli_pairs(&[]).unwrap();
        assert!(dir.is_empty());
        assert_eq!(dir.len(), 0);
    }
}

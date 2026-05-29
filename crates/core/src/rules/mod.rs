//! Import-time automatic classification rules.
//!
//! Rules are loaded from `library/.localref/rules.toml` and evaluated against a
//! single imported metadata record. This crate only returns category paths; it
//! never writes `Cat/`, metadata, or the query database.
//!
//! # `rules.toml` format
//!
//! Rules are declared as an array of TOML tables:
//!
//! ```toml
//! [[rules]]
//! name = "near-field"
//! target = "Wireless/RIS"
//! query = 'title:/near[- ]field/i OR abstract:channel OR tags:RIS'
//! ```
//!
//! `name` is a human-readable rule label. `target` is a slash-separated
//! category path relative to `Cat/`, such as `Wireless/RIS`. `query` is matched
//! against one imported item.
//!
//! # Query grammar
//!
//! The current phase-one grammar is intentionally small:
//!
//! ```text
//! query   = atom (" OR " atom)*
//! atom    = field ":" matcher
//! matcher = substring | "/" regex "/" flags
//! flags   = "" | "i"
//! ```
//!
//! `OR` must be uppercase and surrounded by one space on each side. There is no
//! `AND`, grouping, negation, precedence, phrase operator, or cross-item query.
//! Empty fields or empty matchers are rejected.
//!
//! # Fields
//!
//! Supported fields are:
//!
//! - `title`
//! - `abstract` or `abstract_note`
//! - `doi`
//! - `uri` or `url`
//! - `type` or `item_type`
//! - `venue`
//! - `year`
//! - `tags` or `tag`
//!
//! Unknown fields are valid syntax but simply produce no match. This lets old
//! daemons safely ignore rule fields that may be introduced by a newer config
//! authoring tool.
//!
//! # Matcher semantics
//!
//! A plain substring matcher is case-insensitive:
//!
//! ```toml
//! query = 'title:near field'
//! ```
//!
//! A regex matcher uses Rust's `regex` crate syntax. Regexes are case-sensitive
//! by default and become case-insensitive with the `i` flag:
//!
//! ```toml
//! query = 'title:/near[- ]field/i'
//! ```
//!
//! A rule matches when any atom in its `OR` expression matches. Matching rules
//! return their `target` category. Duplicate category targets are returned only
//! once, preserving the first matching rule order.

use std::fs;
use std::path::Path;

use crate::error::{LocalrefError, Result};
use crate::model::Metadata;
use crate::types::CategoryPath;
use regex::{Regex, RegexBuilder};
use serde::Deserialize;

/// A parsed set of import-time classification rules.
#[derive(Clone, Debug, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

/// One automatic classification rule.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct Rule {
    /// Human-readable rule name.
    pub name: String,
    /// Target category path relative to `Cat/`.
    pub target: String,
    /// Query expression matched against metadata.
    pub query: String,
}

/// Display-ready description of one parsed automatic-classification rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleSummary {
    /// Human-readable rule name.
    pub name: String,
    /// Target category path relative to `Cat/`.
    pub target: String,
    /// Query expression matched against metadata.
    pub query: String,
}

#[derive(Debug, Deserialize)]
struct RuleConfig {
    #[serde(default)]
    rules: Vec<Rule>,
}

enum Matcher {
    Substring { field: String, needle: String },
    Regex { field: String, regex: Regex },
}

impl RuleSet {
    /// Load `library/.localref/rules.toml`, returning an empty set if missing.
    pub fn load(library_root: impl AsRef<Path>) -> Result<Self> {
        let path = library_root.as_ref().join(".localref").join("rules.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .map_err(|source| LocalrefError::io(&path, source))?;
        Self::parse(&text)
    }

    /// Parse rules from TOML text.
    pub fn parse(text: &str) -> Result<Self> {
        let config: RuleConfig = toml::from_str(text)?;
        for rule in &config.rules {
            CategoryPath::new(&rule.target).ok_or_else(|| {
                LocalrefError::InvalidPathComponent {
                    component: rule.target.clone(),
                    reason: "invalid category path",
                }
            })?;
            parse_query(&rule.query)?;
        }
        Ok(Self { rules: config.rules })
    }

    /// Return display-ready summaries for every parsed rule.
    pub fn summaries(&self) -> Vec<RuleSummary> {
        self.rules
            .iter()
            .map(|rule| RuleSummary {
                name: rule.name.clone(),
                target: rule.target.clone(),
                query: rule.query.clone(),
            })
            .collect()
    }

    /// Return all categories matched by a metadata record.
    pub fn match_metadata(
        &self,
        metadata: &Metadata,
    ) -> Result<Vec<CategoryPath>> {
        let mut categories = Vec::new();
        for rule in &self.rules {
            if query_matches(&rule.query, metadata)? {
                let category =
                    CategoryPath::new(&rule.target).ok_or_else(|| {
                        LocalrefError::InvalidPathComponent {
                            component: rule.target.clone(),
                            reason: "invalid category path",
                        }
                    })?;
                if !categories.contains(&category) {
                    categories.push(category);
                }
            }
        }
        Ok(categories)
    }
}

fn query_matches(query: &str, metadata: &Metadata) -> Result<bool> {
    for matcher in parse_query(query)? {
        if matcher.matches(metadata) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse_query(query: &str) -> Result<Vec<Matcher>> {
    let mut matchers = Vec::new();
    for atom in split_or(query) {
        let Some((field, pattern)) = atom.split_once(':') else {
            return Err(LocalrefError::Unsupported(
                "rule query atoms must use field:pattern",
            ));
        };
        let field = field.trim().to_ascii_lowercase();
        let pattern = pattern.trim();
        if field.is_empty() || pattern.is_empty() {
            return Err(LocalrefError::Unsupported("empty rule query atom"));
        }
        matchers.push(parse_matcher(field, pattern)?);
    }
    Ok(matchers)
}

fn split_or(query: &str) -> Vec<&str> {
    query
        .split(" OR ")
        .map(str::trim)
        .filter(|atom| !atom.is_empty())
        .collect()
}

fn parse_matcher(field: String, pattern: &str) -> Result<Matcher> {
    if pattern.starts_with('/') {
        let Some(last_slash) = pattern.rfind('/') else {
            return Err(LocalrefError::Unsupported("unterminated regex rule"));
        };
        if last_slash == 0 {
            return Err(LocalrefError::Unsupported("empty regex rule"));
        }
        let source = &pattern[1..last_slash];
        let flags = &pattern[last_slash + 1..];
        let regex = RegexBuilder::new(source)
            .case_insensitive(flags.contains('i'))
            .build()
            .map_err(|error| LocalrefError::Rule(error.to_string()))?;
        Ok(Matcher::Regex { field, regex })
    } else {
        Ok(Matcher::Substring { field, needle: pattern.to_ascii_lowercase() })
    }
}

impl Matcher {
    fn matches(&self, metadata: &Metadata) -> bool {
        match self {
            Matcher::Substring { field, needle } => {
                values_for_field(metadata, field)
                    .into_iter()
                    .any(|value| value.to_ascii_lowercase().contains(needle))
            }
            Matcher::Regex { field, regex } => {
                values_for_field(metadata, field)
                    .into_iter()
                    .any(|value| regex.is_match(&value))
            }
        }
    }
}

fn values_for_field(metadata: &Metadata, field: &str) -> Vec<String> {
    match field {
        "title" => vec![metadata.title.clone()],
        "abstract" | "abstract_note" => {
            metadata.abstract_note.clone().into_iter().collect()
        }
        "doi" => metadata.doi.clone().into_iter().collect(),
        "uri" | "url" => metadata.uri.clone().into_iter().collect(),
        "type" | "item_type" => vec![metadata.item_type.clone()],
        "venue" => metadata.venue.clone().into_iter().collect(),
        "year" => {
            metadata.year.map(|year| year.to_string()).into_iter().collect()
        }
        "tags" | "tag" => metadata.tags.items.clone(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MetadataFiles, MetadataImport, MetadataTags};

    #[test]
    fn matches_regex_and_substring_rules() {
        let rules = RuleSet::parse(
            r#"
[[rules]]
name = "near-field"
target = "Wireless/RIS"
query = 'title:/near[- ]field/i OR abstract:channel'

[[rules]]
name = "tag"
target = "Tagged"
query = 'tags:ris'
"#,
        )
        .unwrap();
        let metadata = Metadata {
            id: "lr:test:1".to_string(),
            item_type: "journalArticle".to_string(),
            title: "Near Field Paper".to_string(),
            abstract_note: Some("A channel model".to_string()),
            doi: None,
            uri: None,
            year: None,
            venue: None,
            language: None,
            creators: Vec::new(),
            files: MetadataFiles::default(),
            tags: MetadataTags { items: vec!["RIS".to_string()] },
            import: MetadataImport::default(),
            state: Default::default(),
            raw_connector: Default::default(),
        };

        let categories = rules.match_metadata(&metadata).unwrap();

        assert_eq!(categories.len(), 2);
        assert_eq!(categories[0].as_str(), "Wireless/RIS");
        assert_eq!(categories[1].as_str(), "Tagged");
    }

    #[test]
    fn returns_display_summaries_for_parsed_rules() {
        let rules = RuleSet::parse(
            r#"
[[rules]]
name = "near-field"
target = "Wireless/RIS"
query = 'title:RIS'
"#,
        )
        .unwrap();

        let summaries = rules.summaries();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].name, "near-field");
        assert_eq!(summaries[0].target, "Wireless/RIS");
        assert_eq!(summaries[0].query, "title:RIS");
    }
}

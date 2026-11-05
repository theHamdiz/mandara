//! Validated, data-only moderation rules for community repositories.

use serde::Deserialize;

/// A GitHub repository and its current publisher. IDs survive renames.
#[derive(Clone, Debug, Deserialize)]
pub struct Repository {
    #[serde(default)]
    pub id: Option<u64>,
    pub full_name: String,
    pub owner: Publisher,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Publisher {
    #[serde(default)]
    pub id: Option<u64>,
    pub login: String,
}

/// Version one accepts an empty document as an unrestricted policy.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    version: Option<u32>,
    publishers: Vec<PublisherRule>,
    plugins: Vec<PluginRule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublisherRule {
    github_id: Option<u64>,
    login: Option<String>,
    reason: Option<String>,
    #[serde(default = "enabled")]
    hide: bool,
    #[serde(default = "enabled")]
    deny_install: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginRule {
    repository_id: Option<u64>,
    repository: Option<String>,
    reason: Option<String>,
    #[serde(default = "enabled")]
    hide: bool,
    #[serde(default = "enabled")]
    deny_install: bool,
}

fn enabled() -> bool { true }

/// Matching rules combine: an account rule cannot be undone by a plugin rule.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decision<'a> {
    pub hidden: bool,
    pub denied: bool,
    pub reason: Option<&'a str>,
}

/// GitHub repository components, without URLs, traversal or query syntax.
pub(crate) fn valid_repository(name: &str) -> bool {
    let mut parts = name.split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(owner), Some(repo), None) if valid_login(owner) && !repo.is_empty() && repo != "." && repo != ".." && repo.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)))
}

fn valid_login(login: &str) -> bool {
    !login.is_empty() && login.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

/// Compares IDs when both are present, otherwise names without case. ID
/// precedence keeps a renamed account or repository distinct from a new
/// owner of its old name; callers validate required metadata separately.
fn matches(id: Option<u64>, name: Option<&str>, actual_id: Option<u64>, actual_name: &str) -> bool {
    match (id, actual_id) {
        (Some(wanted), Some(actual)) => wanted == actual,
        _ => name.is_some_and(|name| name.eq_ignore_ascii_case(actual_name)),
    }
}

impl Policy {
    /// Missing IDs cannot silently evade a numeric rule. GitHub supplies
    /// both IDs; a mirror omitting them must be fixed before proceeding.
    pub fn validate_identity(&self, repository: &Repository) -> Result<(), String> {
        if (repository.id.is_none() && self.plugins.iter().any(|rule| rule.repository_id.is_some()))
            || (repository.owner.id.is_none() && self.publishers.iter().any(|rule| rule.github_id.is_some())) {
            return Err("repository metadata is missing IDs required by the store policy".into());
        }
        Ok(())
    }

    /// Unknown fields, unsupported versions and missing selectors reject the
    /// whole update, preserving the previously validated policy.
    pub fn parse(text: &str) -> Result<Self, String> {
        let policy: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if policy.version.is_some_and(|version| version != 1) {
            return Err("unsupported store policy version".into());
        }
        for rule in &policy.publishers {
            if rule.github_id == Some(0) || rule.login.as_deref().is_some_and(|s| !valid_login(s)) || (rule.github_id.is_none() && rule.login.is_none()) {
                return Err("invalid publisher selector in store policy".into());
            }
        }
        for rule in &policy.plugins {
            if rule.repository_id == Some(0) || rule.repository.as_deref().is_some_and(|s| !valid_repository(s)) || (rule.repository_id.is_none() && rule.repository.is_none()) {
                return Err("invalid repository selector in store policy".into());
            }
        }
        Ok(policy)
    }

    /// Combines all matching account and repository restrictions; an allowing
    /// rule cannot cancel another rule's restriction. The first available
    /// denial reason borrows from this policy. Callers validate IDs first.
    pub fn decision<'a>(&'a self, repo: &Repository) -> Decision<'a> {
        let mut result = Decision::default();
        let mut add = |hide, deny, reason| {
            result.hidden |= hide;
            result.denied |= deny;
            if deny && result.reason.is_none() { result.reason = reason; }
        };
        for rule in &self.publishers {
            if matches(rule.github_id, rule.login.as_deref(), repo.owner.id, &repo.owner.login) {
                add(rule.hide, rule.deny_install, rule.reason.as_deref());
            }
        }
        for rule in &self.plugins {
            if matches(rule.repository_id, rule.repository.as_deref(), repo.id, &repo.full_name) {
                add(rule.hide, rule.deny_install, rule.reason.as_deref());
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Repository {
        Repository { id: Some(42), full_name: "alice/weather".into(), owner: Publisher { id: Some(7), login: "alice".into() } }
    }

    #[test]
    fn empty_policy_and_strict_schema() {
        assert_eq!(Policy::parse("").unwrap().decision(&repo()), Decision::default());
        for text in ["version = 2", "unknown = true", "[[plugins]]", "[[publishers]]\ngithub_id = 0", "[[plugins]]\nrepository = '../weather'", "[[publishers]]\nlogin = ''", "[[plugins]]\nrepository = 'alice/weather'\ndeny_instal = false"] {
            assert!(Policy::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn ids_follow_account_and_repository_renames_without_banning_reused_names() {
        let policy = Policy::parse("[[publishers]]\ngithub_id = 7\nlogin = 'alice'\nreason = 'abuse'").unwrap();
        let mut renamed = repo();
        renamed.owner.login = "new-name".into();
        assert!(policy.decision(&renamed).denied);
        renamed.owner.login = "alice".into();
        renamed.owner.id = Some(8);
        assert!(!policy.decision(&renamed).denied);
        let policy = Policy::parse("[[plugins]]\nrepository_id = 42\nrepository = 'alice/weather'").unwrap();
        renamed.full_name = "bob/renamed".into();
        assert!(policy.decision(&renamed).denied);
        renamed.id = Some(43);
        renamed.full_name = "alice/weather".into();
        assert!(!policy.decision(&renamed).denied);
    }

    #[test]
    fn hide_only_deny_only_and_account_rules_combine() {
        let policy = Policy::parse("[[plugins]]\nrepository = 'ALICE/WEATHER'\ndeny_install = false").unwrap();
        assert_eq!(policy.decision(&repo()), Decision { hidden: true, denied: false, reason: None });
        let policy = Policy::parse("[[publishers]]\nlogin = 'ALICE'\nhide = false\nreason = 'abuse'\n[[plugins]]\nrepository = 'alice/weather'\ndeny_install = false").unwrap();
        assert_eq!(policy.decision(&repo()), Decision { hidden: true, denied: true, reason: Some("abuse") });
        assert!(!policy.decision(&Repository { owner: Publisher { id: None, login: "bob".into() }, full_name: "bob/weather".into(), ..repo() }).denied);
    }

    #[test]
    fn numeric_rules_require_identity_metadata() {
        let policy = Policy::parse("[[publishers]]\ngithub_id = 7\n[[plugins]]\nrepository_id = 42").unwrap();
        assert!(policy.validate_identity(&repo()).is_ok());
        assert!(policy.validate_identity(&Repository { id: None, ..repo() }).is_err());
        assert!(policy.validate_identity(&Repository { owner: Publisher { id: None, login: "alice".into() }, ..repo() }).is_err());
    }
}

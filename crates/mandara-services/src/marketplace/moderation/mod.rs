//! Runtime moderation for the community store. Installed plugins, bundled
//! plugins and development folders are never changed by these rules.

pub mod cache;
pub mod policy;
pub mod transport;

use mandara_i18n::{t, tf};
use parking_lot::Mutex;
use policy::{Policy, Repository};

/// Policy providers let local integration fixtures stay independent of GitHub.
pub trait PolicySource {
    fn policy(&self, installing: bool) -> Result<Policy, String>;
}

pub struct Remote;

/// Serialize policy read/refresh/write so concurrent store actions cannot
/// overwrite a newer validated document with an older request's response.
static CACHE_LOCK: Mutex<()> = Mutex::new(());

impl PolicySource for Remote {
    fn policy(&self, installing: bool) -> Result<Policy, String> {
        let _lock = CACHE_LOCK.lock();
        let path = mandara_config::paths::cache_dir().join("marketplace-policy.json");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
        cache::load(&path, &transport::Github, now, installing).map_err(|error| {
            tracing::warn!("community store policy check failed: {error}");
            t("Couldn't verify the community store policy. Try again later.").to_string()
        })
    }
}

/// Check a direct repository install or a previously cached update plan.
pub fn authorize(full_name: &str, source: &dyn PolicySource) -> Result<(), String> {
    crate::offline::check()?;
    if !policy::valid_repository(full_name) { return Err(tf("Invalid repository {name}", &[("name", &full_name)])); }
    let policy = source.policy(true)?;
    let body = super::get(&format!("{}/repos/{full_name}", super::api_base()), super::MAX_JSON_BYTES).map_err(String::from)?;
    let repository: Repository = serde_json::from_slice(&body).map_err(|e| tf("Unexpected answer from GitHub: {error}", &[("error", &e)]))?;
    // Keep the marketplace's existing exact-repository ownership guarantee.
    if !repository.full_name.eq_ignore_ascii_case(full_name) {
        return Err(tf("{name} now points to another repository ({page}). Mandara doesn't follow moved plugins; install it again from its new home if you trust it.", &[("name", &full_name), ("page", &repository.full_name)]));
    }
    admit(&policy, &repository)
}

/// Admits an install or update only with sufficient identity metadata and
/// no matching deny rule. Hide-only rules allow direct installs; denials
/// include the maintainer's reason when one was supplied.
fn admit(policy: &Policy, repository: &Repository) -> Result<(), String> {
    policy.validate_identity(repository).map_err(|e| tf("Unexpected answer from GitHub: {error}", &[("error", &e)]))?;
    let decision = policy.decision(repository);
    if !decision.denied { return Ok(()); }
    let message = tf("The community store has blocked installs and updates for {repository}.", &[("repository", &repository.full_name)]);
    Err(match decision.reason.filter(|s| !s.is_empty()) {
        Some(reason) => format!("{message} {reason}"),
        None => message,
    })
}

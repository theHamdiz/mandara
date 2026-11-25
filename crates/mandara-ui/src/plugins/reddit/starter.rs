//! A curated starter list: well-known, safe-for-work communities by topic,
//! shown in Discover so a new reader has somewhere to begin.

use mandara_i18n::tk;

pub(super) struct Category {
    pub name: &'static str,
    pub icon: &'static str,
    pub communities: &'static [&'static str],
}

pub(super) const CATEGORIES: &[Category] = &[
    Category { name: tk("Front pages"), icon: "globe", communities: &["popular", "all", "bestof", "TodayILearned"] },
    Category { name: tk("Linux & open source"), icon: "terminal", communities: &["linux", "archlinux", "opensource", "linuxquestions", "wayland", "selfhosted"] },
    Category { name: tk("Programming"), icon: "workflow", communities: &["programming", "rust", "learnprogramming", "webdev", "Python", "ExperiencedDevs"] },
    Category { name: tk("Technology"), icon: "cpu", communities: &["technology", "hardware", "buildapc", "gadgets", "homelab"] },
    Category { name: tk("Science & space"), icon: "moon", communities: &["science", "space", "askscience", "Physics", "biology"] },
    Category { name: tk("News & ideas"), icon: "release-notes", communities: &["worldnews", "news", "UpliftingNews", "explainlikeimfive", "dataisbeautiful"] },
    Category { name: tk("Design & art"), icon: "palette", communities: &["design", "Art", "photography", "graphic_design", "itookapicture"] },
    Category { name: tk("Games"), icon: "dice", communities: &["gaming", "pcgaming", "linux_gaming", "boardgames", "indiegaming"] },
    Category { name: tk("Learning & life"), icon: "book", communities: &["LifeProTips", "YouShouldKnow", "personalfinance", "Cooking", "books"] },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_starter_community_is_a_valid_name_listed_once() {
        let mut seen = std::collections::HashSet::new();
        for category in CATEGORIES {
            assert!(!category.communities.is_empty());
            for name in category.communities {
                assert!(mandara_services::reddit::Subreddit::parse(name).is_some(), "{name}");
                assert!(seen.insert(name.to_ascii_lowercase()), "{name} is listed twice");
            }
        }
    }
}

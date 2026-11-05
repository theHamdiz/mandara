//! Small revision counters let views follow only the services they display.

/// Independent data sources stored by the hub. CPU samples already use a
/// separate entity and do not invalidate the hub.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Domains(u16);

/// Twelve service and compositor sources fit in one small mask.
const COUNT: usize = 12;

impl Domains {
    pub const NONE: Self = Self(0);
    pub const MEDIA: Self = Self(1 << 0);
    pub const NETWORK: Self = Self(1 << 1);
    pub const BLUETOOTH: Self = Self(1 << 2);
    pub const AUDIO: Self = Self(1 << 3);
    pub const CLIPBOARD: Self = Self(1 << 4);
    pub const NOTIFICATIONS: Self = Self(1 << 5);
    pub const WEATHER: Self = Self(1 << 6);
    pub const NAVIGATION: Self = Self(1 << 7);
    pub const FOCUS: Self = Self(1 << 8);
    pub const CLIENTS: Self = Self(1 << 9);
    pub const KEYBOARD: Self = Self(1 << 10);
    pub const PRIVACY: Self = Self(1 << 11);
    pub const ALL: Self = Self((1 << COUNT) - 1);

    pub const fn union(self, other: Self) -> Self { Self(self.0 | other.0) }
    pub const fn is_empty(self) -> bool { self.0 == 0 }

    fn indices(self) -> impl Iterator<Item = usize> {
        let mut mask = self.0;
        std::iter::from_fn(move || {
            if mask == 0 { return None; }
            let index = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            Some(index)
        })
    }
}

#[derive(Default)]
pub(super) struct Revisions([u64; COUNT]);

impl Revisions {
    pub fn bump(&mut self, domains: Domains) {
        for index in domains.indices() {
            self.0[index] = self.0[index].wrapping_add(1);
        }
    }

    /// A subscriber retains one integer, including when it follows several
    /// sources. A change to any selected source advances this revision.
    pub fn get(&self, domains: Domains) -> u64 {
        domains.indices().fold(0u64, |revision, index| revision.wrapping_add(self.0[index]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_sources_do_not_invalidate_each_other() {
        let mut revisions = Revisions::default();
        for index in 0..COUNT {
            let selected = Domains(1 << index);
            let before = revisions.get(selected);
            let others = Domains(Domains::ALL.0 & !selected.0);
            revisions.bump(others);
            assert_eq!(revisions.get(selected), before);
            revisions.bump(selected);
            assert_ne!(revisions.get(selected), before);
        }
        assert_eq!(Domains::ALL.indices().count(), COUNT);
        assert_eq!(revisions.get(Domains::NONE), 0);
    }

    #[test]
    fn a_multi_source_view_follows_either_input_without_cloning_data() {
        let sources = Domains::AUDIO.union(Domains::MEDIA);
        let mut revisions = Revisions::default();
        assert_eq!(revisions.get(sources), 0);
        revisions.bump(Domains::AUDIO);
        assert_eq!(revisions.get(sources), 1);
        revisions.bump(Domains::MEDIA);
        assert_eq!(revisions.get(sources), 2);
        revisions.bump(Domains::NOTIFICATIONS);
        assert_eq!(revisions.get(sources), 2);
        revisions.bump(sources);
        assert_eq!(revisions.get(sources), 4);
    }

    #[test]
    fn a_wrapping_counter_still_invalidates_the_selected_view() {
        let mut revisions = Revisions::default();
        revisions.0[0] = u64::MAX;
        let before = revisions.get(Domains::MEDIA.union(Domains::AUDIO));
        revisions.bump(Domains::MEDIA);
        assert_ne!(revisions.get(Domains::MEDIA.union(Domains::AUDIO)), before);
    }
}

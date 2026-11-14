use super::*;

struct Probe {
    host: Result<bool, ()>,
    writable: bool,
    tools: bool,
}
impl Inspector for Probe {
    fn tool(&self, _: &str) -> bool {
        self.tools
    }
    fn flake_has_host(&self, _: &Path, _: &str, _: &AtomicBool) -> Result<bool, ()> {
        self.host
    }
    fn writable_flake(&self, _: &Path, _: bool) -> bool {
        self.writable
    }
}

#[test]
fn supported_defaults_remain_enabled_and_unknown_flakes_only_lock_build_actions() {
    let root = tempfile::tempdir().unwrap();
    let paths = crate::sysupdate::fake::nixos_tree(root.path()).unwrap();
    let mut probe = Probe {
        host: Ok(true),
        writable: true,
        tools: true,
    };
    let inspect = |probe: &Probe| inspect_with(&paths, &AtomicBool::new(false), probe);
    assert_eq!(
        inspect(&probe),
        Availability {
            update: None,
            rebuild: None,
            rollback: None,
            generations: None,
            cleanup: None
        }
    );
    probe.host = Ok(false);
    assert_eq!(inspect(&probe).update, Some(Reason::MissingHost));
    assert_eq!(inspect(&probe).rebuild, Some(Reason::MissingHost));
    assert_eq!(
        inspect(&probe).rollback,
        None,
        "existing generations do not need a build source"
    );
    probe.host = Err(());
    assert_eq!(inspect(&probe).rebuild, Some(Reason::UnverifiedFlake));
    probe.host = Ok(true);
    probe.writable = false;
    assert_eq!(inspect(&probe).update, Some(Reason::ReadOnlyFlake));
    assert_eq!(
        inspect(&probe).rebuild,
        None,
        "a locked immutable flake can still be rebuilt"
    );
    probe.tools = false;
    assert_eq!(inspect(&probe).update, Some(Reason::MissingTools));
    assert_eq!(inspect(&probe).rollback, Some(Reason::MissingTools));
}

#[test]
fn a_missing_default_configuration_never_falls_back_to_blind_channel_updates() {
    let root = tempfile::tempdir().unwrap();
    let paths = NixPaths::under(root.path());
    let probe = Probe {
        host: Ok(true),
        writable: true,
        tools: true,
    };
    let result = inspect_with(&paths, &AtomicBool::new(false), &probe);
    assert_eq!(result.rebuild, Some(Reason::MissingConfiguration));
    assert_eq!(result.update, Some(Reason::MissingConfiguration));
    assert_eq!(result.rollback, Some(Reason::NoCurrentGeneration));
}

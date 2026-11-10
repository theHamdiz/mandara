//! The install flow on image-based systems ([`ImageSystem`]).
//!
//! Mandara itself goes into your home folder as everywhere. The system
//! packages it lacks (checked one by one against the running image, so
//! nothing it already ships is layered) are collected by the steps that need
//! them and added in one transaction at the end, after one question: the
//! runtime libraries, then fonts, portals and services, then greetd + cage
//! for Mandara Login and a compositor picked here. They arrive with the
//! next boot, so Mandara Login is switched to after the reboot. Everything
//! that needs no reboot is done first.

use super::*;

/// The packages collected for the next image.
#[derive(Default, Debug)]
pub(super) struct Layer {
    /// Missing runtime libraries: Mandara doesn't start without them.
    pub(super) required: Vec<String>,
    /// Missing fonts, portals and services.
    pub(super) optional: Vec<String>,
    /// greetd + cage for Mandara Login, and a compositor picked here.
    pub(super) login: Vec<String>,
    /// Every package the next boot's image requests (`rpm-ostree status`).
    pub(super) pending: Vec<String>,
    /// The ones of them Mandara wants, waiting for the reboot.
    pub(super) waiting: Vec<String>,
    /// The next boot brings packages already (requested earlier).
    pub(super) reboot_pending: bool,
    /// Mandara Login waits for greetd and cage from the next image.
    pub(super) login_after_reboot: bool,
}

impl Layer {
    /// Everything to add, each once, essential packages first.
    fn all(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for p in self.required.iter().chain(&self.login).chain(&self.optional) {
            if !out.contains(&p.as_str()) {
                out.push(p);
            }
        }
        out
    }

    /// What is left without the optional packages.
    fn essential(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for p in self.required.iter().chain(&self.login) {
            if !out.contains(&p.as_str()) {
                out.push(p);
            }
        }
        out
    }

    /// Whether `package` is on its way already (collected or pending).
    fn has(&self, package: &str) -> bool {
        self.pending.iter().chain(&self.required).chain(&self.optional).chain(&self.login).any(|p| p == package)
    }
}

impl Installer<'_> {
    /// The system's name as the messages show it.
    fn system_name(&self) -> String {
        self.facts.os.display_name().to_string()
    }

    /// Which of `wanted` the running image lacks (all of them when the
    /// query can't run: the tool then reports what it already has).
    fn image_missing<'p>(&mut self, image: ImageSystem, wanted: &[&'p str]) -> Vec<&'p str> {
        if wanted.is_empty() {
            return Vec::new();
        }
        match self.run(Cmd::from_argv(image.query_argv(wanted)).env("LC_ALL", "C").io(Io::Capture)) {
            Some(out) => image::missing_packages(image, &out.stdout, wanted),
            None => wanted.to_vec(),
        }
    }

    /// The `packages` the repositories carry, where the tool won't say
    /// before its transaction ([`ImageSystem::available_argv`]); the others
    /// are reported as skipped, as optional packages are elsewhere. All of
    /// them when the query can't run.
    fn image_available<'p>(&mut self, image: ImageSystem, packages: &[&'p str]) -> Vec<&'p str> {
        if packages.is_empty() {
            return Vec::new();
        }
        let Some(argv) = image.available_argv(packages, |p| self.has(p)) else { return packages.to_vec() };
        let Some(out) = self.run(Cmd::from_argv(argv).env("LC_ALL", "C").io(Io::Capture)).filter(|o| o.success) else { return packages.to_vec() };
        let available = image::available_packages(&out.stdout);
        let (kept, skipped): (Vec<&str>, Vec<&str>) = packages.iter().partition(|p| available.contains(p));
        for p in skipped {
            self.say(Level::Detail, format!("skipped {p} (not available)"));
        }
        kept
    }

    /// Says why `packages` can't be added here: SteamOS's image takes no
    /// packages, elsewhere the tool may be missing. True when they can.
    fn image_can_add(&mut self, image: ImageSystem, packages: &[&str]) -> bool {
        if packages.is_empty() {
            return true;
        }
        let list = packages.join(" ");
        if !image.takes_packages() {
            let system = self.system_name();
            self.say(Level::Warn, tf("{system} keeps its system image read-only, and the next system update removes packages added to it, so Mandara installs only into your home folder. Not available here: {packages}", &[("system", &system), ("packages", &list)]));
            return false;
        }
        if !self.has(image.program()) {
            self.say(Level::Warn, tf("{tool} isn't installed, so Mandara can't add system packages. Add these with your system's own tools: {packages}", &[("tool", &image.program()), ("packages", &list)]));
            return false;
        }
        true
    }

    /// The system-packages step on an image-based system: finds what is
    /// missing and collects it for [`Self::image_packages`]; nothing is
    /// installed yet.
    pub(super) fn image_deps(&mut self, image: ImageSystem, from_source: bool) -> Result<(), String> {
        let system = self.system_name();
        self.say(Level::Step, t("System packages"));
        // SteamOS takes no packages: what it lacks is listed below instead.
        if image.takes_packages() {
            self.say(Level::Info, tf("{system} is image-based: Mandara goes into your home folder, and only the system packages it lacks are added with {tool}, for the next boot.", &[("system", &system), ("tool", &image.program())]));
        }
        let family = self.facts.family;
        let Some(set) = packages::for_family(family) else {
            for line in packages::UNKNOWN_HINT {
                self.say(Level::Warn, *line);
            }
            return Ok(());
        };
        // A source build runs now, before any reboot: its tools must be
        // part of the running image.
        if from_source {
            let missing = self.image_missing(image, set.build);
            if !missing.is_empty() {
                return Err(tf("Building Mandara needs build tools this system's image doesn't have ({packages}). Build inside a toolbox or distrobox container, or install a release instead.", &[("packages", &missing.join(" "))]));
            }
        }
        if image == ImageSystem::RpmOstree
            && let Some(out) = self.run(Cmd::from_argv(OstreeStatus::argv()).env("LC_ALL", "C").io(Io::Capture))
            && let Some(status) = OstreeStatus::parse(&out.stdout)
        {
            self.layer.reboot_pending = status.reboot_pending;
            self.layer.pending = status.requested;
        }
        let portals = packages::portal_packages(family, |p| self.has(p));
        let optional: Vec<&str> = set.opt.iter().chain(set.svc).chain(portals.iter()).copied().collect();
        let required = self.image_missing(image, set.run);
        let optional = self.image_missing(image, &optional);
        let optional = self.image_available(image, &optional);
        let (required, optional) = (self.not_waiting(&required), self.not_waiting(&optional));
        if required.is_empty() && optional.is_empty() {
            if self.layer.waiting.is_empty() {
                self.say(Level::Ok, t("Every system package Mandara needs is already part of this system."));
            }
        } else {
            let wanted: Vec<&str> = required.iter().chain(&optional).map(String::as_str).collect();
            if self.image_can_add(image, &wanted) {
                self.layer.required = required;
                self.layer.optional = optional;
            }
        }
        self.services_hint();
        Ok(())
    }

    /// `missing` without what the next boot already brings (remembered in
    /// [`Layer::waiting`]).
    fn not_waiting(&mut self, missing: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for p in missing {
            if self.layer.pending.iter().any(|q| q == p) {
                if !self.layer.waiting.iter().any(|q| q == p) {
                    self.layer.waiting.push(p.to_string());
                }
            } else {
                out.push(p.to_string());
            }
        }
        out
    }

    /// Collects a compositor picked by the installer (and its portal).
    pub(super) fn image_compositor(&mut self, image: ImageSystem, c: &Compositor, package: &str) {
        if !self.image_can_add(image, &[package]) {
            let command = format!("mandara setup --compositor {}", c.id);
            self.say(Level::Info, tf("Install {compositor} yourself, then run: {command}", &[("compositor", &c.label), ("command", &command)]));
            return;
        }
        if !self.layer.has(package) {
            self.layer.login.push(package.to_string());
        }
        for portal in packages::portal_packages(self.facts.family, |p| c.programs.contains(&p)) {
            if !self.layer.has(portal) && self.image_missing(image, &[portal]).contains(&portal) {
                self.layer.optional.push(portal.to_string());
            }
        }
        self.args.compositor = Some(c.id.to_string());
        self.say(Level::Detail, tf("{compositor} ({package}) arrives with the next boot; pick it on your login screen then.", &[("compositor", &c.label), ("package", &package)]));
    }

    /// Mandara Login on an image-based system whose image lacks greetd or
    /// cage (`missing`, programs): their packages are collected, and the
    /// switch waits for the next boot, when they exist.
    pub(super) fn image_login(&mut self, image: ImageSystem, missing: &[&str]) {
        let family = self.facts.family;
        let wanted: Vec<String> = missing.iter().map(|p| packages::login_package(family, p)).collect();
        let wanted: Vec<&str> = wanted.iter().map(String::as_str).collect();
        if !self.image_can_add(image, &wanted) {
            return;
        }
        for p in self.not_waiting(&wanted) {
            if !self.layer.has(&p) {
                self.layer.login.push(p);
            }
        }
        // No switch plan yet: it is made after the reboot, against the image
        // that has greetd.
        self.layer.login_after_reboot = true;
    }

    /// The root-owned files that must survive updates (the admin helper,
    /// Mandara Login's copy) can't on this system: says so. True when they
    /// can be installed.
    pub(super) fn image_keeps_root_files(&mut self) -> bool {
        match self.facts.image {
            Some(image) if !image.keeps_usr_local() => {
                if !self.root_files_skipped {
                    let system = self.system_name();
                    self.say(Level::Warn, tf("{system} replaces its system files with every update, so the admin helper and Mandara Login are skipped; Studio's administrator actions stay unavailable.", &[("system", &system)]));
                    self.root_files_skipped = true;
                }
                false
            }
            _ => true,
        }
    }

    /// The last step on an image-based system: one question, then one
    /// transaction adding everything collected; then what the reboot
    /// brings. Optional packages a repository lacks fail the whole
    /// transaction, so a failure is retried with the essential ones.
    pub(super) fn image_packages(&mut self) {
        let Some(image) = self.facts.image else { return };
        let all: Vec<String> = self.layer.all().into_iter().map(str::to_string).collect();
        if !self.layer.waiting.is_empty() {
            let list = self.layer.waiting.join(" ");
            self.say(Level::Info, tf("Already waiting for the next boot: {packages}", &[("packages", &list)]));
        }
        if all.is_empty() {
            if self.layer.reboot_pending && !self.layer.waiting.is_empty() {
                self.say(Level::Info, t("Restart your computer to start using the new packages. Everything else is ready now."));
            }
            self.login_after_reboot();
            return;
        }
        self.say(Level::Step, t("Packages for the next boot"));
        let Some(argv) = image.add_argv(&all) else { return };
        let shown = self.root_hint(&argv);
        let question = tf("Add {count} packages with {tool}? They arrive with the next boot: {packages}", &[("count", &all.len()), ("tool", &image.program()), ("packages", &all.join(" "))]);
        if !self.ask(&question, true) {
            self.say(Level::Info, tf("No packages were added. Add them later with: {command}", &[("command", &shown)]));
            return;
        }
        let essential: Vec<String> = self.layer.essential().into_iter().map(str::to_string).collect();
        if let Some(finish) = image.finish_argv() {
            // ABRoot only records packages (checking each is in the
            // repositories) and stops at the first unknown one: the essential
            // ones together, the optional ones one by one, then one build.
            let mut added = essential.is_empty() || image.add_argv(&essential).is_some_and(|argv| self.as_root(Cmd::from_argv(argv)));
            if added {
                // abroot's first use asks you to accept its terms: that call
                // keeps the terminal, the rest are quiet.
                let mut io = if essential.is_empty() { Io::Inherit } else { Io::Quiet };
                for p in self.layer.optional.clone() {
                    let recorded = image.add_argv(&[p.as_str()]).is_some_and(|argv| self.as_root(Cmd::from_argv(argv).io(io)));
                    if recorded {
                        io = Io::Quiet;
                    }
                    if !recorded && !self.dry() {
                        self.say(Level::Detail, format!("skipped {p} (not available)"));
                    }
                }
                added = self.as_root(Cmd::from_argv(finish));
            }
            self.image_packages_done(image, added, &shown);
            return;
        }
        let mut added = self.as_root(Cmd::from_argv(argv));
        if !added && !self.layer.optional.is_empty() && !essential.is_empty() {
            let optional = self.layer.optional.join(" ");
            self.say(Level::Warn, tf("Trying again without the optional packages: {packages}", &[("packages", &optional)]));
            added = image.add_argv(&essential).is_some_and(|argv| self.as_root(Cmd::from_argv(argv)));
        }
        self.image_packages_done(image, added, &shown);
    }

    /// What the transaction's outcome means for the next boot.
    fn image_packages_done(&mut self, image: ImageSystem, added: bool, shown: &str) {
        if !added {
            self.say(Level::Warn, tf("{tool} couldn't add the packages. Add them later with: {command}", &[("tool", &image.program()), ("command", &shown)]));
            return;
        }
        let level = if self.dry() { Level::Info } else { Level::Ok };
        self.say(level, t("Restart your computer to start using the new packages. Everything else is ready now."));
        self.login_after_reboot();
    }

    /// How to switch to Mandara Login once greetd and cage are there.
    fn login_after_reboot(&mut self) {
        if !self.layer.login_after_reboot {
            return;
        }
        let bin = self.paths.binary().to_string_lossy().into_owned();
        let command = self.root_hint(&[bin.as_str(), "greeter", "use", "mandara", "--apply"]);
        self.say(Level::Info, tf("Mandara Login needs greetd and cage, which arrive with the next boot. After restarting, switch to it in Studio → Login screen → Login manager, or run: {command}", &[("command", &command)]));
    }
}

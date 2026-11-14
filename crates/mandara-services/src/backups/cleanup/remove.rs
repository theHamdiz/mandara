//! Claim each selected file atomically before removing it.

use super::*;
use super::paths::{open, child, parent, claim};

fn verify(root: &File, entry: &Entry) -> Result<()> {
    let (directory, name) = parent(root, &entry.relative)?;
    ensure!(Stamp::of(&child(&directory, &name)?)? == entry.stamp, "{}", t("The cleanup selection changed. Preview it again before removing anything."));
    Ok(())
}

fn remove_file(directory: &File, name: &OsStr, quarantine: &File, staged: &str, expected: Stamp) -> Result<()> {
    ensure!(Stamp::of(&child(directory, name)?)? == expected, "{}", t("The cleanup selection changed. Preview it again before removing anything."));
    claim(directory, name, quarantine, OsStr::new(staged))?;
    let selected = child(quarantine, OsStr::new(staged)).and_then(|file| Stamp::of(&file));
    if !selected.is_ok_and(|stamp| expected.after_rename(stamp)) {
        claim(quarantine, OsStr::new(staged), directory, name).context("the changed file is retained in cleanup recovery")?;
        anyhow::bail!("{}", t("The cleanup selection changed. Preview it again before removing anything."));
    }
    unlinkat(quarantine, staged, AtFlags::empty())?;
    Ok(())
}

pub fn remove(preview: &Preview, cancel: &AtomicBool) -> Result<Report> {
    ensure!(cfg!(target_os = "linux"), "{}", t("Reviewed file cleanup is available on Linux."));
    ensure!(!cancel.load(Ordering::Acquire), "{}", t("Cancelled"));
    let root = open(&preview.candidate.root)?;
    ensure!(preview.root.identity(Stamp::of(&root)?), "cleanup root changed");
    for entry in &preview.files {
        ensure!(!cancel.load(Ordering::Acquire), "{}", t("Cancelled"));
        verify(&root, entry)?;
    }
    let information = preview.info.as_ref().map(|(path, stamp, parent_stamp)| -> Result<_> {
        let directory = open(path.parent().context("Trash information has no parent")?)?;
        ensure!(parent_stamp.identity(Stamp::of(&directory)?), "Trash information directory changed");
        let name = path.file_name().context("Trash information has no filename")?.to_os_string();
        ensure!(Stamp::of(&child(&directory, &name)?)? == *stamp, "Trash information changed; preview it again");
        Ok((directory, name, *stamp))
    }).transpose()?;
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|error| anyhow::anyhow!("could not create cleanup staging name: {error}"))?;
    let name = format!(".mandara-cleanup-{:032x}", u128::from_ne_bytes(random));
    mkdirat(&root, name.as_str(), Mode::RWXU)?;
    let quarantine = child(&root, OsStr::new(&name))?;
    let recovery = preview.candidate.root.join(&name);
    let mut report = Report { items_left: preview.left_untouched, ..Default::default() };
    let result = (|| -> Result<()> {
        for (index, entry) in preview.files.iter().enumerate() {
            ensure!(!cancel.load(Ordering::Acquire), "{}", t("Cancelled"));
            let (directory, filename) = parent(&root, &entry.relative)?;
            let staged = index.to_string();
            remove_file(&directory, &filename, &quarantine, &staged, entry.stamp)?;
            report.files_removed += 1;
            report.logical_bytes += entry.stamp.bytes;
        }
        for entry in &preview.directories {
            ensure!(!cancel.load(Ordering::Acquire), "{}", t("Cancelled"));
            let (directory, filename) = parent(&root, &entry.relative)?;
            let selected = child(&directory, &filename)?;
            ensure!(entry.stamp.identity(Stamp::of(&selected)?), "cleanup directory changed");
            match unlinkat(&directory, &filename, AtFlags::REMOVEDIR) {
                Ok(()) => {}
                Err(rustix::io::Errno::NOTEMPTY | rustix::io::Errno::BUSY) => report.items_left += 1,
                Err(error) => return Err(error.into()),
            }
        }
        if let Some((directory, name, stamp)) = &information {
            let absent = child(&root, preview.candidate.relative.as_os_str()).is_err_and(|error| error.downcast_ref::<rustix::io::Errno>() == Some(&rustix::io::Errno::NOENT));
            if absent { remove_file(directory, name, &quarantine, "trashinfo", *stamp)?; }
        }
        Ok(())
    })();
    let empty = unlinkat(&root, name.as_str(), AtFlags::REMOVEDIR);
    result.with_context(|| mandara_i18n::tf("Cleanup stopped after removing {count} files. Inspect {path} if it remains.", &[("count", &report.files_removed as &dyn std::fmt::Display), ("path", &recovery.display())]))?;
    empty.context("cleanup recovery directory is not empty")?;
    Ok(report)
}

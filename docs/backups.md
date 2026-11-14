# Backups and storage

Enable **Backups** in **Studio → Plugins**, then open it from the bar. It brings
encrypted backups, local Btrfs snapshots and reviewed cleanup into one panel.
These are separate tools: an encrypted backup can live on another drive; a
local snapshot shares the original filesystem's storage and failure risks.

## Your encrypted archive

Open the panel's **Settings** and choose:

- **Backup repository:** an absolute path on this computer or a mounted backup
  drive. Mandara supports local restic repositories.
- **Backup password file:** an existing file owned by you with private
  permissions (`0600`). Mandara saves its path, never the password. Keep the
  password somewhere safe separately from the backup drive.
- **Source folders:** the absolute paths you want to back up. The repository
  itself is excluded, and a repository cannot be a source folder.

Use **Initialize repository** once for a new repository, then **Back up now**.
The timeline lists recent snapshots with their source folders, file counts and
sizes when available. **Check repository** asks restic to check the repository's structure and
indexes; it does not read every byte of every stored file. Older Restic versions
report this check as text; Mandara uses the command’s exit result.

**Restore a copy…** asks for a parent folder and creates a fresh private folder
inside it. Your original files stay where they are. Cancelling leaves any
partially restored copies in that folder. The completed restore location stays
available when you switch tabs or run unrelated disk jobs within the same repository. Explicitly started jobs can finish
while the panel is closed; disabling the plugin requests cancellation.

Mandara does not schedule backups, forget old restic snapshots or prune a
repository automatically. Those remain deliberate choices outside this panel.
Snapshot navigation stops at the last page and adjusts when the list changes.

## Local Btrfs snapshots

Choose **Snapshots**, select a Btrfs filesystem and load its snapshot list.
Reading the complete list can require **Load snapshots with administrator
approval**. For creation, set **Local snapshot source** to an existing subvolume
and **Local snapshot folder** to an existing folder on the selected Btrfs mount.
New snapshots are read-only. Nested subvolumes need their own snapshots.

**Remove snapshot…** shows the selected path, subvolume ID and UUID before you
continue. Mounted snapshots, the default subvolume and the filesystem's top
level are protected. The administrator helper rechecks the mount and snapshot
identities before removing anything. A changed mount or snapshot requires a
fresh review. Successful snapshot and maintenance actions refresh the selected
filesystem; a privileged list may need administrator approval again. Removing a snapshot does not guarantee an equal increase in free
space: other snapshots may still reference the same blocks.
Successful administrator reads replace the earlier permission warning. If you
open this tab during another backup job, its first read waits until that job
finishes. Container namespace mounts stay in the safety checks without appearing
as disks or blocking the filesystem list.

## Reviewed cleanup

In **Cleanup**, choose one cache folder or Trash item and press **Preview
files…**. The preview lists the exact files and their logical sizes. By default,
only cache files older than 30 days qualify; change **Cache age** in Settings.
Trash items use a separate preview rather than the cache age.

Close affected applications before **Permanently remove these files**. Removal
is permanent, and applications may need to download their caches again.
Changed selections require a new preview. Files added after the preview, symbolic and hard links,
other mounted filesystems, recovery folders and protected personal-data
locations stay untouched. Trash recovery information stays until its payload
is gone. Sizes describe the selected files, not a promise about reclaimed disk
space on filesystems with shared blocks or snapshots.
Starting another job cancels an unfinished preview and returns to the cleanup
items, ready for a new review.

## Filesystem maintenance

Open **Cleanup → Filesystem maintenance** and select a filesystem. Each action
shows a review before administrator approval:

| Action | What it does |
| --- | --- |
| **Check integrity** | Runs Btrfs scrub over data and metadata. Redundant good copies can repair damaged copies. It does not clean files. |
| **Reclaim empty chunks** | Runs a Btrfs balance filtered to completely empty data and metadata block groups. It does not convert redundancy profiles or run a full balance. |
| **Trim unused blocks** | Tells a supported SSD or thin-provisioned device which blocks are unused. It does not delete files or increase filesystem free space. |

Trim is offered for writable Btrfs, ext4, XFS, F2FS, FAT, exFAT and ntfs3 mounts;
the device and mount must also support discard. Unsupported and read-only
operations explain why they are unavailable. Mandara never formats a disk,
runs an offline filesystem repair or replaces the active system from a snapshot.

## Required tools and commands

Encrypted backups use the distribution’s `restic` package, including 0.16.4.
Each restore uses a new private directory rather than overwriting an earlier copy. On Linux, snapshots and scrub/reclaim use
`btrfs-progs`, trim uses `fstrim`, and administrator approval uses polkit.
`mandara install` includes these as optional packages, with the package names
appropriate to the distribution. On NixOS, add `pkgs.restic`, `pkgs.btrfs-progs`
and `pkgs.util-linux` to your system packages. FreeBSD supports encrypted
backups; the native snapshot, cleanup and maintenance controls require Linux.

`mandara commands` documents `backups list`, `start`, `check`, `init`, `restore`
and `cancel`. The `backups storage` commands inspect current mounts, return a
reviewed native request and require a separate `confirm` before running it.
Commands return the real job result over IPC, including errors and cancellation.

For file cleanup, `backups cleanup locations` lists eligible paths. Pass one to
`backups cleanup preview`, then read `backups cleanup review` and its
`next_offset` pages; every reviewed file has an absolute path.
`backups cleanup confirm` permanently removes only that
retained manifest; `backups cancel` discards it. New previews, leaving the tab, closing the panel
and restarting the shell also discard the review. A late preview cannot bring a
cancelled review back. Each plugin runs one explicit
command at a time, including reads.

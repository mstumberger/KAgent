---
name: slow-boot
description: find which units make boot slow
---
1. `boot_analysis` (all sections) - total time, firmware vs loader vs kernel vs
   userspace.
2. `boot_analysis` section blame - top offenders; ignore masked/one-shot jobs
   like plocate-updatedb and fstrim if they run after login.
3. For each top unit: `systemd` status <unit> and `journal` unit <unit> lines 40.
4. `systemd` list state failed - failed units with DefaultDependency delays hurt.

Good answer: total seconds, the 2-3 units that dominate userspace time, whether
firmware/kernel dominates instead (BIOS or initramfs, not systemd), and which of
them are safe to disable or mask (suggest, never do it).

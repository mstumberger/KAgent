---
name: disk-full
description: find what filled the disk and what is safe to remove
---
1. `disk_info` usage - which mount is full.
2. `shell` with `du -xh --max-depth=1 <mount> 2>/dev/null | sort -rh | head -20`
   (this is a mutating-classified command; it will ask - that is fine, or use
   `list_dir` iteratively on the biggest dirs).
3. Common safe wins to check: ~/.cache, journal size (`journal` then
   `shell` with `journalctl --disk-usage`), old cores (`crashes`), docker
   (`shell` with `docker system df`), snap revisions (`shell` with `snap list --all`).
4. Never delete anything yourself. List candidates with sizes and let the user pick.

Good answer: top space consumers with sizes, marked safe/ask, and the commands
the user could run (journalctl --vacuum-size=500M, docker system prune, snap
remove --revision old).

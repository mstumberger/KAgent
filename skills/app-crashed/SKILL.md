---
name: app-crashed
description: find out why an application crashed
---
1. `crashes` (list, since -7d) - find the pid and exe of the crash.
2. `crashes` with that pid - signal, stack top.
3. `journal` with grep "<exe or pid>" lines 60 around the crash time.
4. If OOM: the stack shows oom-kill; confirm with `kernel_info` log filter
   "Out of memory" and `process_list` sort mem now.
5. GPU apps: `hardware` gpu - Xid errors in `kernel_info` log filter Xid.

Good answer: crash time, signal (segv vs oom vs abort), the top frame if useful,
and the pattern (one-off vs repeating).

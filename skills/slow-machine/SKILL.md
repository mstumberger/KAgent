---
name: slow-machine
description: diagnose why the machine feels slow or laggy
---
Goal: find what is consuming CPU, memory, disk or GPU, then say what and why.

1. `system_info` - load average vs core count, memory available, swap, temperatures.
   Rule of thumb: 1-min load > 2x logical cores, or available memory < 10% of total,
   or swap heavily used = the likely axis.
2. `process_list` (sort cpu, limit 15), then `process_list` (sort mem, limit 15).
3. If CPU is idle but it still feels slow: `disk_info` usage, then `shell` with
   `iostat -x 1 3` if present, else check `dmesg | tail -50` via kernel_info log.
4. GPU load: `hardware` section gpu (nvidia-smi).
5. Thermal throttling: `hardware` section sensors - any temp at or above its high.

Good answer: names the top consumer with numbers ("chrome at 240% CPU"), states
whether it is CPU, memory, disk or thermal, and proposes the next fix (close it,
wait for it, add memory, clean fan). Never kill anything without asking.

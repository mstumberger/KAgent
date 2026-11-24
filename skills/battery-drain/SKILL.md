---
name: battery-drain
description: diagnose fast battery drain on a laptop
---
1. `power` - current draw in W, profile, governors.
2. `process_list` sort cpu limit 10 - sustained CPU users drain fastest.
3. `hardware` section gpu - GPU at high utilization or max clocks.
4. `shell` with `bluetoothctl show` - bluetooth powered on adds idle drain.
5. `kernel_info` section log filter suspend or sleep - failed suspends mean the
   machine never actually slept.

Good answer: current draw, the biggest consumer, profile/governor state, and one
or two concrete changes (switch profile, close app, disable bluetooth).

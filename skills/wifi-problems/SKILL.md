---
name: wifi-problems
description: diagnose wifi drops, weak signal or no connectivity
---
1. `network_info` section addresses - does wlan have an IP? section routes for
   default route.
2. `shell` with `nmcli -t -f IN-USE,SSID,SIGNAL,CHAN dev wifi list` (RO allowlist).
3. `journal` unit NetworkManager lines 80 - look for disconnect reasons
   (deauth, supplicant timeouts, roaming).
4. `kernel_info` section log filter wifi or iwlwifi - firmware crashes re-appear here.
5. Compare 2.4 vs 5 GHz channel congestion if signal is low.

Good answer: signal strength, disconnect history with reasons, and whether the
problem is radio (weak signal), driver (firmware errors) or configuration.

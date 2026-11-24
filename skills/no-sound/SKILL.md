---
name: no-sound
description: diagnose missing or wrong audio output
---
1. `audio` - is there a sink at all? Which one is default (marked `*`)?
2. If no sinks: `systemd` status pipewire.service, wireplumber.service, and
   `kernel_info` section modules filter snd (is the sound card module loaded?).
3. Wrong output: the default sink is not the one expected (HDMI vs analog vs
   Bluetooth). Report available sinks with ids so the user can choose
   (`audio_control set_default` arrives in S9).
4. Volume muted or 0 shows in wpctl status - say so.
5. Per-app silent: check "microphone users" style streams only if recording is the
   issue; playback streams appear under sinks.

Good answer: names the current default sink, whether anything is muted, and which
sink the sound is probably going to.

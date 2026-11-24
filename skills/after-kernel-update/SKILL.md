---
name: after-kernel-update
description: check DKMS drivers (NVIDIA, etc.) after a kernel update
---
1. `drivers` - running kernel, dkms status per kernel, nvidia module version.
2. `kernel_info` section modules filter nvidia - is the module actually loaded?
3. `kernel_info` section taint - O (out-of-tree) or E (unsigned) set is expected
   with DKMS, but a missing module is not.
4. `journal` kernel filter "dkms\|nvidia\|module" lines 100 since the update -
   build failures and firmware load errors land here.
5. If dkms shows installed but the module is missing for the running kernel:
   the header package or secure boot MOK enrollment is the usual cause
   (`drivers` shows Secure Boot state).

Good answer: DKMS state for the running kernel, whether the module is loaded,
and if broken, the likely fix (reinstall headers, update dkms, enroll MOK) as a
suggestion.

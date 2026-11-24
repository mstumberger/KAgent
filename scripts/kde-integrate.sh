#!/usr/bin/env bash
# Kept for old instructions: the integration script now handles every desktop.
exec "$(dirname "$0")/desktop-integrate.sh" "$@"

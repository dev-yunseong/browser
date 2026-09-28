#!/bin/bash
# Capture URL with Playwright Chromium at the parity viewport.
# usage: scripts/parity/chromium.sh OUT.png [URL] [WIDTH,HEIGHT]
set -u
source ~/.nvm/nvm.sh >/dev/null 2>&1 || true
timeout 120s npx --yes playwright screenshot --browser chromium \
  --viewport-size "${3:-800,1200}" --wait-for-timeout 5000 \
  "${2:-https://www.naver.com}" "${1:?output png path}"

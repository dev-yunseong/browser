#!/bin/bash
# Capture a page with this repository's browser-daemon.
#
# usage: scripts/parity/cap.sh OUT.png [SETTLE_SECONDS] [URL]
# env:   REPO      repository root with target/release binaries (default: this repository)
#        PORT      daemon port (default 7071)
#        VIEWPORT  1 = capture the 800x1200 viewport at the page's scroll position
#                  (matches `npx playwright screenshot --viewport-size 800,1200`);
#                  otherwise capture the full page
#        DUMP      path to also save the DOM the last render used
#        CAPTURE_AT seconds after navigation starts to capture (overrides SETTLE)
set -u
REPO=${REPO:-$(cd "$(dirname "$0")/../.." && pwd)}
PORT=${PORT:-7071}
OUT=${1:?output png path}
SETTLE=${2:-5}
URL=${3:-https://www.naver.com}
CLI="$REPO/target/release/browser-cli"
cd "$REPO"
pkill -f "browser-daemon --no-gui --port $PORT( |$)" 2>/dev/null
for _ in $(seq 1 40); do ss -ltn | grep -q ":$PORT " || break; sleep 0.5; done
timeout 900s "$REPO/target/release/browser-daemon" --no-gui --port "$PORT" --viewport-height 1200 \
  > "/tmp/browser-daemon-$PORT.log" 2>&1 &
for _ in $(seq 1 40); do ss -ltn | grep -q ":$PORT " && break; sleep 0.25; done
NAV_START=$(date +%s.%N)
timeout 150s "$CLI" --port "$PORT" navigate "$URL" > "/tmp/browser-nav-$PORT.txt" 2>&1
# Keep the page's timers and frame messages moving while it settles. With
# CAPTURE_AT=<seconds>, settle until that long after navigation started
# instead (to line rolling banners up with a capture taken at that time).
if [ -n "${CAPTURE_AT:-}" ]; then
  while awk -v s="$NAV_START" -v now="$(date +%s.%N)" -v at="$CAPTURE_AT" 'BEGIN { exit !(now - s < at) }'; do
    timeout 60s "$CLI" --port "$PORT" tick 5 > /dev/null 2>&1
    sleep 0.2
  done
else
  for _ in $(seq 1 "$SETTLE"); do
    timeout 60s "$CLI" --port "$PORT" tick 5 > /dev/null 2>&1
    sleep 1
  done
fi
MODE=--full-page
[ "${VIEWPORT:-}" = 1 ] && MODE=--viewport
for _ in $(seq 1 20); do
  timeout 60s "$CLI" --port "$PORT" screenshot "$MODE" "$OUT" > /dev/null 2>&1
  if head -c 4 "$OUT" 2>/dev/null | grep -q PNG; then
    [ -n "${DUMP:-}" ] && timeout 60s "$CLI" --port "$PORT" dom "$DUMP" > /dev/null 2>&1
    git -C "$REPO" checkout -q storage.json 2>/dev/null
    echo "saved $OUT"
    exit 0
  fi
  sleep 3
done
git -C "$REPO" checkout -q storage.json 2>/dev/null
echo "screenshot failed: $(cat "$OUT" 2>/dev/null)"
exit 1

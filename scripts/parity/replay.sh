#!/bin/bash
# Compare browser-daemon with Chromium on identical content.
#
# usage: scripts/parity/replay.sh record|compare OUT_DIR [URL]
#   record   run both clients through a recording proxy once
#   compare  replay the recording to both clients and compare the captures
set -u
MODE=${1:?record|compare}
OUT=${2:?output directory}
URL=${3:-https://www.naver.com}
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
PORT=18080
mkdir -p "$OUT/recording"
CA_BUNDLE="$OUT/ca-bundle.pem"
cat /etc/ssl/certs/ca-certificates.crt ~/.mitmproxy/mitmproxy-ca-cert.pem > "$CA_BUNDLE"

# A proxy left over from an earlier run would keep the port.
for pid in $(pgrep -x mitmdump); do
  tr '\0' ' ' < "/proc/$pid/cmdline" | grep -q -- "-p $PORT " && kill "$pid"
done
for _ in $(seq 1 20); do ss -ltn | grep -q ":$PORT " || break; sleep 0.25; done

proxy_mode=replay
[ "$MODE" = record ] && proxy_mode=record
PARITY_MODE=$proxy_mode PARITY_DIR="$OUT/recording" \
  timeout 900s ~/.local/bin/mitmdump -q -p $PORT -s "$HERE/replay_addon.py" > "$OUT/mitm-$MODE.log" 2>&1 &
PROXY_PID=$!
for _ in $(seq 1 40); do ss -ltn | grep -q ":$PORT " && break; sleep 0.25; done

source ~/.nvm/nvm.sh >/dev/null 2>&1 || true
NB=$(ls -d ~/.nvm/versions/node/*/bin 2>/dev/null | tail -1)
CAPTURE_AT=${CAPTURE_AT:-10}
# Playwright is resolved from the directory holding chromium_at.js.
[ -d "$OUT/node_modules/playwright" ] || (cd "$OUT" && PATH="$NB:$PATH" npm i playwright > /dev/null 2>&1)
cp "$HERE/chromium_at.js" "$OUT/chromium_at.js"
PATH="$NB:$PATH" timeout 180s node "$OUT/chromium_at.js" "$URL" "$OUT/chromium-$MODE.png" "$CAPTURE_AT" "http://127.0.0.1:$PORT" > /dev/null 2>&1

NO_PROXY=127.0.0.1,localhost HTTPS_PROXY="http://127.0.0.1:$PORT" HTTP_PROXY="http://127.0.0.1:$PORT" SSL_CERT_FILE="$CA_BUNDLE" \
  CAPTURE_AT=$CAPTURE_AT VIEWPORT=1 "$HERE/cap.sh" "$OUT/ours-$MODE.png" 0 "$URL"

kill "$PROXY_PID" 2>/dev/null
if [ "$MODE" = compare ]; then
  python3 "$HERE/compare.py" "$OUT/chromium-$MODE.png" "$OUT/ours-$MODE.png" "$OUT/side-$MODE.png"
fi

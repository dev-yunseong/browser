"""mitmproxy addon: record a page's responses once, then replay them to any
client so Chromium and browser-daemon see identical content. Record the
same page with both clients so the recording covers what each requests.

env: PARITY_MODE=record|replay, PARITY_DIR=<directory for recorded responses>

Replay serves the recorded response for the exact URL, else the one for the
same method, host and path (query strings often carry timestamps or random
cache busters), else 404. HTML responses get a seeded Math.random so
client-side shuffles (newsstand order, ad rotation) match too.
"""
import hashlib
import json
import os
import pathlib

from mitmproxy import http

MODE = os.environ.get("PARITY_MODE", "record")
DIR = pathlib.Path(os.environ.get("PARITY_DIR", "parity-recording"))
DIR.mkdir(parents=True, exist_ok=True)

# Math.random seeded per call site (script URL, line and column of the
# caller) with a per-site counter, so a shuffle gets the same numbers in both
# engines even when they call Math.random a different number of times
# elsewhere. Both run V8, so stack frames read the same.
SEED = rb"""<script>(function(){
var counts={};
function h(s){var x=2166136261;for(var i=0;i<s.length;i++){x^=s.charCodeAt(i);x=Math.imul(x,16777619);}return x>>>0;}
function mix(a){a=a+0x6D2B79F5|0;var t=Math.imul(a^a>>>15,1|a);t=t+Math.imul(t^t>>>7,61|t)^t;return((t^t>>>14)>>>0)/4294967296;}
Math.random=function(){
  var line=(new Error().stack||'').split('\n')[2]||'';
  var m=line.match(/(https?:[^\s()]+|<anonymous>[^\s()]*)\)?\s*$/);
  var site=m?m[1]:line;
  var n=counts[site]=(counts[site]||0)+1;
  return mix((h(site)^Math.imul(n,0x9E3779B1))|0);
};})();</script>"""
LOCAL_HOSTS = {"127.0.0.1", "localhost"}
DROP_HEADERS = {"content-encoding", "content-length", "transfer-encoding", "strict-transport-security"}


def _name(parts):
    return hashlib.sha1("\n".join(parts).encode()).hexdigest()


def _keys(req):
    exact = _name([req.method, req.pretty_url])
    loose = _name([req.method, req.host, req.path.split("?", 1)[0]])
    return exact, loose


def _inject(flow):
    ctype = flow.response.headers.get("content-type", "")
    if "text/html" not in ctype:
        return
    body = flow.response.content or b""
    low = body[:4096].lower()
    at = low.find(b"<head")
    if at >= 0:
        at = low.find(b">", at) + 1
    else:
        at = 0
    flow.response.content = body[:at] + SEED + body[at:]


class ParityReplay:
    def request(self, flow: http.HTTPFlow):
        if MODE != "replay" or flow.request.host in LOCAL_HOSTS:
            return
        exact, loose = _keys(flow.request)
        for name in (exact, loose):
            meta = DIR / f"{name}.json"
            if meta.exists():
                info = json.loads(meta.read_text())
                body = (DIR / f"{name}.body").read_bytes()
                flow.response = http.Response.make(info["status"], body, info["headers"])
                return
        flow.response = http.Response.make(404, b"", {"content-type": "text/plain"})

    def response(self, flow: http.HTTPFlow):
        if flow.request.host in LOCAL_HOSTS:
            return
        if MODE == "record" and flow.response is not None:
            headers = [(k, v) for k, v in flow.response.headers.items(multi=True) if k.lower() not in DROP_HEADERS]
            info = {"status": flow.response.status_code, "headers": dict(headers)}
            body = flow.response.content or b""
            for name in _keys(flow.request):
                if name == _keys(flow.request)[1] and (DIR / f"{name}.json").exists():
                    continue  # keep the first response for a path
                (DIR / f"{name}.json").write_text(json.dumps(info))
                (DIR / f"{name}.body").write_bytes(body)
        # Seed Math.random in both modes, so the recording holds exactly the
        # resources the seeded page asks for.
        if flow.response is not None:
            _inject(flow)


addons = [ParityReplay()]

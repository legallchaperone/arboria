"""P0 spike 6: a minimal ACP client (JSON-RPC over stdio) driving claude-agent-acp,
which drives the user's own `claude` (CLAUDE_CODE_EXECUTABLE). Prints what an AI drawer would render.

python3 client.py ADAPTER_JS CWD
"""
import json
import os
import shutil
import subprocess
import sys
import threading
from collections import Counter

adapter, cwd = sys.argv[1], sys.argv[2]
env = {k: v for k, v in os.environ.items() if k != "CLAUDECODE" and not k.startswith("CLAUDE_CODE_")}
env["CLAUDE_CODE_EXECUTABLE"] = shutil.which("claude")
p = subprocess.Popen(["node", adapter], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(os.path.join(cwd, "..", "acp-stderr.txt"), "w"), env=env, text=True, bufsize=1)
pending, nid, lock = {}, [0], threading.Lock()
kinds = Counter()
transcript = []


def send(obj):
    p.stdin.write(json.dumps(obj) + "\n"); p.stdin.flush()


def request(method, params):
    with lock:
        nid[0] += 1; i = nid[0]
        ev = threading.Event(); pending[i] = [ev, None]
    send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
    ev.wait(180)
    return pending.pop(i)[1]


def reader():
    for line in p.stdout:
        msg = json.loads(line)
        if "id" in msg and "method" not in msg:  # response to us
            pending[msg["id"]][1] = msg; pending[msg["id"]][0].set()
        elif msg.get("method") == "session/update":
            u = msg["params"]["update"]; k = u.get("sessionUpdate"); kinds[k] += 1
            if k == "agent_message_chunk":
                transcript.append(u["content"].get("text", ""))
            elif k in ("tool_call", "tool_call_update", "plan", "usage_update"):
                print("  update", k, json.dumps({x: u.get(x) for x in ("title", "kind", "status", "toolCallId", "used", "size", "cost") if x in u}, ensure_ascii=False)[:200])
        elif msg.get("method") == "session/request_permission":
            prm = msg["params"]
            opts = [(o["optionId"], o["kind"]) for o in prm["options"]]
            print("  PERMISSION", prm["toolCall"].get("title"), opts)
            allow = next(o for o, k in opts if k == "allow_once")
            send({"jsonrpc": "2.0", "id": msg["id"], "result": {"outcome": {"outcome": "selected", "optionId": allow}}})
        elif "method" in msg:
            print("  server request/notification", msg["method"])
            if "id" in msg:
                send({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32601, "message": "not supported"}})


threading.Thread(target=reader, daemon=True).start()
init = request("initialize", {"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": False, "writeTextFile": False}, "terminal": False}})
r = init.get("result", init)
print("initialize:", json.dumps({k: r.get(k) for k in ("protocolVersion", "agentInfo", "authMethods")}, ensure_ascii=False)[:400])
print("  agentCapabilities:", json.dumps(r.get("agentCapabilities"))[:300])
s = request("session/new", {"cwd": cwd, "mcpServers": []})
sid = s["result"]["sessionId"]
print("session:", sid, "modes:", [m["id"] for m in (s["result"].get("modes") or {}).get("availableModes", [])])
for text in ["Reply with exactly: pong", "Create a file hello.txt in the current directory containing the word hi. Then reply: done"]:
    transcript.clear()
    res = request("session/prompt", {"sessionId": sid, "prompt": [{"type": "text", "text": text}]})
    print(f"prompt {text[:40]!r} -> stopReason={res.get('result', res).get('stopReason')} usage={json.dumps(res.get('result', {}).get('usage'))} text={''.join(transcript)!r}")
print("update kinds:", dict(kinds))
print("hello.txt exists:", os.path.exists(os.path.join(cwd, "hello.txt")))
p.terminate()

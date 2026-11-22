import json
import subprocess
import urllib.error
import urllib.request

OLLAMA_URL = "http://localhost:11434/api/chat"
MODEL = "qwen2.5:3b"

proc = subprocess.Popen(
    [r".\target\release\linkguard.exe", "mcp"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
    bufsize=1,
)

req_id = 0
def mcp_call(method, params=None, notify=False):
    global req_id
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    if not notify:
        req_id += 1
        msg["id"] = req_id
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()
    return None if notify else json.loads(proc.stdout.readline())

def chat(messages, tools):
    body = json.dumps({
        "model": MODEL,
        "messages": messages,
        "tools": tools,
        "stream": False,
    }).encode()
    req = urllib.request.Request(
        OLLAMA_URL,
        data=body,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req) as resp:
            return json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        print(f"HTTP {e.code}: {e.read().decode()}")
        raise

# 1. MCP handshake + discover tools
mcp_call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "local-bridge", "version": "0.1"}})
mcp_call("notifications/initialized", notify=True)
tools_resp = mcp_call("tools/list")
ollama_tools = [
    {"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["inputSchema"]}}
    for t in tools_resp["result"]["tools"]
]

# 2. Agent loop
with open("./bad_telemetry.py", "r", encoding="utf-8") as f:
    file_src = f.read()

messages = [{
    "role": "user",
    "content": (
        "Here is `./bad_telemetry.py`:\n```python\n"
        f"{file_src}\n```\n\n"
        "1. Call `analyze_code` with `path='./bad_telemetry.py'`, `profile='lora-sf12'`, `hz=10`.\n"
        "2. Call `evaluate_payload` to verify a compact binary payload size (e.g. 20 bytes) and a PASSING Hz rate.\n"
        "3. Output the fixed Python code (use `struct.pack`, `sock.settimeout`, `await asyncio.sleep(...)` at the passing Hz, and no `json.dumps`)."
    ),
}]

for _ in range(6):
    res = chat(messages, ollama_tools)
    msg = res["message"]
    messages.append(msg)

    tool_calls = msg.get("tool_calls") or []
    if not tool_calls:
        print("\n=== FINAL MODEL RESPONSE ===\n")
        print(msg.get("content", ""))
        break

    for tc in tool_calls:
        fn = tc["function"]["name"]
        raw_args = tc["function"].get("arguments", {})
        args = json.loads(raw_args) if isinstance(raw_args, str) else raw_args
        print(f"[tool call] {fn}({args})")
        out = mcp_call("tools/call", {"name": fn, "arguments": args})
        tool_text = out["result"]["content"][0]["text"]
        messages.append({"role": "tool", "content": tool_text})

proc.terminate()
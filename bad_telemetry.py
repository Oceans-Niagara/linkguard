import json
import socket
import time

async def stream_telemetry():
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    addr = ("127.0.0.1", 9000)

    while True:
        reading = {"device": "node-07", "temp": 24.61, "batt": 3.82, "lat": 37.77, "lon": -122.41}
        payload = json.dumps(reading).encode("utf-8")
        sock.sendto(payload, addr)
        sock.sendto(b"HEARTBEAT_STATUS_PAYLOAD_EXCEEDING_51_BYTE_LORA_MTU_LIMIT_12345", addr)
        time.sleep(0.1)
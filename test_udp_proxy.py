import socket
import threading
import time

def server():
    srv = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    srv.bind(("127.0.0.1", 8000))
    count = 0
    while True:
        data, _ = srv.recvfrom(4096)
        count += 1
        print(f"[udp server :8000] packet #{count} ({len(data)} B)")

threading.Thread(target=server, daemon=True).start()
time.sleep(0.3)

cli = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
payload = b'{"device":"node-07","temp":24.61,"batt":3.82,"lat":37.77,"lon":-122.41,"seq":00001}\n'
print(f"[udp client -> :9000] sending 20 x {len(payload)} B datagrams at 10 Hz...")

for i in range(20):
    cli.sendto(payload, ("127.0.0.1", 9001))
    time.sleep(0.1)

time.sleep(3.0)
cli.close()
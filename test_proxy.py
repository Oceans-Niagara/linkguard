import socket
import threading
import time

def server():
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.bind(("127.0.0.1", 8000))
    srv.listen(1)
    conn, _ = srv.accept()
    total = 0
    while True:
        data = conn.recv(4096)
        if not data:
            break
        total += len(data)
        print(f"[server :8000] received {len(data)} B (total {total} B)")

threading.Thread(target=server, daemon=True).start()
time.sleep(0.3)

cli = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
cli.connect(("127.0.0.1", 9000))
payload = b'{"device":"node-07","temp":24.61,"batt":3.82,"lat":37.77,"lon":-122.41,"seq":00001}\n'
print(f"[client -> :9000] sending 20 x {len(payload)} B packets at 10 Hz...")

t0 = time.time()
for i in range(20):
    cli.sendall(payload)
    time.sleep(0.1)

time.sleep(1.5)
cli.close()
print(f"Done in {time.time() - t0:.2f}s")
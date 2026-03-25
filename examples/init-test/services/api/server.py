"""Simulates a Go API service for testing purposes."""
import http.server
import json
import os
import sys
import threading
import time

PORT = int(os.environ.get('PORT', 5002))

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/healthz':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'status': 'ok', 'service': 'api'}).encode())
            return
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({'message': 'API service running'}).encode())

    def log_message(self, format, *args):
        print(f'{args[0]}', flush=True)

def periodic_logger():
    counter = 0
    messages = [
        'processing request queue',
        'cache hit ratio: 94.2%',
        'db query completed in 3ms',
        'auth token validated',
        'rate limit check passed',
    ]
    while True:
        msg = messages[counter % len(messages)]
        print(f'{msg} (tick={counter})', flush=True)
        counter += 1
        time.sleep(2)

if __name__ == '__main__':
    t = threading.Thread(target=periodic_logger, daemon=True)
    t.start()
    server = http.server.HTTPServer(('', PORT), Handler)
    print(f'listening on port {PORT}', flush=True)
    sys.stdout.flush()
    server.serve_forever()

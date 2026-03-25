import http.server
import json
import os

PORT = int(os.environ.get('PORT', 5001))

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/healthz':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({'status': 'ok', 'service': 'auth'}).encode())
            return
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(json.dumps({'message': 'Auth service running'}).encode())

    def log_message(self, format, *args):
        print(f'[auth] {args[0]}')

if __name__ == '__main__':
    server = http.server.HTTPServer(('', PORT), Handler)
    print(f'[auth] listening on port {PORT}')
    server.serve_forever()

const http = require('http');
const PORT = process.env.PORT || 5100;

const server = http.createServer((req, res) => {
  if (req.url === '/healthz') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ status: 'ok', service: 'portal' }));
    return;
  }
  res.writeHead(200, { 'Content-Type': 'text/html' });
  res.end('<html><body><h1>Portal</h1></body></html>');
});

server.listen(PORT, () => {
  console.log(`[portal] listening on port ${PORT}`);
});

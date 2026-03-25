const http = require('http');
const PORT = process.env.PORT || 5010;

const server = http.createServer((req, res) => {
  if (req.url === '/healthz') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end(JSON.stringify({ status: 'ok', service: 'gateway' }));
    return;
  }
  res.writeHead(200, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({ message: 'Gateway service running' }));
});

server.listen(PORT, () => {
  console.log(`[gateway] listening on port ${PORT}`);
});

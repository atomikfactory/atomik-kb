// Local IPv4-only forward proxy for development machines whose DNS returns IPv6
// addresses but that have no IPv6 connectivity (dual-stack connects fail with
// WSAEACCES / os error 10013 on Windows). rustup and cargo honour HTTPS_PROXY /
// CARGO_HTTP_PROXY, so pointing them at http://127.0.0.1:3128 makes downloads work
// without touching system settings. Loopback only; no authentication; no logging of
// request contents.
//
// Usage: node scripts/dev-ipv4-proxy.mjs [port]
import net from "node:net";
import http from "node:http";
import dns from "node:dns";

const PORT = Number(process.argv[2] ?? 3128);

function lookup4(host, cb) {
  dns.lookup(host, { family: 4 }, cb);
}

const server = http.createServer((req, res) => {
  // Plain HTTP (used for http:// URLs only).
  let url;
  try {
    url = new URL(req.url);
  } catch {
    res.writeHead(400);
    res.end();
    return;
  }
  lookup4(url.hostname, (err, address) => {
    if (err) {
      res.writeHead(502);
      res.end();
      return;
    }
    const upstream = http.request(
      {
        host: address,
        port: url.port || 80,
        method: req.method,
        path: url.pathname + url.search,
        headers: { ...req.headers, host: url.host },
      },
      (upstreamRes) => {
        res.writeHead(upstreamRes.statusCode ?? 502, upstreamRes.headers);
        upstreamRes.pipe(res);
      },
    );
    upstream.on("error", () => {
      if (!res.headersSent) res.writeHead(502);
      res.end();
    });
    req.pipe(upstream);
  });
});

server.on("connect", (req, clientSocket, head) => {
  // HTTPS tunnelling: CONNECT host:port
  const idx = req.url.lastIndexOf(":");
  const host = idx > 0 ? req.url.slice(0, idx) : req.url;
  const port = idx > 0 ? Number(req.url.slice(idx + 1)) : 443;
  lookup4(host, (err, address) => {
    if (err) {
      clientSocket.end("HTTP/1.1 502 Bad Gateway\r\n\r\n");
      return;
    }
    const upstream = net.connect(port, address, () => {
      clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n");
      if (head.length) upstream.write(head);
      upstream.pipe(clientSocket);
      clientSocket.pipe(upstream);
    });
    upstream.on("error", () => clientSocket.destroy());
    clientSocket.on("error", () => upstream.destroy());
  });
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`ipv4-only proxy listening on http://127.0.0.1:${PORT}`);
});

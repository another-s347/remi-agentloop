#!/usr/bin/env python3
"""
CORS reverse-proxy + static file server for the browser WASM demo.

Static files are served from the current directory.
POST requests to /api/* are forwarded to the target LLM API with
streaming support and CORS headers.

Usage:
    cd examples/composable-calculator-browser
    python3 serve.py              # default: proxy to https://api.moonshot.cn/v1
    python3 serve.py --port 7778 --target https://api.openai.com/v1
"""

import argparse
import http.client
import http.server
import ssl
import sys
from urllib.parse import urlparse


class CORSProxyHandler(http.server.SimpleHTTPRequestHandler):
    """Serves static files + proxies /api/* to the target LLM API."""

    target_base = "https://api.moonshot.cn/v1"

    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "POST, GET, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.send_header("Access-Control-Expose-Headers", "*")
        super().end_headers()

    def do_OPTIONS(self):
        self.send_response(200)
        self.end_headers()

    def do_POST(self):
        if self.path.startswith("/api/"):
            self._proxy()
        else:
            self.send_error(404)

    def _proxy(self):
        # Build target URL
        api_path = self.path[len("/api/"):]
        parsed = urlparse(self.target_base)
        target_path = f"{parsed.path.rstrip('/')}/{api_path}"

        # Read request body
        content_length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_length) if content_length > 0 else b""

        # Forward relevant headers
        fwd_headers = {}
        for key in ("Content-Type", "Authorization", "Accept"):
            val = self.headers.get(key)
            if val:
                fwd_headers[key] = val

        # Connect to target
        ctx = ssl.create_default_context()
        if parsed.scheme == "https":
            conn = http.client.HTTPSConnection(parsed.hostname, parsed.port or 443, context=ctx)
        else:
            conn = http.client.HTTPConnection(parsed.hostname, parsed.port or 80)

        try:
            conn.request("POST", target_path, body=body, headers=fwd_headers)
            resp = conn.getresponse()

            # Send status + headers
            self.send_response(resp.status)
            for key, value in resp.getheaders():
                low = key.lower()
                # Skip hop-by-hop and CORS headers (we add our own)
                if low in ("transfer-encoding", "connection", "access-control-allow-origin"):
                    continue
                self.send_header(key, value)
            self.end_headers()

            # Stream body chunks
            while True:
                chunk = resp.read(4096)
                if not chunk:
                    break
                self.wfile.write(chunk)
                self.wfile.flush()
        except Exception as e:
            self.send_error(502, f"Proxy error: {e}")
        finally:
            conn.close()

    def log_message(self, format, *args):
        try:
            first = str(args[0]) if args else ""
            parts = first.split()
            method = parts[0] if parts else ""
            path = parts[1] if len(parts) > 1 else ""
        except Exception:
            super().log_message(format, *args)
            return
        if path.startswith("/api/"):
            sys.stderr.write(f"\033[36m[proxy]\033[0m {method} {path} -> {self.target_base}\n")
        elif method in ("GET", "HEAD"):
            pass  # suppress static file logs
        else:
            super().log_message(format, *args)


def main():
    parser = argparse.ArgumentParser(description="CORS proxy + static server for browser WASM demo")
    parser.add_argument("--port", type=int, default=7778)
    parser.add_argument("--target", default="https://api.moonshot.cn/v1",
                        help="Target LLM API base URL")
    args = parser.parse_args()

    CORSProxyHandler.target_base = args.target

    server = http.server.HTTPServer(("0.0.0.0", args.port), CORSProxyHandler)
    print(f"Serving on http://0.0.0.0:{args.port}")
    print(f"Proxying /api/* → {args.target}")
    print(f"Open http://localhost:{args.port}/index.html")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped.")


if __name__ == "__main__":
    main()

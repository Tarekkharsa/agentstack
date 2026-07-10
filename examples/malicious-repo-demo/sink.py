#!/usr/bin/env python3
"""Localhost exfiltration sink for the demo.

Appends every request body it receives to SINK_LOG. If AgentStack does its job,
that file stays empty on the protected paths — which is exactly what run-demo.sh
asserts.
"""
import os
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = os.environ.get("SINK_LOG", "sink.log")
PORT = int(os.environ.get("SINK_PORT", "8799"))


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        with open(LOG, "ab") as f:
            f.write(body + b"\n")
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *args):
        pass  # keep the demo output clean


HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()

#!/usr/bin/env python3
"""Task-local HTTP body-byte measurement proxy for an isolated Caos stack."""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http.client
import json
import argparse
from urllib.parse import urlsplit
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--listen", default="127.0.0.1")
parser.add_argument("--port", type=int, default=9097)
parser.add_argument("--upstream", default="http://127.0.0.1:9090")
args = parser.parse_args()
upstream_url = urlsplit(args.upstream)
assert upstream_url.scheme == "http" and upstream_url.hostname and not upstream_url.path.strip("/"), "upstream must be an HTTP origin"
lock = threading.Lock()
class Proxy(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *_):
        pass
    def handle_request(self):
        start = time.monotonic_ns()
        if self.headers.get("Transfer-Encoding"):
            self.send_error(501, "chunked requests are outside this measurement")
            return
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        upstream = http.client.HTTPConnection(upstream_url.hostname, upstream_url.port or 80, timeout=900)
        headers = {key:value for key,value in self.headers.items()
                   if key.lower() not in {"host","connection","accept-encoding"}}
        headers["Connection"] = "close"
        upstream.request(self.command, self.path, body=body, headers=headers)
        response = upstream.getresponse()
        data = response.read()
        self.send_response(response.status)
        for key,value in response.getheaders():
            if key.lower() not in {"connection","transfer-encoding","content-length","server","date"}:
                self.send_header(key,value)
        self.send_header("Content-Length", response.getheader("Content-Length", "0") if self.command == "HEAD" else str(len(data)))
        self.send_header("Connection","close")
        self.end_headers()
        self.wfile.write(data)
        self.close_connection = True
        with lock:
            print(json.dumps({"method":self.command,"path":self.path,"status":response.status,
                "request_bytes":len(body),"response_bytes":len(data),
                "response_sha256":hashlib.sha256(data).hexdigest(),
                "elapsed_ns":time.monotonic_ns()-start,"at_ns":time.time_ns()}), flush=True)
        upstream.close()
    do_HEAD = handle_request
    do_GET = handle_request
    do_POST = handle_request
    do_PUT = handle_request
ThreadingHTTPServer((args.listen,args.port),Proxy).serve_forever()

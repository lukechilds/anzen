#!/usr/bin/env python3
"""Small, dependency-free interactive viewer for a Flex Speculos instance."""

from __future__ import annotations

import argparse
import json
import threading
import webbrowser
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlparse
from urllib.request import Request, urlopen


SCREEN_WIDTH = 480
SCREEN_HEIGHT = 600
HTML = Path(__file__).with_name("flex-viewer.html").read_bytes()


class ViewerServer(ThreadingHTTPServer):
    speculos_url: str


class ViewerHandler(BaseHTTPRequestHandler):
    server: ViewerServer

    def do_GET(self) -> None:
        path = urlparse(self.path).path
        if path == "/":
            self._respond(HTTPStatus.OK, "text/html; charset=utf-8", HTML)
        elif path == "/api/frame":
            self._proxy("GET", "/screenshot")
        elif path == "/api/events":
            self._proxy("GET", "/events?currentscreenonly=true")
        else:
            self._json_error(HTTPStatus.NOT_FOUND, "not found")

    def do_POST(self) -> None:
        try:
            body = self._read_json()
            if self.path == "/api/finger":
                self._validate_finger(body)
                self._proxy("POST", "/finger", body)
            elif self.path == "/api/side-button":
                self._validate_button(body)
                self._proxy("POST", "/button/right", body)
            else:
                self._json_error(HTTPStatus.NOT_FOUND, "not found")
        except (ValueError, json.JSONDecodeError) as error:
            self._json_error(HTTPStatus.BAD_REQUEST, str(error))

    def _read_json(self) -> dict[str, object]:
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError as error:
            raise ValueError("invalid content length") from error
        if length <= 0 or length > 16_384:
            raise ValueError("invalid request size")
        value = json.loads(self.rfile.read(length))
        if not isinstance(value, dict):
            raise ValueError("request body must be a JSON object")
        return value

    @staticmethod
    def _validate_finger(body: dict[str, object]) -> None:
        if body.get("action") != "press-and-release":
            raise ValueError("unsupported finger action")
        for name, upper_bound in (("x", SCREEN_WIDTH), ("y", SCREEN_HEIGHT)):
            value = body.get(name)
            if not isinstance(value, int) or isinstance(value, bool):
                raise ValueError(f"{name} must be an integer")
            if not 0 <= value < upper_bound:
                raise ValueError(f"{name} is outside the Flex screen")
        for name, upper_bound in (("x2", SCREEN_WIDTH), ("y2", SCREEN_HEIGHT)):
            value = body.get(name)
            if value is None:
                continue
            if not isinstance(value, int) or isinstance(value, bool):
                raise ValueError(f"{name} must be an integer")
            if not 0 <= value < upper_bound:
                raise ValueError(f"{name} is outside the Flex screen")

    @staticmethod
    def _validate_button(body: dict[str, object]) -> None:
        if body.get("action") not in {"press", "release", "press-and-release"}:
            raise ValueError("unsupported side-button action")

    def _proxy(
        self, method: str, path: str, body: dict[str, object] | None = None
    ) -> None:
        data = None if body is None else json.dumps(body).encode()
        request = Request(
            f"{self.server.speculos_url}{path}",
            data=data,
            method=method,
            headers={"Content-Type": "application/json"} if data else {},
        )
        try:
            with urlopen(request, timeout=3) as response:
                self._respond(
                    response.status,
                    response.headers.get_content_type(),
                    response.read(),
                )
        except HTTPError as error:
            self._respond(
                error.code,
                error.headers.get_content_type(),
                error.read(),
            )
        except URLError as error:
            self._json_error(
                HTTPStatus.BAD_GATEWAY,
                f"cannot reach Speculos at {self.server.speculos_url}: {error.reason}",
            )

    def _json_error(self, status: HTTPStatus, message: str) -> None:
        self._respond(
            status,
            "application/json",
            json.dumps({"error": message}).encode(),
        )

    def _respond(self, status: int, content_type: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        if args and str(args[1]).startswith(("4", "5")):
            super().log_message(format, *args)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", default=5002, type=int)
    parser.add_argument("--speculos", default="http://127.0.0.1:5001")
    parser.add_argument("--open", action="store_true", dest="open_browser")
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    server = ViewerServer((args.host, args.port), ViewerHandler)
    server.speculos_url = args.speculos.rstrip("/")
    viewer_url = f"http://{args.host}:{args.port}"
    print(f"Anzen Flex viewer: {viewer_url}")
    print(f"Proxying Speculos: {server.speculos_url}")
    print("Press Ctrl-C to stop.")
    if args.open_browser:
        threading.Timer(0.2, webbrowser.open, args=(viewer_url,)).start()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()

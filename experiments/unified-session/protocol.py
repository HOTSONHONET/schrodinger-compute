"""Shared wire limits and HTTP client for a trusted, single-owner lab."""
import json
import re
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

CHUNK = 1024 * 1024
ID = re.compile(r"[0-9a-f]{32}\Z")
NODE = re.compile(r"[a-zA-Z0-9_-]{1,64}\Z")


def check_token(token):
    if not re.fullmatch(r"[!-~]{32,256}", token):
        raise ValueError("SC_TOKEN must be 32–256 printable ASCII characters without spaces")


def origin(value):
    url = urlsplit(value)
    if (url.scheme not in ("http", "https") or not url.hostname or url.username is not None
            or url.query or url.fragment or url.path not in ("", "/")):
        raise ValueError("worker address must be an HTTP(S) origin without credentials")
    _ = url.port  # Validate the optional port.
    return value.rstrip("/")


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def request(url, token, method, path, *, body=None, node=None):
    headers = {"Authorization": f"Bearer {token}"}
    if node is not None:
        headers["X-SC-Node"] = node
    req = Request(origin(url) + path, data=body, headers=headers, method=method)
    opener = build_opener(ProxyHandler({}), NoRedirect())
    try:
        with opener.open(req, timeout=15) as response:
            data = response.read(CHUNK + 1)
    except HTTPError as error:
        with error:
            detail = error.read(4096).decode(errors="replace")
        raise RuntimeError(f"{url} {method} {path}: HTTP {error.code}: {detail}") from error
    if len(data) > CHUNK:
        raise RuntimeError("worker response exceeds shard size")
    return data


def document(data):
    value = json.loads(data)
    if not isinstance(value, dict):
        raise ValueError("expected a JSON object")
    return value

"""Request authentication with no credential logging."""

from __future__ import annotations

import hmac
from base64 import b64decode
from binascii import Error as Base64Error
from collections.abc import Awaitable, Callable

from aiohttp import web

from .errors import ConfigurationError


class ApiAuthenticator:
    """Validate Bearer or HA-compatible Basic credentials in constant time."""

    def __init__(self, token: str) -> None:
        if len(token) < 32 or token.lower() in {"change-me", "changeme", "password"}:
            raise ConfigurationError("bridge API token is missing, short, or a default")
        self._token = token

    def accepts(self, authorization: str | None) -> bool:
        if not authorization:
            return False
        if authorization.startswith("Bearer "):
            return hmac.compare_digest(authorization[7:], self._token)
        if not authorization.startswith("Basic "):
            return False
        try:
            decoded = b64decode(authorization[6:], validate=True).decode("utf-8")
        except (Base64Error, UnicodeDecodeError):
            return False
        username, separator, password = decoded.partition(":")
        return (
            bool(separator)
            and hmac.compare_digest(username, "homeassistant")
            and hmac.compare_digest(password, self._token)
        )


AUTHENTICATOR = web.AppKey("authenticator", ApiAuthenticator)


@web.middleware
async def authentication_middleware(
    request: web.Request, handler: Callable[[web.Request], Awaitable[web.StreamResponse]]
) -> web.StreamResponse:
    """Protect everything except the deliberately minimal health endpoint."""
    if request.path == "/healthz":
        return await handler(request)
    authenticator = request.app[AUTHENTICATOR]
    if not authenticator.accepts(request.headers.get("Authorization")):
        raise web.HTTPUnauthorized(
            text='{"error":"unauthorized"}', content_type="application/json"
        )
    return await handler(request)

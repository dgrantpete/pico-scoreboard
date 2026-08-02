# MicroPython aiohttp library
# MIT license; Copyright (c) 2023 Carlos Gil
#
# Vendored from micropython-lib python-ecosys/aiohttp with modifications:
# - Removed WebSocket support (aiohttp_ws dependency)
# - Added ClientResponse.readinto() for zero-copy reads into pre-allocated buffers
# - Added TCP/TLS connection reuse with stale connection detection
# - Bodiless responses (204/304, HEAD) are marked body-consumed so connection
#   reuse survives them (otherwise every 304 poll tore down the TLS session)

import asyncio
import json as _json

HttpVersion10 = "HTTP/1.0"
HttpVersion11 = "HTTP/1.1"


class ClientResponse:
    def __init__(self, reader):
        self.content = reader
        self._body_consumed = False

    def _get_header(self, keyname, default):
        for k in self.headers:
            if k.lower() == keyname:
                return self.headers[k]
        return default

    def _decode(self, data):
        c_encoding = self._get_header("content-encoding", None)
        if c_encoding in ("gzip", "deflate", "gzip,deflate"):
            try:
                import deflate
                import io

                if c_encoding == "deflate":
                    with deflate.DeflateIO(io.BytesIO(data), deflate.ZLIB) as d:
                        return d.read()
                elif c_encoding == "gzip":
                    with deflate.DeflateIO(io.BytesIO(data), deflate.GZIP, 15) as d:
                        return d.read()
            except ImportError:
                print("WARNING: deflate module required")
        return data

    async def read(self, sz=-1):
        data = self._decode(
            await (self.content.read(sz) if sz == -1 else self.content.readexactly(sz))
        )
        self._body_consumed = True
        return data

    async def text(self, encoding="utf-8"):
        data = (await self.read(int(self._get_header("content-length", -1)))).decode(encoding)
        self._body_consumed = True
        return data

    async def json(self):
        data = _json.loads(await self.read(int(self._get_header("content-length", -1))))
        self._body_consumed = True
        return data

    async def readinto(self, buf):
        """Read response body into pre-allocated buffer/memoryview. Returns slice of data read."""
        content_len = int(self._get_header("content-length", 0))
        if content_len == 0:
            raise ValueError("Missing or zero Content-Length")
        if content_len > len(buf):
            raise ValueError(f"Response too large: {content_len} > {len(buf)}")
        dest = buf[:content_len]
        bytes_read = 0
        while bytes_read < content_len:
            n = await self.content.readinto(dest[bytes_read:])
            if n is None or n == 0:
                raise OSError(f"Connection closed after {bytes_read}/{content_len} bytes")
            bytes_read += n
        self._body_consumed = True
        return dest

    def __repr__(self):
        return "<ClientResponse %d %s>" % (self.status, self.headers)


class ChunkedClientResponse(ClientResponse):
    def __init__(self, reader):
        self.content = reader
        self._body_consumed = False
        self.chunk_size = 0

    async def read(self, sz=4 * 1024 * 1024):
        if self.chunk_size == 0:
            l = await self.content.readline()
            l = l.split(b";", 1)[0]
            self.chunk_size = int(l, 16)
            if self.chunk_size == 0:
                # End of message
                sep = await self.content.readexactly(2)
                assert sep == b"\r\n"
                self._body_consumed = True
                return b""
        data = await self.content.readexactly(min(sz, self.chunk_size))
        self.chunk_size -= len(data)
        if self.chunk_size == 0:
            sep = await self.content.readexactly(2)
            assert sep == b"\r\n"
            self._body_consumed = True
        return self._decode(data)

    def __repr__(self):
        return "<ChunkedClientResponse %d %s>" % (self.status, self.headers)


class _RequestContextManager:
    def __init__(self, client, request_co):
        self.reqco = request_co
        self.client = client
        self._resp = None

    async def __aenter__(self):
        self._resp = await self.reqco
        return self._resp

    async def __aexit__(self, *args):
        # Close connection if body wasn't fully consumed or server wants close
        if (not getattr(self._resp, '_body_consumed', False)
                or self.client._should_close):
            await self.client._close_connection()
        return await asyncio.sleep(0)


class ClientSession:
    def __init__(self, base_url="", headers={}, version=HttpVersion11):
        self._reader = None
        self._writer = None
        self._conn_host = None
        self._conn_port = None
        self._should_close = False
        self._base_url = base_url
        self._base_headers = {"Connection": "keep-alive", "User-Agent": "compat"}
        self._base_headers.update(**headers)
        self._http_version = version

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        await self._close_connection()

    async def _close_connection(self):
        if self._writer is not None:
            try:
                self._writer.close()
                await self._writer.wait_closed()
            except Exception:
                pass
            self._writer = None
            self._reader = None
            self._conn_host = None
            self._conn_port = None
        self._should_close = False

    async def close(self):
        await self._close_connection()

    async def _request(self, method, url, data=None, json=None, ssl=None, params=None, headers={}):
        for attempt in range(2):
            redir_cnt = 0
            redir_url = url
            while redir_cnt < 2:
                try:
                    reader = await self.request_raw(method, redir_url, data, json, ssl, params, headers)
                    _headers = []
                    sline = await reader.readline()
                except OSError:
                    # Connection error (ECONNRESET, etc.) — treat as stale
                    sline = b""

                # Stale connection: readline returns b'' on dead socket,
                # or OSError was caught above
                if not sline:
                    if attempt == 0:
                        await self._close_connection()
                        break  # break redirect loop to retry
                    raise OSError("Connection closed by server")

                sline = sline.split(None, 2)
                status = int(sline[1])
                chunked = False
                self._should_close = False
                while True:
                    line = await reader.readline()
                    if not line or line == b"\r\n":
                        break
                    _headers.append(line)
                    if line.startswith(b"Transfer-Encoding:"):
                        if b"chunked" in line:
                            chunked = True
                    elif line.startswith(b"Location:"):
                        redir_url = line.rstrip().split(None, 1)[1].decode()
                    elif line.lower().startswith(b"connection:"):
                        if b"close" in line.lower():
                            self._should_close = True

                if 301 <= status <= 303:
                    redir_cnt += 1
                    await self._close_connection()
                    continue

                # Valid response — build and return it
                if chunked:
                    resp = ChunkedClientResponse(reader)
                else:
                    resp = ClientResponse(reader)
                resp.status = status
                resp.headers = _headers
                resp.url = redir_url
                # RFC 7230 §3.3: 204/304 responses and responses to HEAD carry
                # no body. Mark them consumed so the context-manager exit keeps
                # the connection alive for reuse instead of closing it.
                if status == 204 or status == 304 or method == "HEAD":
                    resp._body_consumed = True
                if params:
                    resp.url += "?" + "&".join(f"{k}={params[k]}" for k in sorted(params))
                try:
                    resp.headers = {
                        val.split(":", 1)[0]: val.split(":", 1)[-1].strip()
                        for val in [hed.decode().strip() for hed in _headers]
                    }
                except Exception:
                    pass
                return resp
            else:
                # Redirect loop exhausted without a valid response
                continue  # try next attempt

        # Should not reach here, but if both attempts fail
        raise OSError("Connection closed by server")

    async def request_raw(
        self,
        method,
        url,
        data=None,
        json=None,
        ssl=None,
        params=None,
        headers={},
    ):
        if json and isinstance(json, dict):
            data = _json.dumps(json)
        if data is not None and method == "GET":
            method = "POST"
        if params:
            url += "?" + "&".join(f"{k}={params[k]}" for k in sorted(params))
        try:
            proto, dummy, host, path = url.split("/", 3)
        except ValueError:
            proto, dummy, host = url.split("/", 2)
            path = ""

        if proto == "http:":
            port = 80
        elif proto == "https:":
            port = 443
            if ssl is None:
                ssl = True
        else:
            raise ValueError("Unsupported protocol: " + proto)

        if ":" in host:
            host, port = host.split(":", 1)
            port = int(port)

        # Reuse existing connection if same host:port and not marked for close
        if (self._writer is not None
                and self._conn_host == host
                and self._conn_port == port
                and not self._should_close):
            reader, writer = self._reader, self._writer
        else:
            await self._close_connection()
            reader, writer = await asyncio.open_connection(host, port, ssl=ssl)
            self._writer = writer
            self._conn_host = host
            self._conn_port = port

        if "Host" not in headers:
            headers.update(Host=host)
        if not data:
            query = b"%s /%s %s\r\n%s\r\n" % (
                method,
                path,
                self._http_version,
                "\r\n".join(f"{k}: {v}" for k, v in headers.items()) + "\r\n" if headers else "",
            )
        else:
            if json:
                headers.update(**{"Content-Type": "application/json"})
            if isinstance(data, bytes):
                headers.update(**{"Content-Type": "application/octet-stream"})
            else:
                data = data.encode()

            headers.update(**{"Content-Length": len(data)})
            query = b"""%s /%s %s\r\n%s\r\n%s""" % (
                method,
                path,
                self._http_version,
                "\r\n".join(f"{k}: {v}" for k, v in headers.items()) + "\r\n",
                data,
            )
        await writer.awrite(query)
        self._reader = reader
        return reader

    def request(self, method, url, data=None, json=None, ssl=None, params=None, headers={}):
        return _RequestContextManager(
            self,
            self._request(
                method,
                self._base_url + url,
                data=data,
                json=json,
                ssl=ssl,
                params=params,
                headers=dict(**self._base_headers, **headers),
            ),
        )

    def get(self, url, **kwargs):
        return self.request("GET", url, **kwargs)

    def post(self, url, **kwargs):
        return self.request("POST", url, **kwargs)

    def put(self, url, **kwargs):
        return self.request("PUT", url, **kwargs)

    def patch(self, url, **kwargs):
        return self.request("PATCH", url, **kwargs)

    def delete(self, url, **kwargs):
        return self.request("DELETE", url, **kwargs)

    def head(self, url, **kwargs):
        return self.request("HEAD", url, **kwargs)

    def options(self, url, **kwargs):
        return self.request("OPTIONS", url, **kwargs)

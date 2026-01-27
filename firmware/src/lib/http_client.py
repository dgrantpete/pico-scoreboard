"""
Persistent HTTP client with connection reuse.

Designed for MicroPython on memory-constrained devices like the Pico W.
Keeps a single TCP/SSL connection open to reduce socket exhaustion and
SSL handshake overhead that causes ENOMEM errors with urequests.
"""

# MicroPython module names vary by version
try:
    import usocket as socket
except ImportError:
    import socket

try:
    import ussl as ssl
except ImportError:
    import ssl


def parse_url(url):
    """
    Parse URL into components.

    Args:
        url: Full URL (e.g., "https://example.com:8080/path")

    Returns:
        Tuple of (use_ssl, host, port, path)
    """
    # Determine protocol
    if url.startswith("https://"):
        use_ssl = True
        url = url[8:]
        default_port = 443
    elif url.startswith("http://"):
        use_ssl = False
        url = url[7:]
        default_port = 80
    else:
        raise ValueError(f"Unsupported URL scheme: {url}")

    # Split host from path
    slash_idx = url.find("/")
    if slash_idx >= 0:
        host_port = url[:slash_idx]
        path = url[slash_idx:]
    else:
        host_port = url
        path = "/"

    # Split host from port
    colon_idx = host_port.find(":")
    if colon_idx >= 0:
        host = host_port[:colon_idx]
        port = int(host_port[colon_idx + 1:])
    else:
        host = host_port
        port = default_port

    return (use_ssl, host, port, path)


class PersistentHttp:
    """
    HTTP client with persistent connection reuse.

    Keeps a single TCP/SSL connection open to reduce socket exhaustion
    and SSL handshake overhead on memory-constrained devices.

    Example:
        http = PersistentHttp("example.com", 443, use_ssl=True)
        status, body = http.get("/api/data", {"Authorization": "Bearer xyz"})
        http.close()
    """

    def __init__(self, host, port=443, use_ssl=True, timeout=10):
        """
        Initialize persistent HTTP client.

        Args:
            host: Server hostname (e.g., "api.example.com")
            port: Server port (default 443 for HTTPS)
            use_ssl: Whether to use TLS (default True)
            timeout: Socket timeout in seconds (default 10)
        """
        self._host = host
        self._port = port
        self._use_ssl = use_ssl
        self._timeout = timeout
        self._sock = None
        self._request_count = 0  # Requests on current connection

    @classmethod
    def from_url(cls, base_url, timeout=10):
        """
        Create client from a base URL.

        Args:
            base_url: Base URL like "https://api.example.com:8080"
            timeout: Socket timeout in seconds

        Returns:
            PersistentHttp instance
        """
        use_ssl, host, port, _ = parse_url(base_url)
        return cls(host, port, use_ssl, timeout)

    def _connect(self):
        """Establish connection to server if not already connected."""
        if self._sock is not None:
            return  # Already connected

        # Resolve address and create socket
        ai = socket.getaddrinfo(self._host, self._port, 0, socket.SOCK_STREAM)[0]
        self._sock = socket.socket(ai[0], socket.SOCK_STREAM, ai[2])
        self._sock.settimeout(self._timeout)

        try:
            self._sock.connect(ai[-1])

            if self._use_ssl:
                self._sock = ssl.wrap_socket(self._sock, server_hostname=self._host)

            self._request_count = 0
            print(f"HTTP: Connected to {self._host}:{self._port}")
        except Exception as e:
            self._disconnect()
            raise

    def _disconnect(self):
        """Close connection."""
        if self._sock is not None:
            try:
                self._sock.close()
            except:
                pass
            self._sock = None

    def _send_request(self, method, path, headers):
        """Send HTTP request line and headers."""
        # Request line
        self._sock.write(f"{method} {path} HTTP/1.1\r\n".encode())

        # Required headers
        self._sock.write(f"Host: {self._host}\r\n".encode())
        self._sock.write(b"Connection: keep-alive\r\n")

        # Custom headers
        for key, value in headers.items():
            self._sock.write(f"{key}: {value}\r\n".encode())

        # End headers
        self._sock.write(b"\r\n")

    def _read_status_line(self):
        """Read and parse HTTP status line."""
        line = self._sock.readline()
        if not line:
            raise OSError("Connection closed by server")

        # Parse "HTTP/1.1 200 OK"
        parts = line.decode().strip().split(None, 2)
        if len(parts) < 2:
            raise ValueError(f"Invalid status line: {line}")

        return int(parts[1])

    def _read_headers(self):
        """Read response headers into dict."""
        headers = {}
        while True:
            line = self._sock.readline()
            if line in (b"\r\n", b"\n", b""):
                break

            decoded = line.decode().strip()
            if ": " in decoded:
                key, value = decoded.split(": ", 1)
                headers[key.lower()] = value

        return headers

    def _read_exactly(self, n):
        """
        Read exactly n bytes from socket.

        Uses pre-allocated bytearray and readinto() to avoid
        repeated allocations and O(n^2) copying from concatenation.
        """
        buf = bytearray(n)
        mv = memoryview(buf)
        pos = 0
        while pos < n:
            nbytes = self._sock.readinto(mv[pos:])
            if not nbytes:
                raise OSError("Connection closed while reading body")
            pos += nbytes
        return bytes(buf)

    def _read_body(self, headers):
        """Read response body based on headers."""
        # Content-Length is most common
        if "content-length" in headers:
            length = int(headers["content-length"])
            return self._read_exactly(length)

        # Chunked transfer encoding
        if headers.get("transfer-encoding", "").lower() == "chunked":
            return self._read_chunked_body()

        # Connection close - read until EOF (rare with keep-alive)
        return self._sock.read()

    def _read_chunked_body(self):
        """Read chunked transfer encoding body."""
        chunks = []
        while True:
            # Read chunk size line
            size_line = self._sock.readline().decode().strip()
            chunk_size = int(size_line, 16)

            if chunk_size == 0:
                # Read trailing CRLF after final chunk
                self._sock.readline()
                break

            # Read exact chunk data
            chunks.append(self._read_exactly(chunk_size))
            # Read trailing CRLF after chunk
            self._sock.readline()

        return b"".join(chunks)

    def request(self, method, path, headers=None):
        """
        Make HTTP request, reusing existing connection.

        Automatically reconnects on connection errors (server timeout,
        connection reset, etc.) and retries once.

        Args:
            method: HTTP method (GET, POST, etc.)
            path: Request path (e.g., "/api/games")
            headers: Optional dict of headers

        Returns:
            Tuple of (status_code, body_bytes)

        Raises:
            OSError: On network errors after retry
        """
        headers = headers or {}

        # Try with existing connection first, retry once on failure
        for attempt in range(2):
            try:
                self._connect()
                self._send_request(method, path, headers)

                status_code = self._read_status_line()
                response_headers = self._read_headers()
                body = self._read_body(response_headers)

                self._request_count += 1

                # Log keep-alive related headers for debugging
                conn_header = response_headers.get("connection", "")
                keep_alive = response_headers.get("keep-alive", "")
                print(f"HTTP: #{self._request_count} {status_code}, Conn: '{conn_header}', KA: '{keep_alive}', {len(body)}B")

                # Check if server wants to close connection
                if conn_header.lower() == "close":
                    print("HTTP: Server requested connection close")
                    self._disconnect()

                return (status_code, body)

            except OSError as e:
                print(f"HTTP: Error after {self._request_count} requests: {e}")
                self._disconnect()
                if attempt == 0:
                    continue  # Retry with fresh connection
                raise

    def get(self, path, headers=None):
        """
        Make GET request.

        Args:
            path: Request path
            headers: Optional headers dict

        Returns:
            Tuple of (status_code, body_bytes)
        """
        return self.request("GET", path, headers)

    def close(self):
        """Close the connection."""
        self._disconnect()

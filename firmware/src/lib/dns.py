"""
Captive Portal DNS Server.

Minimal DNS responder that hijacks all DNS queries and returns the Pico's IP.
This triggers captive portal detection on most devices when they connect to AP mode.
"""

import socket
import uasyncio as asyncio


async def run_dns_server(ip_address='192.168.4.1'):
    """
    Simple DNS server that responds to all queries with the given IP.
    Runs as an async task alongside the web server.

    Args:
        ip_address: The IP to return for all DNS queries (default: 192.168.4.1)
    """
    # Convert IP string to bytes
    ip_bytes = bytes(map(int, ip_address.split('.')))

    # Create UDP socket
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setblocking(False)
    sock.bind(('0.0.0.0', 53))

    print(f"DNS server started, redirecting all queries to {ip_address}")

    while True:
        try:
            # Non-blocking receive
            data, addr = sock.recvfrom(512)

            # Build response
            response = _build_dns_response(data, ip_bytes)
            sock.sendto(response, addr)

        except OSError:
            # No data available, yield to other tasks
            await asyncio.sleep_ms(50)


def _build_dns_response(query, ip_bytes):
    """
    Build a DNS response that returns the given IP for any A record query.

    Args:
        query: The raw DNS query packet
        ip_bytes: The IP address as bytes (4 bytes for IPv4)

    Returns:
        The raw DNS response packet
    """
    # Transaction ID (first 2 bytes of query)
    transaction_id = query[:2]

    # Flags: standard response, no error
    flags = b'\x81\x80'

    # Questions: 1, Answers: 1, Authority: 0, Additional: 0
    counts = b'\x00\x01\x00\x01\x00\x00\x00\x00'

    # Find the question section (starts at byte 12)
    # Copy it for the response
    question_end = 12
    while query[question_end] != 0:
        question_end += query[question_end] + 1
    question_end += 5  # null byte + qtype (2) + qclass (2)

    question = query[12:question_end]

    # Answer section
    # Name pointer to question (0xC00C = pointer to offset 12)
    answer = b'\xc0\x0c'
    # Type A (1), Class IN (1)
    answer += b'\x00\x01\x00\x01'
    # TTL (60 seconds)
    answer += b'\x00\x00\x00\x3c'
    # Data length (4 bytes for IPv4)
    answer += b'\x00\x04'
    # IP address
    answer += ip_bytes

    return transaction_id + flags + counts + question + answer

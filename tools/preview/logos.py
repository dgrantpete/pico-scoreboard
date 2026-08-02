"""Team logo provider: deterministic placeholders, optional real backend fetch.

By default a scenario's logos are synthetic 24x24 RGB565 tiles built from the
team's colors (a solid primary fill inside a 2px alternate-color border) so the
gallery is fully offline and reproducible. Pass `--backend-url`/`--api-key`
(or set `SCOREBOARD_API_KEY`) to fetch the real logos via the same request the
firmware `LogoPool` issues:

    GET {base}/baseball/mlb/teams/{abbr}/logo?width=24&height=24&background_color=000000
    Accept: image/x-rgb565
    X-Api-Key: <key>

Fetched logos are cached under `tools/preview/cache/{abbr}_24x24.rgb565` (raw
RGB565 bytes, exactly the backend body). `--refresh-logos` ignores the cache.
Every logo is returned as a framebuf-shim `FrameBuffer` ready to blit, matching
what the firmware hands the renderer.
"""

import urllib.request
from pathlib import Path

from .shims import framebuf_shim

_LOGO_W = 24
_LOGO_H = 24
_LOGO_BYTES = _LOGO_W * _LOGO_H * 2

CACHE_DIR = Path(__file__).resolve().parent / "cache"


def _rgb565(packed: int) -> int:
    """0x00RRGGBB -> little-endian RGB565 int."""
    r = (packed >> 16) & 0xFF
    g = (packed >> 8) & 0xFF
    b = packed & 0xFF
    return ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3)


def _placeholder_bytes(primary_packed: int, alt_packed: int) -> bytearray:
    """Deterministic 24x24 RGB565: primary fill + 2px alternate border."""
    buf = bytearray(_LOGO_BYTES)
    fb = framebuf_shim.FrameBuffer(buf, _LOGO_W, _LOGO_H, framebuf_shim.RGB565)
    fb.fill(_rgb565(primary_packed))
    border = _rgb565(alt_packed)
    fb.rect(0, 0, _LOGO_W, _LOGO_H, border)
    fb.rect(1, 1, _LOGO_W - 2, _LOGO_H - 2, border)
    return buf


class LogoProvider:
    """Resolves team abbreviations to 24x24 RGB565 framebuffers."""

    def __init__(self, backend_url=None, api_key=None, refresh=False,
                 cache_dir: Path = CACHE_DIR) -> None:
        self._backend_url = backend_url.rstrip("/") if backend_url else None
        self._api_key = api_key
        self._refresh = refresh
        self._cache_dir = cache_dir
        self._mem: dict[str, framebuf_shim.FrameBuffer] = {}

    def _cache_path(self, abbr: str, sport_league: str) -> Path:
        prefix = sport_league.replace("/", "_").lower()
        return self._cache_dir / f"{prefix}_{abbr.lower()}_{_LOGO_W}x{_LOGO_H}.rgb565"

    def _fetch(self, abbr: str, sport_league: str) -> bytearray | None:
        url = (
            f"{self._backend_url}/{sport_league}/teams/{abbr}/logo"
            f"?width={_LOGO_W}&height={_LOGO_H}&background_color=000000"
        )
        req = urllib.request.Request(url, headers={
            "Accept": "image/x-rgb565",
            "X-Api-Key": self._api_key or "",
        })
        try:
            with urllib.request.urlopen(req, timeout=15) as resp:
                if resp.status != 200:
                    print(f"[logos] fetch {abbr}: status {resp.status}, using placeholder")
                    return None
                body = resp.read()
        except Exception as exc:  # noqa: BLE001 - network is best-effort
            print(f"[logos] fetch {abbr} failed ({type(exc).__name__}: {exc}); placeholder")
            return None
        if len(body) < _LOGO_BYTES:
            print(f"[logos] fetch {abbr}: short body {len(body)}B, using placeholder")
            return None
        buf = bytearray(body[:_LOGO_BYTES])
        self._cache_dir.mkdir(parents=True, exist_ok=True)
        self._cache_path(abbr, sport_league).write_bytes(buf)
        return buf

    def _load_bytes(self, abbr: str, primary_packed: int, alt_packed: int,
                    sport_league: str) -> bytearray:
        if self._backend_url:
            cache_path = self._cache_path(abbr, sport_league)
            if not self._refresh and cache_path.is_file():
                data = cache_path.read_bytes()
                if len(data) >= _LOGO_BYTES:
                    return bytearray(data[:_LOGO_BYTES])
            fetched = self._fetch(abbr, sport_league)
            if fetched is not None:
                return fetched
        return _placeholder_bytes(primary_packed, alt_packed)

    def get(self, abbr: str, primary_packed: int, alt_packed: int,
            sport_league: str = "baseball/mlb") -> framebuf_shim.FrameBuffer:
        """Resolve a logo; `sport_league` namespaces the backend path and the
        caches, mirroring the firmware's league-namespaced LogoPool keys."""
        key = f"{sport_league}/{abbr}".lower()
        cached = self._mem.get(key)
        if cached is not None:
            return cached
        buf = self._load_bytes(abbr, primary_packed, alt_packed, sport_league)
        fb = framebuf_shim.FrameBuffer(buf, _LOGO_W, _LOGO_H, framebuf_shim.RGB565)
        self._mem[key] = fb
        return fb

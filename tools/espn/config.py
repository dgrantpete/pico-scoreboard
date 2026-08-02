"""Store connection config: `ESPN_DB_URL` env var, else the gitignored
`tools/espn/.env` (see `.env.example`). The service container always sets the
env var; the .env file is the dev-PC convenience for the analysis CLIs."""

import os
from pathlib import Path

_ENV_FILE = Path(__file__).resolve().parent / ".env"


def database_url() -> str:
    url = os.environ.get("ESPN_DB_URL")
    if url:
        return url
    if _ENV_FILE.exists():
        for line in _ENV_FILE.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line.startswith("ESPN_DB_URL="):
                value = line.split("=", 1)[1].strip()
                if value:
                    return value
    raise SystemExit(
        "no database configured: set ESPN_DB_URL or create tools/espn/.env "
        "from tools/espn/.env.example"
    )

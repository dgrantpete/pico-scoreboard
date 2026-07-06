"""cwd-independent entry point for the tray app (target of the HKCU Run key)."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tools.espn.tray import main

sys.exit(main())

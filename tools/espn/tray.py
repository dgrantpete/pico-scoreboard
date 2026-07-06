"""Windows system-tray shell around the Collector for continuous collection.

Threading: the pystray icon owns the main thread; one persistent worker thread
runs Collector sessions in a loop (a menu change just signals the current
session to stop — the worker rebuilds the next Collector from fresh config);
a UI ticker thread refreshes the tooltip/menu from Collector.snapshot().
The Store is constructed inside the worker thread (sqlite3 check_same_thread)
and the UI only ever reads the in-memory snapshot.
"""

import ctypes
import functools
import json
import os
import sys
import threading
import traceback
import winreg
from datetime import datetime, timedelta
from pathlib import Path

import pystray
from PIL import Image, ImageDraw

from .collect import Collector
from .db import Store
from .leagues import GAME_DAY_TZ, KNOWN_LEAGUES

APP_NAME = "PicoScoreboardEspnTray"
MUTEX_NAME = "pico-scoreboard-espn-tray"
RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
ERROR_ALREADY_EXISTS = 183

# Until 5am ET the previous day is still "the game day": post-midnight live
# games keep their ?dates= stream, and no league starts before ~7:30am ET.
GAME_DAY_ROLLOVER_HOURS = 5

IDLE_INTERVAL_CHOICES = ((30, "30 s"), (60, "60 s"), (120, "2 min"), (300, "5 min"))
DEFAULT_IDLE_INTERVAL = 60
UI_TICK_SECONDS = 5.0
LOG_ROTATE_BYTES = 1_000_000

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_DB = REPO_ROOT / "data" / "espn" / "espn.db"

_COLORS = {
    "running": (60, 200, 90, 255),
    "paused": (240, 200, 40, 255),
    "crashed": (220, 60, 60, 255),
}


def _game_day() -> str:
    now = datetime.now(GAME_DAY_TZ) - timedelta(hours=GAME_DAY_ROLLOVER_HOURS)
    return now.strftime("%Y%m%d")


def _disc(color: tuple[int, int, int, int]) -> Image.Image:
    img = Image.new("RGBA", (32, 32), (0, 0, 0, 0))
    ImageDraw.Draw(img).ellipse((4, 4, 28, 28), fill=color)
    return img


class TrayConfig:
    def __init__(self, path: Path):
        self._path = path
        self.leagues: set[str] = set(KNOWN_LEAGUES)
        self.idle_interval: int = DEFAULT_IDLE_INTERVAL
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
            self.leagues = {k for k in raw["leagues"] if k in KNOWN_LEAGUES}
            self.idle_interval = int(raw["idle_interval"])
        except (OSError, ValueError, KeyError, TypeError):
            pass

    def save(self) -> None:
        doc = {"leagues": sorted(self.leagues), "idle_interval": self.idle_interval}
        tmp = self._path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(doc, indent=2), encoding="utf-8")
        os.replace(tmp, self._path)


class TrayApp:
    def __init__(self, db_path: Path):
        self._db_path = db_path
        self._data_dir = db_path.parent
        self._log_path = self._data_dir / "tray.log"
        self._config = TrayConfig(self._data_dir / "tray_config.json")
        self._quit = threading.Event()
        self._pause = threading.Event()
        self._session_stop = threading.Event()
        self._collector: Collector | None = None
        self._crashed = False
        self._log_lock = threading.Lock()
        self._icons = {state: _disc(color) for state, color in _COLORS.items()}
        self._icon = pystray.Icon(
            "espn-collector", self._icons["running"], "ESPN collector", menu=self._build_menu()
        )
        self._worker = threading.Thread(target=self._worker_loop, name="collector", daemon=True)
        self._ticker = threading.Thread(target=self._ui_ticker, name="ui-ticker", daemon=True)

    # -- lifecycle ---------------------------------------------------------

    def run(self) -> int:
        self._rotate_log()
        self._log(f"=== tray starting {datetime.now().isoformat(timespec='seconds')} ===")
        self._icon.run(setup=self._setup)
        self._quit.set()
        self._session_stop.set()
        self._worker.join(timeout=20)
        self._log(f"=== tray exit {datetime.now().isoformat(timespec='seconds')} ===")
        return 0

    def _setup(self, icon: pystray.Icon) -> None:
        icon.visible = True
        self._worker.start()
        self._ticker.start()
        if pystray.Icon.HAS_NOTIFICATION:
            icon.notify("ESPN collector running — right-click the tray icon to manage", "ESPN collector")

    def _worker_loop(self) -> None:
        store = Store(self._db_path)
        try:
            while not self._quit.is_set():
                # replace the event BEFORE reading config: a toggle that fires
                # in between sets the new event, so its config change is never lost
                self._session_stop = threading.Event()
                keys = sorted(self._config.leagues)
                if not keys:
                    self._quit.wait(1.0)
                    continue
                collector = Collector(
                    store,
                    {key: KNOWN_LEAGUES[key] for key in keys},
                    _game_day,
                    idle_interval=float(self._config.idle_interval),
                    source="tray",
                    stop_event=self._session_stop,
                    pause_event=self._pause,
                    log=self._log,
                )
                self._collector = collector
                self._log(
                    f"session start: {', '.join(keys)} (idle {self._config.idle_interval}s)"
                )
                collector.run()
        except Exception:
            self._crashed = True
            self._log("worker crashed:\n" + traceback.format_exc())
            self._apply_icon_state()
            if pystray.Icon.HAS_NOTIFICATION:
                self._icon.notify("Collector crashed — see data/espn/tray.log", "ESPN collector")
        finally:
            store.close()

    def _ui_ticker(self) -> None:
        last_title = None
        while not self._quit.is_set():
            title = self._tooltip()[:127]
            if title != last_title:
                self._icon.title = title
                self._icon.update_menu()
                last_title = title
            self._quit.wait(UI_TICK_SECONDS)

    # -- menu --------------------------------------------------------------

    def _build_menu(self) -> pystray.Menu:
        # actions go through pystray's arity check (co_argcount counts defaulted
        # params), so bind the loop variable with partial — not default-arg lambdas
        league_items = [
            pystray.MenuItem(
                lambda item, k=key: self._league_text(k),
                functools.partial(self._toggle_league, key),
                checked=lambda item, k=key: k in self._config.leagues,
            )
            for key in KNOWN_LEAGUES
        ]
        interval_items = [
            pystray.MenuItem(
                label,
                functools.partial(self._set_idle_interval, seconds),
                checked=lambda item, s=seconds: self._config.idle_interval == s,
                radio=True,
            )
            for seconds, label in IDLE_INTERVAL_CHOICES
        ]
        return pystray.Menu(
            pystray.MenuItem(lambda item: self._tooltip(), None, enabled=False),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Leagues", pystray.Menu(*league_items)),
            pystray.MenuItem("Idle interval", pystray.Menu(*interval_items)),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem(
                "Pause collection", self._toggle_pause, checked=lambda item: self._pause.is_set()
            ),
            pystray.MenuItem("Stop", self._stop_app),
        )

    def _league_text(self, key: str) -> str:
        if key not in self._config.leagues:
            return key
        collector = self._collector
        status = collector.snapshot().get(key) if collector else None
        if status is None or status.last_poll_epoch is None:
            return f"{key} — waiting"
        counts = [f"{n} {s}" for s in ("in", "pre", "post") if (n := status.states.get(s))]
        summary = " / ".join(counts) if counts else "no events"
        stamp = datetime.fromtimestamp(status.last_poll_epoch).strftime("%H:%M:%S")
        health = "ok" if status.last_http_status == 200 else f"HTTP {status.last_http_status}"
        return f"{key} — {summary}, {health} {stamp}"

    def _tooltip(self) -> str:
        if self._crashed:
            return "ESPN collector — CRASHED, see data/espn/tray.log"
        if self._pause.is_set():
            return "ESPN collector — paused"
        if not self._config.leagues:
            return "ESPN collector — no leagues selected"
        collector = self._collector
        snapshot = collector.snapshot() if collector else {}
        live = sum(1 for s in snapshot.values() if s.live)
        polls = sum(s.polls for s in snapshot.values())
        return f"ESPN collector — {len(self._config.leagues)} leagues, {live} live, {polls} polls"

    # -- menu actions ------------------------------------------------------

    def _toggle_league(self, key: str, icon=None, item=None) -> None:
        leagues = set(self._config.leagues)
        if key in leagues:
            leagues.discard(key)
        else:
            leagues.add(key)
        self._config.leagues = leagues
        self._config.save()
        self._log(f"leagues -> {', '.join(sorted(leagues)) or '(none)'}")
        self._session_stop.set()

    def _set_idle_interval(self, seconds: int, icon=None, item=None) -> None:
        self._config.idle_interval = seconds
        self._config.save()
        self._log(f"idle interval -> {seconds}s")
        self._session_stop.set()

    def _toggle_pause(self, icon: pystray.Icon, item: pystray.MenuItem) -> None:
        if self._pause.is_set():
            self._pause.clear()
            self._log("resumed")
        else:
            self._pause.set()
            self._log("paused")
        self._apply_icon_state()

    def _stop_app(self, icon: pystray.Icon, item: pystray.MenuItem) -> None:
        self._log("stop requested from menu")
        self._quit.set()
        self._session_stop.set()
        icon.stop()

    def _apply_icon_state(self) -> None:
        state = "crashed" if self._crashed else "paused" if self._pause.is_set() else "running"
        self._icon.icon = self._icons[state]

    # -- logging -----------------------------------------------------------

    def _rotate_log(self) -> None:
        try:
            if self._log_path.stat().st_size > LOG_ROTATE_BYTES:
                os.replace(self._log_path, self._log_path.with_suffix(".log.old"))
        except OSError:
            pass

    def _log(self, message: str) -> None:
        with self._log_lock:
            with self._log_path.open("a", encoding="utf-8") as f:
                f.write(message + "\n")
        if sys.stdout is not None:
            print(message, flush=True)


# -- startup registration & single instance -------------------------------


def startup_command() -> str:
    pythonw = Path(sys.executable).resolve().with_name("pythonw.exe")
    launcher = Path(__file__).resolve().with_name("tray_launcher.pyw")
    return f'"{pythonw}" "{launcher}"'


def install_startup() -> str:
    value = startup_command()
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY, 0, winreg.KEY_SET_VALUE) as key:
        winreg.SetValueEx(key, APP_NAME, 0, winreg.REG_SZ, value)
    return value


def uninstall_startup() -> bool:
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY, 0, winreg.KEY_SET_VALUE) as key:
            winreg.DeleteValue(key, APP_NAME)
        return True
    except FileNotFoundError:
        return False


def _acquire_single_instance() -> int | None:
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    handle = kernel32.CreateMutexW(None, False, MUTEX_NAME)
    if ctypes.get_last_error() == ERROR_ALREADY_EXISTS:
        kernel32.CloseHandle(handle)
        return None
    return handle


def main(db_path: str | Path | None = None) -> int:
    mutex = _acquire_single_instance()
    if mutex is None:
        ctypes.windll.user32.MessageBoxW(
            None, "The ESPN tray collector is already running.", "ESPN collector", 0x40
        )
        return 0
    try:
        return TrayApp(Path(db_path) if db_path else DEFAULT_DB).run()
    finally:
        ctypes.WinDLL("kernel32").CloseHandle(mutex)

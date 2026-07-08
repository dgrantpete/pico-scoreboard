"""Scenario registry: named display states driven through the REAL firmware.

Each scenario mutates the actual `scoreboard.state` mailbox with the real
setters (`get_write_state` / `commit_state`, `set_error`, `set_toast`,
`set_setup_mode`, ...) and hand-builds domain objects (`LiveGame`, `Inning`,
`TeamState`, ...) via their plain constructors -- exactly what the poller does
on Core 0. The preview then renders whatever those setters published, so a
scenario exercises the same code the device runs.

A scenario declares a `duration_ms`: 0 renders a single static frame; a
positive value renders `duration_ms // 50` frames at the 50 ms display tick and
is assembled into a GIF (animations advance the virtual clock, never real time).

`compatible_variants` (a set of variant names, or None for "all") lets a
scenario opt out of variants that don't apply to it.
"""

from .shims.time_shim import VirtualClock

# Registry populated by the @scenario decorator, insertion-ordered.
REGISTRY: "dict[str, Scenario]" = {}


class Scenario:
    def __init__(self, name, setup, duration_ms, compatible_variants):
        self.name = name
        self.setup = setup
        self.duration_ms = duration_ms
        self.compatible_variants = compatible_variants

    def frame_count(self) -> int:
        if self.duration_ms <= 0:
            return 1
        return max(1, self.duration_ms // 50)


def scenario(name, duration_ms=0, compatible_variants=None):
    def deco(fn):
        REGISTRY[name] = Scenario(name, fn, duration_ms, compatible_variants)
        return fn
    return deco


class ScenarioContext:
    """Handed to each scenario: firmware modules, the clock, and the logos."""

    def __init__(self, env, logos) -> None:
        self.env = env
        self.clock: VirtualClock = env.clock
        self.logos = logos
        self.mlb = env.mlb
        self.state = env.state
        self.display = env.display
        self.fonts = env.fonts
        self.config = env.config

    def reset(self) -> None:
        """Return the write buffer to a clean baseline so scenarios don't bleed.

        Clears the per-mode fields (live game, toast, play, error, logos) and
        republishes UI colors from the config defaults through the real
        `update_ui_colors` path, then commits an idle baseline.
        """
        st = self.state.get_write_state()
        st.mode = "idle"
        st.animation_start_ms = self.clock.now
        st.home_logo = None
        st.away_logo = None
        st.toast.text = ""
        st.toast.updated_ms = 0
        st.error.title = ""
        st.error.lines = []
        game = st.game
        game.game_id = ""
        game.live = None
        game.fetched_ms = self.clock.now
        play = game.play
        play.id = ""
        play.text = ""
        play.updated_ms = 0
        play.display_ms = 0
        self.state.commit_state()
        # Publish config-default UI colors via the real firmware path.
        self.state.update_ui_colors(self.config.Config())


# --- Team color palette (packed 0x00RRGGBB, primary + alternate) -------------

_TEAMS = {
    "BOS": (0xBD3039, 0x0C2340),
    "NYY": (0x0C2340, 0xFFFFFF),
    "LAD": (0x005A9C, 0xEF3E42),
    "SF":  (0xFD5A1E, 0x27251F),
}


def _live_game(mlb, *, away, home, inning_num, half, balls, strikes, outs,
               bases, away_score, home_score, at_bat, play_text):
    ap, aa = _TEAMS[away]
    hp, ha = _TEAMS[home]
    first, second, third = bases
    return mlb.LiveGame(
        game_id="401555001",
        inning=mlb.Inning(inning_num, half),
        home=mlb.TeamState(home, home_score, mlb.TeamColors(hp, ha)),
        away=mlb.TeamState(away, away_score, mlb.TeamColors(ap, aa)),
        count=mlb.Count(balls, strikes, outs),
        bases=mlb.Bases(first, second, third),
        at_bat=at_bat,
        last_play=mlb.LastPlay("play-1", play_text),
    )


def _publish_live(ctx: ScenarioContext, live, play_text=None, play_id=None):
    """Set mode=game with `live` published + logos; optionally arm a play flash."""
    st = ctx.state.get_write_state()
    st.mode = "game"
    st.animation_start_ms = ctx.clock.now
    st.game.game_id = live.game_id
    st.game.live = live
    st.game.fetched_ms = ctx.clock.now
    ap, aa = _TEAMS[live.away.abbreviation]
    hp, ha = _TEAMS[live.home.abbreviation]
    st.away_logo = ctx.logos.get(live.away.abbreviation, ap, aa)
    st.home_logo = ctx.logos.get(live.home.abbreviation, hp, ha)
    if play_text is not None:
        st.game.play.id = play_id or "play-flash"
        st.game.play.text = play_text
        st.game.play.updated_ms = ctx.clock.now
        st.game.play.display_ms = ctx.display.play_text_display_ms(play_text)
    ctx.state.commit_state()


# =============================================================================
# Non-game screens
# =============================================================================

@scenario("startup")
def startup(ctx: ScenarioContext) -> None:
    # set_startup_step publishes mode='startup' and pre-builds the strings.
    ctx.state.set_startup_step(3, 5, "Connecting WiFi", "ssid: home-network")


@scenario("idle")
def idle(ctx: ScenarioContext) -> None:
    ctx.state.set_mode("idle")


@scenario("no_games")
def no_games(ctx: ScenarioContext) -> None:
    ctx.state.set_mode("no_games")


@scenario("error")
def error(ctx: ScenarioContext) -> None:
    ctx.state.set_error("API ERROR", [
        "Connection refused",
        "backend unreachable",
        "retrying in 30s",
    ])


@scenario("setup-fresh")
def setup_fresh(ctx: ScenarioContext) -> None:
    # miqro is not stubbed, so QR generation degrades to no-QR (text only).
    ctx.state.set_setup_mode(
        reason="no_config", ap_ssid="scoreboard", ap_ip="192.168.4.1"
    )


# =============================================================================
# Live game screens
# =============================================================================

@scenario("live-basic")
def live_basic(ctx: ScenarioContext) -> None:
    live = _live_game(
        ctx.mlb, away="BOS", home="NYY", inning_num=5, half=ctx.mlb.TOP,
        balls=1, strikes=1, outs=0, bases=(True, False, True),
        away_score=3, home_score=2,
        at_bat=ctx.mlb.AtBat("G. Cole", "R. Devers"),
        play_text="Ball 1.",
    )
    _publish_live(ctx, live)


@scenario("live-critical-count", duration_ms=2000)
def live_critical_count(ctx: ScenarioContext) -> None:
    # Full count, two outs: every dot group pulses (two 1000 ms cycles).
    live = _live_game(
        ctx.mlb, away="LAD", home="SF", inning_num=9, half=ctx.mlb.BOTTOM,
        balls=3, strikes=2, outs=2, bases=(True, True, True),
        away_score=4, home_score=4,
        at_bat=ctx.mlb.AtBat("C. Webb", "M. Betts"),
        play_text="Foul ball.",
    )
    _publish_live(ctx, live)


@scenario("live-play-flash", duration_ms=4000)
def live_play_flash(ctx: ScenarioContext) -> None:
    # Long play text scrolls one full cycle; duration is set below to match.
    live = _live_game(
        ctx.mlb, away="BOS", home="NYY", inning_num=7, half=ctx.mlb.BOTTOM,
        balls=0, strikes=0, outs=1, bases=(False, False, False),
        away_score=5, home_score=6,
        at_bat=ctx.mlb.AtBat("K. Jansen", "A. Judge"),
        play_text=None,
    )
    text = "Judge homers (28) on a fly ball to deep left center. Stanton scores."
    _publish_live(ctx, live, play_text=text, play_id="hr-28")
    # One full scroll cycle for this exact text.
    live_play_flash.scenario.duration_ms = ctx.display.play_text_display_ms(text)


@scenario("live-toast-skipping", duration_ms=1500)
def live_toast_skipping(ctx: ScenarioContext) -> None:
    live = _live_game(
        ctx.mlb, away="LAD", home="SF", inning_num=3, half=ctx.mlb.TOP,
        balls=2, strikes=1, outs=1, bases=(True, False, False),
        away_score=1, home_score=0,
        at_bat=ctx.mlb.AtBat("L. Webb", "F. Freeman"),
        play_text="Strike 1.",
    )
    _publish_live(ctx, live)
    # The button-feedback toast overlays the play-text region.
    ctx.state.set_toast("SKIPPING")


@scenario("live-no-atbat")
def live_no_atbat(ctx: ScenarioContext) -> None:
    # Middle of the inning: no active at-bat, so pitcher/batter go blank.
    live = _live_game(
        ctx.mlb, away="BOS", home="NYY", inning_num=6, half=ctx.mlb.MIDDLE,
        balls=0, strikes=0, outs=3, bases=(False, False, False),
        away_score=2, home_score=2,
        at_bat=None,
        play_text="",
    )
    _publish_live(ctx, live)


# Back-reference each function to its Scenario so a setup can tune duration.
for _s in REGISTRY.values():
    _s.setup.scenario = _s

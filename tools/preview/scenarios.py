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
        # Mirror the poller: pre-render the strip so previews exercise the
        # same fast path the device renders with.
        st.game.play.strip = ctx.state.build_play_strip(play_text)
    ctx.state.commit_state()


# =============================================================================
# Non-game screens
# =============================================================================

@scenario("startup", compatible_variants={"default"})
def startup(ctx: ScenarioContext) -> None:
    # set_startup_step publishes mode='startup' and pre-builds the strings.
    ctx.state.set_startup_step(3, 5, "Connecting WiFi", "ssid: home-network")


@scenario("idle", compatible_variants={"default"})
def idle(ctx: ScenarioContext) -> None:
    ctx.state.set_mode("idle")


@scenario("no_games", compatible_variants={"default"})
def no_games(ctx: ScenarioContext) -> None:
    ctx.state.set_mode("no_games")


@scenario("error", compatible_variants={"default"})
def error(ctx: ScenarioContext) -> None:
    ctx.state.set_error("API ERROR", [
        "Connection refused",
        "backend unreachable",
        "retrying in 30s",
    ])


@scenario("setup-fresh", compatible_variants={"default"})
def setup_fresh(ctx: ScenarioContext) -> None:
    # miqro is not stubbed, so QR generation degrades to no-QR (text only).
    ctx.state.set_setup_mode(
        reason="no_config", ap_ssid="scoreboard", ap_ip="192.168.4.1"
    )


# =============================================================================
# Live game screens
# =============================================================================

@scenario("live-basic", compatible_variants={"default"})
def live_basic(ctx: ScenarioContext) -> None:
    live = _live_game(
        ctx.mlb, away="BOS", home="NYY", inning_num=5, half=ctx.mlb.TOP,
        balls=1, strikes=1, outs=0, bases=(True, False, True),
        away_score=3, home_score=2,
        at_bat=ctx.mlb.AtBat("G. Cole", "R. Devers"),
        play_text="Ball 1.",
    )
    _publish_live(ctx, live)


@scenario("live-critical-count", duration_ms=2000, compatible_variants={"default"})
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


@scenario("live-play-flash", duration_ms=4000, compatible_variants={"default"})
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


@scenario("live-toast-skipping", duration_ms=1500, compatible_variants={"default"})
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


@scenario("live-no-atbat", compatible_variants={"default"})
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


# =============================================================================
# Pregame screens
# =============================================================================

_PREGAME_VARIANTS = {"pregame-A", "pregame-B", "pregame-C"}
_FINAL_VARIANTS = {"final-A", "final-B", "final-C"}
_RED_TINT_VARIANTS = {"red-tint-0", "red-tint-48", "red-tint-80", "red-tint-128"}


def _pregame_team(mlb, abbr, wins, losses, pitcher):
    p, a = _TEAMS[abbr]
    return mlb.PregameTeam(abbr, mlb.TeamColors(p, a), wins, losses, pitcher)


def _publish_pregame(ctx, game, utc_offset_s):
    """Fetch logos and drive the REAL set_pregame with a PregameGame."""
    ap, aa = _TEAMS[game.away.abbreviation]
    hp, ha = _TEAMS[game.home.abbreviation]
    away_logo = ctx.logos.get(game.away.abbreviation, ap, aa)
    home_logo = ctx.logos.get(game.home.abbreviation, hp, ha)
    ctx.state.set_pregame(game, home_logo, away_logo, utc_offset_s)


# Start epoch chosen with the offset below so the local first pitch reads
# "7:05 PM" (gmtime(83100 - 14400) == 1970-01-01 19:05:00Z).
_START_EPOCH = 83100
_UTC_OFFSET_EDT = -4 * 3600


@scenario("pregame-full", duration_ms=12000, compatible_variants=_PREGAME_VARIANTS)
def pregame_full(ctx: ScenarioContext) -> None:
    game = ctx.mlb.PregameGame(
        game_id="401570100",
        start_epoch=_START_EPOCH,
        venue="Oriole Park at Camden Yards",
        weather_temp=72,
        weather_condition="Partly Cloudy",
        away=_pregame_team(ctx.mlb, "BOS", 47, 42, "K. Gausman"),
        home=_pregame_team(ctx.mlb, "NYY", 53, 38, "G. Cole"),
    )
    _publish_pregame(ctx, game, _UTC_OFFSET_EDT)


@scenario("pregame-sparse", duration_ms=8000, compatible_variants=_PREGAME_VARIANTS)
def pregame_sparse(ctx: ScenarioContext) -> None:
    # No weather, no records, no local time (utc offset unknown).
    game = ctx.mlb.PregameGame(
        game_id="401570101",
        start_epoch=_START_EPOCH,
        venue="Yankee Stadium",
        weather_temp=None,
        weather_condition=None,
        away=_pregame_team(ctx.mlb, "LAD", None, None, "T. Glasnow"),
        home=_pregame_team(ctx.mlb, "SF", None, None, "L. Webb"),
    )
    _publish_pregame(ctx, game, None)


@scenario("pregame-no-time", duration_ms=9000, compatible_variants=_PREGAME_VARIANTS)
def pregame_no_time(ctx: ScenarioContext) -> None:
    # Full data but the time phase is omitted (offset None) -- the cycle drops
    # to venue<->weather; the "Big time" variant shows no clock.
    game = ctx.mlb.PregameGame(
        game_id="401570102",
        start_epoch=_START_EPOCH,
        venue="Oracle Park",
        weather_temp=64,
        weather_condition="Windy",
        away=_pregame_team(ctx.mlb, "BOS", 47, 42, "B. Bello"),
        home=_pregame_team(ctx.mlb, "SF", 44, 45, "B. Snell"),
    )
    _publish_pregame(ctx, game, None)


# =============================================================================
# Final screens
# =============================================================================

def _final_team(mlb, abbr, score, line):
    p, a = _TEAMS[abbr]
    return mlb.FinalTeam(abbr, mlb.TeamColors(p, a), score, bytes(line))


def _publish_final(ctx, game):
    ap, aa = _TEAMS[game.away.abbreviation]
    hp, ha = _TEAMS[game.home.abbreviation]
    away_logo = ctx.logos.get(game.away.abbreviation, ap, aa)
    home_logo = ctx.logos.get(game.home.abbreviation, hp, ha)
    ctx.state.set_final(game, home_logo, away_logo)


@scenario("final-9", duration_ms=6000, compatible_variants=_FINAL_VARIANTS)
def final_9(ctx: ScenarioContext) -> None:
    game = ctx.mlb.FinalGame(
        game_id="401570200",
        innings_played=9,
        away=_final_team(ctx.mlb, "BOS", 3, (0, 0, 1, 0, 0, 2, 0, 0, 0)),
        home=_final_team(ctx.mlb, "NYY", 4, (0, 1, 0, 0, 0, 0, 2, 1, 0)),
    )
    _publish_final(ctx, game)


@scenario("final-walkoff", duration_ms=6000, compatible_variants=_FINAL_VARIANTS)
def final_walkoff(ctx: ScenarioContext) -> None:
    # Home led after the top of the 9th, so the bottom half was never played:
    # home line is one column short -> " X " in the 9th.
    game = ctx.mlb.FinalGame(
        game_id="401570201",
        innings_played=9,
        away=_final_team(ctx.mlb, "LAD", 3, (1, 0, 0, 2, 0, 0, 0, 0, 0)),
        home=_final_team(ctx.mlb, "SF", 4, (0, 2, 0, 1, 0, 1, 0, 0)),
    )
    _publish_final(ctx, game)


@scenario("final-extras", duration_ms=11000, compatible_variants=_FINAL_VARIANTS)
def final_extras(ctx: ScenarioContext) -> None:
    # 13 innings ("F/13"): the line score is wide and scrolls.
    game = ctx.mlb.FinalGame(
        game_id="401570202",
        innings_played=13,
        away=_final_team(ctx.mlb, "BOS", 4, (0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1)),
        home=_final_team(ctx.mlb, "NYY", 5, (1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3)),
    )
    _publish_final(ctx, game)


@scenario("final-blowout", duration_ms=6000, compatible_variants=_FINAL_VARIANTS)
def final_blowout(ctx: ScenarioContext) -> None:
    # A 2-digit inning run (10) and a 2-digit total (13) exercise cell width.
    game = ctx.mlb.FinalGame(
        game_id="401570203",
        innings_played=9,
        away=_final_team(ctx.mlb, "LAD", 13, (0, 0, 10, 0, 0, 2, 0, 1, 0)),
        home=_final_team(ctx.mlb, "SF", 1, (0, 0, 0, 0, 1, 0, 0, 0, 0)),
    )
    _publish_final(ctx, game)


# =============================================================================
# Polish animations
# =============================================================================

@scenario("toast-dim-pulse", duration_ms=2500, compatible_variants={"default"})
def toast_dim_pulse(ctx: ScenarioContext) -> None:
    # A sticky SKIPPING toast (in-flight skip) whose one rejected-press dim
    # cycle plays out over the first ~1000ms of the GIF.
    live = _live_game(
        ctx.mlb, away="LAD", home="SF", inning_num=3, half=ctx.mlb.TOP,
        balls=2, strikes=1, outs=1, bases=(True, False, False),
        away_score=1, home_score=0,
        at_bat=ctx.mlb.AtBat("L. Webb", "F. Freeman"),
        play_text="Strike 1.",
    )
    _publish_live(ctx, live)
    ctx.state.set_toast("SKIPPING", sticky=True)
    ctx.state.pulse_toast()   # stamps the dim at t0; set_toast must precede it


@scenario("critical-red-tint", duration_ms=2000,
          compatible_variants=_RED_TINT_VARIANTS)
def critical_red_tint(ctx: ScenarioContext) -> None:
    # Full count, two outs: the critical dots warm from white toward pale red.
    # The red-tint-* variants sweep CRITICAL_PULSE_S_MAX.
    live = _live_game(
        ctx.mlb, away="LAD", home="SF", inning_num=9, half=ctx.mlb.BOTTOM,
        balls=3, strikes=2, outs=2, bases=(True, True, True),
        away_score=4, home_score=4,
        at_bat=ctx.mlb.AtBat("C. Webb", "M. Betts"),
        play_text="Foul ball.",
    )
    _publish_live(ctx, live)


# Back-reference each function to its Scenario so a setup can tune duration.
for _s in REGISTRY.values():
    _s.setup.scenario = _s

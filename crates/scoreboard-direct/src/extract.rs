//! The owned side of the seam: one polled game, and the borrowed view the
//! display stack consumes.

use scoreboard_espn::{football, mlb, nba, soccer};
use scoreboard_model::feed::GameDetail;
use scoreboard_wire::GameState;

use crate::{CommentaryExtract, Sport};

/// One extracted game, owned and bounded — the direct feed's answer to a
/// decoded wire payload.
///
/// The variants are `scoreboard-espn`'s own extract structs, unchanged: they
/// are already the post-transform domain shape, already bound at the wire's
/// string cap, and already pinned byte-for-byte to the committed goldens
/// through their `as_game()` views. This enum adds the sport tag the wire
/// format carries in its endpoint and nothing else.
///
/// Sized by its largest variant (football's, which alone carries a venue, a
/// clock, a rank line per team and two line scores) rather than by the game in
/// hand — the price of `no_std` without an allocator, and the reason the
/// poller holds exactly one.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "bounded owned variants are the design; boxing needs alloc"
)]
pub enum DirectExtract {
    Mlb(mlb::Extract),
    Nba(nba::Extract),
    Football(football::GameExtract),
    Soccer(soccer::SoccerExtract),
}

impl DirectExtract {
    /// The borrowed view `Store` consumes — the whole point of the crate.
    ///
    /// Every field borrows this extract's own storage, so the view is valid
    /// exactly as long as the extract is and no copy happens here. `Store`
    /// copies what it keeps into its bounded snapshot fields, as it does in
    /// wire mode.
    pub fn detail(&self) -> GameDetail<'_> {
        match self {
            DirectExtract::Mlb(extract) => GameDetail::Mlb(extract.as_game()),
            DirectExtract::Nba(extract) => GameDetail::Nba(extract.as_game()),
            DirectExtract::Football(extract) => GameDetail::Football(extract.as_game()),
            DirectExtract::Soccer(extract) => GameDetail::Soccer(extract.as_game()),
        }
    }

    pub fn sport(&self) -> Sport {
        match self {
            DirectExtract::Mlb(_) => Sport::Mlb,
            DirectExtract::Nba(_) => Sport::Nba,
            DirectExtract::Football(_) => Sport::Football,
            DirectExtract::Soccer(_) => Sport::Soccer,
        }
    }

    /// The event id this extract was fetched for. Read from the extract rather
    /// than remembered from the request, so it is the id ESPN actually served.
    pub fn game_id(&self) -> &str {
        match self {
            DirectExtract::Mlb(extract) => extract.game_id(),
            DirectExtract::Nba(extract) => extract.game_id.as_str(),
            DirectExtract::Football(extract) => extract.game_id(),
            DirectExtract::Soccer(extract) => match extract {
                soccer::SoccerExtract::Pregame(game) => game.game_id.as_str(),
                soccer::SoccerExtract::Live(game) => game.game_id.as_str(),
                soccer::SoccerExtract::Final(game) => game.game_id.as_str(),
            },
        }
    }

    pub fn state(&self) -> GameState {
        match self {
            DirectExtract::Mlb(extract) => extract.state(),
            DirectExtract::Nba(extract) => match extract.kind {
                nba::Kind::Pregame(_) => GameState::Pregame,
                nba::Kind::Live(_) => GameState::Live,
                nba::Kind::Final(_) => GameState::Final,
            },
            DirectExtract::Football(extract) => extract.state(),
            DirectExtract::Soccer(extract) => match extract {
                soccer::SoccerExtract::Pregame(_) => GameState::Pregame,
                soccer::SoccerExtract::Live(_) => GameState::Live,
                soccer::SoccerExtract::Final(_) => GameState::Final,
            },
        }
    }

    /// Attach (or clear) the commentary line from a soccer summary pass.
    ///
    /// A no-op for every other sport and for non-live soccer, because only
    /// `scoreboard_wire::soccer::Live` has a commentary slot. Calling it with
    /// `None` after a failed summary fetch is the correct degradation, not an
    /// error path: the live payload is complete without it.
    pub fn set_commentary(&mut self, commentary: Option<CommentaryExtract>) {
        if let DirectExtract::Soccer(extract) = self {
            extract.set_commentary(commentary);
        }
    }

    /// True when a soccer summary fetch would add anything — the one condition
    /// under which the poller spends a second body on a game.
    pub fn wants_commentary(&self) -> bool {
        matches!(self, DirectExtract::Soccer(soccer::SoccerExtract::Live(_)))
    }
}

/// Measured 2,916 bytes on `thumbv8m.main-none-eabihf` and 2,968 on the host
/// (the difference is `heapless`' `usize` lengths). The poller holds one, so
/// this is a static-budget line, not a per-game cost — but it is the largest
/// single thing the direct path adds, and BUDGET.md takes measured numbers.
/// Asserted here rather than only in a host test so the *device* build is the
/// one that fails when a bound moves.
const _: () = assert!(
    core::mem::size_of::<DirectExtract>() <= 4096,
    "DirectExtract outgrew its budget; re-measure before raising this"
);

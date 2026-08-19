//! Print the streaming-state sizes the device budgets against.
//!
//! `cargo run -p scoreboard-espn --example sizes` — host numbers (device is a
//! few percent smaller: heapless lengths are `usize`). These are the values
//! that size `scoreboard-direct`'s stream enums, the poller's task pool, and
//! — because big values move by memcpy through the poll frame — the poll
//! task's stack peaks. The S3 stack crunch was diagnosed by exactly this
//! arithmetic; keep the tool.

use core::mem::size_of;

use scoreboard_espn::common::{IgnoreQuirks, NoRows};
use scoreboard_espn::{football, mlb, nba, soccer};

fn main() {
    println!("detail extractors (streaming state, host):");
    println!(
        "  mlb::DetailExtractor      {:>7}",
        size_of::<mlb::DetailExtractor<'static, 'static, IgnoreQuirks>>()
    );
    println!(
        "  nba::Extractor            {:>7}",
        size_of::<nba::Extractor<'static, NoRows, IgnoreQuirks>>()
    );
    println!(
        "  football::DetailExtractor {:>7}",
        size_of::<football::DetailExtractor<'static, IgnoreQuirks>>()
    );
    println!(
        "  soccer::GameExtractor     {:>7}",
        size_of::<soccer::GameExtractor<'static, IgnoreQuirks>>()
    );
    println!("list extractors:");
    println!(
        "  mlb::ListExtractor        {:>7}",
        size_of::<mlb::ListExtractor<'static, 'static, NoRows, IgnoreQuirks>>()
    );
    println!(
        "  football::ListExtractor   {:>7}",
        size_of::<football::ListExtractor<'static, NoRows, IgnoreQuirks>>()
    );
    println!(
        "  soccer::ListExtractor     {:>7}",
        size_of::<soccer::ListExtractor<'static, NoRows, IgnoreQuirks>>()
    );
    println!("extracts (owned results):");
    println!("  mlb::Extract              {:>7}", size_of::<mlb::Extract>());
    println!("  nba::Extract              {:>7}", size_of::<nba::Extract>());
    println!(
        "  football::GameExtract     {:>7}",
        size_of::<football::GameExtract>()
    );
    println!(
        "  soccer::SoccerExtract     {:>7}",
        size_of::<soccer::SoccerExtract>()
    );
}

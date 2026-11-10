//! How a sport divides regulation, as the screens name it.
//!
//! A table of names rather than a count and a naming scheme side by side: the
//! number of periods *is* the table's length, so "two periods named Q1 and
//! Q2" cannot be built. The field is private, so the only values that exist
//! are the ones defined here (NCAAB.md D2).

use crate::snapshot::SHORT;
use crate::text::{Text, set_plain};

/// The regulation periods' names; overtime is every period past them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Periods(&'static [&'static str]);

impl Periods {
    /// Four quarters: the NBA, NCAA women's basketball, football.
    pub const QUARTERS: Periods = Periods(&["Q1", "Q2", "Q3", "Q4"]);
    /// Two halves: NCAA men's basketball — the ordinals soccer and hockey use.
    pub const HALVES: Periods = Periods(&["1ST", "2ND"]);

    /// How many periods regulation has.
    pub fn regulation(self) -> u8 {
        self.0.len() as u8
    }

    /// A period's chip: the table's name through regulation, then "OT",
    /// "2OT", …. Period 0 — a live body before the tip — has no name.
    pub fn write_name(self, period: u8, dst: &mut Text<SHORT>) {
        dst.clear();
        let regulation = self.regulation();
        match period {
            0 => {}
            p if p <= regulation => set_plain(dst, self.0[p as usize - 1]),
            p if p == regulation + 1 => set_plain(dst, "OT"),
            p => {
                let overtimes = p - regulation;
                crate::text::write_text!(dst, "{overtimes}OT");
            }
        }
    }
}

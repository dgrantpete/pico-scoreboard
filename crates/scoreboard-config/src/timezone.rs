//! The browser-seeded timezone offset schedule: the record, its encoding,
//! and the DST flip.
//!
//! The app's `timezone` module owns the storage key, the live cell, and the
//! HTTP seam; this module is the pure half — no embassy, no storage, no
//! device anything — living here rather than in the app for the same reason
//! `scoreboard_log::breadcrumb` and `scoreboard_ota::attempt` live in
//! crates: the app builds only for thumbv8m and has no host-test path, and
//! an encoding whose tests cannot run has no tests. This crate already
//! carries the serde surface for browser-posted documents, which is the
//! dependency profile the record needs; the storage-key separation the
//! app-side docs argue for is about the *key*, and is unchanged.

use serde::Deserialize;

/// The stored record's on-flash size. Fixed: every field is present in
/// every record, and the flag byte says which of them mean anything.
pub const MAX_BYTES: usize = 12;

/// Bumped when the layout below changes. A record that does not carry this
/// reads back as absent, which puts the device in the state it was in
/// before anyone seeded it — the same answer [`super::load`] gives a device
/// that has never been visited.
const VERSION: u8 = 1;

const FLAG_SCHEDULE: u8 = 0b0000_0001;
const FLAG_MANUAL: u8 = 0b0000_0010;

/// UTC−12:00 through UTC+14:00, which is every offset IANA uses.
///
/// The range is the safety property, not a formatting nicety: it is what
/// bounds the arithmetic in [`super::offset_seconds_at`], and it is checked
/// on the way in *and* on the way out of flash, so a bit-rotted record
/// cannot put a nonsense hour on the panel.
const MIN_OFFSET_MINUTES: i32 = -12 * 60;
const MAX_OFFSET_MINUTES: i32 = 14 * 60;

/// 2020-01-01T00:00:00Z. A transition instant below this is not one a
/// browser computed — it is a client sending something other than unix
/// seconds, and answering `400` is more useful than storing it.
const EARLIEST_TRANSITION: u64 = 1_577_836_800;

/// The seeded half: an offset, and optionally the instant it stops being
/// the answer.
///
/// `next` is `None` for a zone with no DST at all — Arizona, Iceland, most
/// of Asia — which is a flat offset forever and needs no refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub offset_minutes: i16,
    pub next: Option<Transition>,
}

/// When the offset changes, and to what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// Unix seconds, UTC.
    pub at_epoch_s: u32,
    pub offset_minutes: i16,
}

/// Everything the device stores about its timezone.
///
/// `Default` is the never-seeded device: no schedule, no override, and
/// therefore no opinion about local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Record {
    pub schedule: Option<Schedule>,
    /// The manual override. Wins over `schedule` whenever it is set — see
    /// the module docs on precedence.
    pub manual_minutes: Option<i16>,
}

impl Record {
    /// The offset in force at `now_epoch_s`, or `None` if nobody has ever
    /// told this device where it is.
    ///
    /// Pure, total, and the only place the precedence rule is written.
    pub fn offset_minutes_at(&self, now_epoch_s: u32) -> Option<i16> {
        if let Some(manual) = self.manual_minutes {
            return Some(manual);
        }
        let schedule = self.schedule?;
        match schedule.next {
            Some(next) if now_epoch_s >= next.at_epoch_s => Some(next.offset_minutes),
            _ => Some(schedule.offset_minutes),
        }
    }

    /// Pack into the flash record.
    ///
    /// ```text
    /// 0      version
    /// 1..3   i16 LE  the schedule's current offset, minutes
    /// 3..5   i16 LE  the offset after the transition, minutes
    /// 5..9   u32 LE  the transition instant, unix seconds
    /// 9      flags   bit 0: a schedule is stored; bit 1: an override is
    /// 10..12 i16 LE  the manual override, minutes
    /// ```
    ///
    /// Fixed width rather than a tagged encoding, because twelve bytes is
    /// smaller than the smallest thing `sequential-storage` can write and
    /// the flags byte already carries every distinction the layout needs.
    pub fn encode(&self, out: &mut [u8; MAX_BYTES]) {
        let mut flags = 0u8;
        let (offset, next_offset, at) = match self.schedule {
            Some(schedule) => {
                flags |= FLAG_SCHEDULE;
                match schedule.next {
                    Some(next) => {
                        (schedule.offset_minutes, next.offset_minutes, next.at_epoch_s)
                    }
                    None => (schedule.offset_minutes, 0, 0),
                }
            }
            None => (0, 0, 0),
        };
        let manual = match self.manual_minutes {
            Some(minutes) => {
                flags |= FLAG_MANUAL;
                minutes
            }
            None => 0,
        };

        out[0] = VERSION;
        out[1..3].copy_from_slice(&offset.to_le_bytes());
        out[3..5].copy_from_slice(&next_offset.to_le_bytes());
        out[5..9].copy_from_slice(&at.to_le_bytes());
        out[9] = flags;
        out[10..12].copy_from_slice(&manual.to_le_bytes());
    }

    /// Read one back, or `None` if it is not a record this firmware wrote.
    ///
    /// Every value is re-checked against the same bounds the `PUT` checked,
    /// so the only way an out-of-range offset reaches the display is a bug
    /// on both sides of the flash.
    pub fn decode(bytes: &[u8]) -> Option<Record> {
        let bytes: &[u8; MAX_BYTES] = bytes.try_into().ok()?;
        if bytes[0] != VERSION {
            return None;
        }
        let offset = i16::from_le_bytes([bytes[1], bytes[2]]);
        let next_offset = i16::from_le_bytes([bytes[3], bytes[4]]);
        let at = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
        let flags = bytes[9];
        let manual = i16::from_le_bytes([bytes[10], bytes[11]]);

        let schedule = if flags & FLAG_SCHEDULE == 0 {
            None
        } else {
            let next = match at {
                0 => None,
                at => Some(Transition {
                    at_epoch_s: check_transition(u64::from(at))?,
                    offset_minutes: check_offset(i32::from(next_offset))?,
                }),
            };
            Some(Schedule {
                offset_minutes: check_offset(i32::from(offset))?,
                next,
            })
        };
        let manual_minutes = if flags & FLAG_MANUAL == 0 {
            None
        } else {
            Some(check_offset(i32::from(manual))?)
        };
        Some(Record {
            schedule,
            manual_minutes,
        })
    }
}

/// The `PUT /api/timezone` body, and what `GET` serves back.
///
/// Deliberately wider than the record: minutes arrive as `i32` and the
/// instant as `u64` so that an out-of-range value is *rejected* by
/// [`Document::into_record`] rather than silently wrapping during
/// deserialisation. It is what catches the one client mistake worth
/// catching — a transition posted in milliseconds, which is four billion
/// times too large and would otherwise truncate into a plausible date.
///
/// Unknown fields are ignored, which is serde's default and the same
/// leniency `ConfigPatch` relies on. That is what lets `GET`'s body, which
/// carries one derived field this does not name, be `PUT` back unchanged.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
pub struct Document {
    pub offset_minutes: Option<i32>,
    pub next_offset_minutes: Option<i32>,
    pub transition_epoch_s: Option<u64>,
    pub manual_offset_minutes: Option<i32>,
}

/// Why a document was refused. Each maps to one error code in the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    /// An offset outside UTC−12:00..=UTC+14:00.
    Offset,
    /// A transition without an offset to transition to (or the reverse), a
    /// transition alongside no current offset at all, or an instant that is
    /// not plausibly unix seconds.
    Schedule,
}

impl Document {
    /// Validate the whole document into a record, or refuse all of it.
    ///
    /// All-or-nothing, like `DeviceConfig::apply`: a body with one bad
    /// field changes nothing, so a client cannot half-succeed and be left
    /// guessing which half.
    ///
    /// Absent fields mean absent values — this is a **replacement**, not a
    /// patch. See the HTTP-surface docs in [`crate::http::routes`] for why
    /// this endpoint is not shaped like `PUT /api/config`.
    pub fn into_record(self) -> Result<Record, Invalid> {
        let manual_minutes = match self.manual_offset_minutes {
            Some(minutes) => Some(check_offset(minutes).ok_or(Invalid::Offset)?),
            None => None,
        };

        let next = match (self.next_offset_minutes, self.transition_epoch_s) {
            (Some(minutes), Some(at)) => Some(Transition {
                at_epoch_s: check_transition(at).ok_or(Invalid::Schedule)?,
                offset_minutes: check_offset(minutes).ok_or(Invalid::Offset)?,
            }),
            (None, None) => None,
            // Half a transition says nothing at all, and guessing the other
            // half would be inventing a date change.
            _ => return Err(Invalid::Schedule),
        };

        let schedule = match self.offset_minutes {
            Some(minutes) => Some(Schedule {
                offset_minutes: check_offset(minutes).ok_or(Invalid::Offset)?,
                next,
            }),
            // A transition off the end of a schedule that is not there.
            None if next.is_some() => return Err(Invalid::Schedule),
            None => None,
        };

        Ok(Record {
            schedule,
            manual_minutes,
        })
    }
}

fn check_offset(minutes: i32) -> Option<i16> {
    (MIN_OFFSET_MINUTES..=MAX_OFFSET_MINUTES)
        .contains(&minutes)
        .then_some(minutes as i16)
}

fn check_transition(epoch_s: u64) -> Option<u32> {
    (EARLIEST_TRANSITION..=u64::from(u32::MAX))
        .contains(&epoch_s)
        .then_some(epoch_s as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHICAGO_CST: i16 = -360;
    const CHICAGO_CDT: i16 = -300;
    /// 2027-03-14T08:00:00Z — the US spring-forward instant.
    const SPRING_FORWARD: u32 = 1_805_270_400;

    fn seeded() -> Record {
        Record {
            schedule: Some(Schedule {
                offset_minutes: CHICAGO_CST,
                next: Some(Transition {
                    at_epoch_s: SPRING_FORWARD,
                    offset_minutes: CHICAGO_CDT,
                }),
            }),
            manual_minutes: None,
        }
    }

    #[test]
    fn nothing_seeded_has_no_opinion() {
        assert_eq!(Record::default().offset_minutes_at(SPRING_FORWARD), None);
    }

    #[test]
    fn the_offset_flips_at_the_transition_and_not_before() {
        let record = seeded();
        assert_eq!(
            record.offset_minutes_at(SPRING_FORWARD - 1),
            Some(CHICAGO_CST)
        );
        assert_eq!(record.offset_minutes_at(SPRING_FORWARD), Some(CHICAGO_CDT));
        assert_eq!(
            record.offset_minutes_at(SPRING_FORWARD + 86_400),
            Some(CHICAGO_CDT)
        );
    }

    #[test]
    fn a_zone_without_dst_is_flat_forever() {
        let record = Record {
            schedule: Some(Schedule {
                offset_minutes: -420,
                next: None,
            }),
            manual_minutes: None,
        };
        assert_eq!(record.offset_minutes_at(0), Some(-420));
        assert_eq!(record.offset_minutes_at(u32::MAX), Some(-420));
    }

    #[test]
    fn the_manual_override_wins_on_both_sides_of_a_transition() {
        let record = Record {
            manual_minutes: Some(330),
            ..seeded()
        };
        assert_eq!(record.offset_minutes_at(SPRING_FORWARD - 1), Some(330));
        assert_eq!(record.offset_minutes_at(SPRING_FORWARD), Some(330));
    }

    #[test]
    fn clearing_the_override_restores_the_seeded_answer() {
        let overridden = Record {
            manual_minutes: Some(330),
            ..seeded()
        };
        let cleared = Record {
            manual_minutes: None,
            ..overridden
        };
        assert_eq!(cleared.offset_minutes_at(SPRING_FORWARD), Some(CHICAGO_CDT));
    }

    #[test]
    fn an_override_alone_answers_without_a_schedule() {
        let record = Record {
            schedule: None,
            manual_minutes: Some(0),
        };
        assert_eq!(record.offset_minutes_at(0), Some(0));
    }

    fn round_trip(record: Record) -> Option<Record> {
        let mut bytes = [0u8; MAX_BYTES];
        record.encode(&mut bytes);
        Record::decode(&bytes)
    }

    #[test]
    fn every_shape_survives_a_round_trip() {
        for record in [
            Record::default(),
            seeded(),
            Record {
                manual_minutes: Some(-720),
                ..seeded()
            },
            Record {
                schedule: Some(Schedule {
                    offset_minutes: 840,
                    next: None,
                }),
                manual_minutes: None,
            },
            Record {
                schedule: None,
                manual_minutes: Some(0),
            },
        ] {
            assert_eq!(round_trip(record), Some(record), "{record:?}");
        }
    }

    #[test]
    fn a_record_from_another_firmware_reads_as_absent() {
        let mut bytes = [0u8; MAX_BYTES];
        seeded().encode(&mut bytes);
        bytes[0] = VERSION + 1;
        assert_eq!(Record::decode(&bytes), None);
        assert_eq!(Record::decode(&bytes[..MAX_BYTES - 1]), None);
        assert_eq!(Record::decode(&[]), None);
    }

    #[test]
    fn a_corrupt_offset_reads_as_absent_rather_than_as_a_wrong_hour() {
        let mut bytes = [0u8; MAX_BYTES];
        seeded().encode(&mut bytes);
        bytes[1..3].copy_from_slice(&i16::MAX.to_le_bytes());
        assert_eq!(Record::decode(&bytes), None);
    }

    #[test]
    fn the_offset_range_is_utc_minus_twelve_to_plus_fourteen() {
        for minutes in [-721, 841, 100_000, -100_000] {
            assert_eq!(check_offset(minutes), None, "{minutes}");
        }
        for minutes in [-720, 0, 330, 840] {
            assert_eq!(check_offset(minutes), Some(minutes as i16), "{minutes}");
        }
    }

    #[test]
    fn a_transition_posted_in_milliseconds_is_refused() {
        let document = Document {
            offset_minutes: Some(-360),
            next_offset_minutes: Some(-300),
            transition_epoch_s: Some(u64::from(SPRING_FORWARD) * 1_000),
            manual_offset_minutes: None,
        };
        assert_eq!(document.into_record(), Err(Invalid::Schedule));
    }

    #[test]
    fn half_a_transition_is_refused() {
        let half = Document {
            offset_minutes: Some(-360),
            next_offset_minutes: Some(-300),
            transition_epoch_s: None,
            ..Document::default()
        };
        assert_eq!(half.into_record(), Err(Invalid::Schedule));

        let other_half = Document {
            offset_minutes: Some(-360),
            transition_epoch_s: Some(u64::from(SPRING_FORWARD)),
            ..Document::default()
        };
        assert_eq!(other_half.into_record(), Err(Invalid::Schedule));
    }

    #[test]
    fn a_transition_without_a_current_offset_is_refused() {
        let document = Document {
            offset_minutes: None,
            next_offset_minutes: Some(-300),
            transition_epoch_s: Some(u64::from(SPRING_FORWARD)),
            manual_offset_minutes: None,
        };
        assert_eq!(document.into_record(), Err(Invalid::Schedule));
    }

    #[test]
    fn an_out_of_range_offset_is_refused_whichever_field_carries_it() {
        let current = Document {
            offset_minutes: Some(900),
            ..Document::default()
        };
        assert_eq!(current.into_record(), Err(Invalid::Offset));

        let after = Document {
            offset_minutes: Some(-360),
            next_offset_minutes: Some(-900),
            transition_epoch_s: Some(u64::from(SPRING_FORWARD)),
            ..Document::default()
        };
        assert_eq!(after.into_record(), Err(Invalid::Offset));

        let manual = Document {
            manual_offset_minutes: Some(-900),
            ..Document::default()
        };
        assert_eq!(manual.into_record(), Err(Invalid::Offset));
    }

    #[test]
    fn an_empty_document_clears_everything() {
        assert_eq!(Document::default().into_record(), Ok(Record::default()));
    }

    fn parse(body: &str) -> Document {
        serde_json_core::from_slice::<Document>(body.as_bytes())
            .expect("parses")
            .0
    }

    /// The property the `GET`-only `effective_offset_minutes` field rests
    /// on: a body read from `GET` is a valid `PUT` body, because unknown
    /// fields are ignored.
    #[test]
    fn a_get_body_posts_back_unchanged() {
        let body = r#"{"offset_minutes":-360,"next_offset_minutes":-300,"transition_epoch_s":1805270400,"manual_offset_minutes":null,"effective_offset_minutes":-360}"#;
        assert_eq!(parse(body).into_record(), Ok(seeded()));
    }

    #[test]
    fn unknown_fields_are_ignored_however_deep() {
        let body = r#"{"offset_minutes":330,"iana":"Asia/Kolkata","nested":{"a":[1,2,3]}}"#;
        assert_eq!(
            parse(body).into_record(),
            Ok(Record {
                schedule: Some(Schedule {
                    offset_minutes: 330,
                    next: None
                }),
                manual_minutes: None,
            })
        );
    }

    #[test]
    fn an_explicit_null_reads_the_same_as_an_absent_field() {
        let explicit = r#"{"offset_minutes":null,"manual_offset_minutes":null}"#;
        assert_eq!(parse(explicit).into_record(), Ok(Record::default()));
        assert_eq!(parse("{}").into_record(), Ok(Record::default()));
    }

    /// Wider-than-the-record wire types earn their keep here: an offset
    /// that would have wrapped into a plausible i16 is refused instead.
    #[test]
    fn an_offset_past_i16_is_refused_rather_than_wrapped() {
        assert_eq!(
            parse(r#"{"offset_minutes":65896}"#).into_record(),
            Err(Invalid::Offset)
        );
    }

    #[test]
    fn a_full_document_becomes_the_record_it_describes() {
        let document = Document {
            offset_minutes: Some(-360),
            next_offset_minutes: Some(-300),
            transition_epoch_s: Some(u64::from(SPRING_FORWARD)),
            manual_offset_minutes: Some(330),
        };
        assert_eq!(
            document.into_record(),
            Ok(Record {
                manual_minutes: Some(330),
                ..seeded()
            })
        );
    }
}

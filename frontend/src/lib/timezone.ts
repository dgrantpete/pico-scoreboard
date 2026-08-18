/**
 * What timezone the scoreboard is in — answered by this browser.
 *
 * The device has no timezone database and nothing upstream knows what
 * timezone a living room is in. This page does: it is running on a phone or a
 * laptop in the same household, and `Date` carries the full IANA rules for
 * that household's zone.
 *
 * Posting the current offset alone would be wrong for half of every year, so
 * we post a *schedule* — the offset now, the instant it next changes, and the
 * offset after that. The device stores three numbers and flips at the
 * timestamp. See the firmware's `app/src/timezone.rs` for the other half.
 */

export interface OffsetSchedule {
	/** Minutes east of UTC right now. UTC−06:00 is -360. */
	offset_minutes: number;
	/** The offset after the next transition; null in a zone without DST. */
	next_offset_minutes: number | null;
	/** When the offset changes, in Unix seconds; null in a zone without DST. */
	transition_epoch_s: number | null;
}

const SECOND_MS = 1000;
const DAY_MS = 24 * 60 * 60 * SECOND_MS;

/**
 * How far apart the coarse probes are. Real zones put their transitions
 * months apart — the closest pair anywhere is Lord Howe Island's, six months —
 * so a week cannot step over one, and 58 probes is nothing.
 */
const PROBE_STEP_MS = 7 * DAY_MS;

/**
 * How far ahead to look. Just over a year, so a zone with DST always has a
 * transition inside the window no matter which month the page is opened in.
 */
const HORIZON_MS = 400 * DAY_MS;

/**
 * Minutes east of UTC at an instant, in this browser's zone.
 *
 * `getTimezoneOffset()` is minutes to ADD to local time to reach UTC, so it
 * runs the other way: 360 for UTC−06:00. Every value that leaves this module
 * is negated into the sign the firmware and the ISO world use.
 */
function offsetMinutesAt(ms: number): number {
	return -new Date(ms).getTimezoneOffset();
}

/**
 * The offset schedule for this browser's timezone.
 *
 * Coarse-probes forward a week at a time until the offset changes, then
 * bisects that week down to the millisecond. Transitions land on whole
 * minutes, so the bisected instant is exact rather than approximate.
 *
 * Costs about 60 `Date` constructions plus ~40 for the bisection — under a
 * millisecond, which is why it runs on every page load rather than being
 * cached anywhere.
 */
export function browserSchedule(now: Date = new Date()): OffsetSchedule {
	const start = now.getTime();
	const current = offsetMinutesAt(start);
	const flat: OffsetSchedule = {
		offset_minutes: current,
		next_offset_minutes: null,
		transition_epoch_s: null
	};

	// `lo` is always an instant known to still have the current offset.
	let lo = start;
	for (let probe = start + PROBE_STEP_MS; probe <= start + HORIZON_MS; probe += PROBE_STEP_MS) {
		if (offsetMinutesAt(probe) === current) {
			lo = probe;
			continue;
		}

		// The transition is in (lo, probe]. Bisect to the millisecond.
		let hi = probe;
		while (hi - lo > 1) {
			const mid = lo + Math.floor((hi - lo) / 2);
			if (offsetMinutesAt(mid) === current) lo = mid;
			else hi = mid;
		}

		return {
			offset_minutes: current,
			next_offset_minutes: offsetMinutesAt(hi),
			transition_epoch_s: Math.round(hi / SECOND_MS)
		};
	}

	// No change in a year: Arizona, Iceland, most of Asia. A flat offset needs
	// no refreshing, but the device stores it the same way.
	return flat;
}

/** `-360` → `"UTC−06:00"`. The minus is U+2212, to match the rest of the UI. */
export function formatOffset(minutes: number): string {
	const sign = minutes < 0 ? '−' : '+';
	const total = Math.abs(minutes);
	const hours = Math.floor(total / 60);
	const rest = total % 60;
	return `UTC${sign}${String(hours).padStart(2, '0')}:${String(rest).padStart(2, '0')}`;
}

/**
 * Every UTC offset IANA actually uses, for the manual-override picker.
 *
 * A list rather than a 15-minute sweep from −12:00 to +14:00: that would be
 * 105 entries, two thirds of which no place on earth uses, and the ones that
 * exist (Nepal's +05:45, the Chatham Islands' +12:45) are exactly the ones a
 * sweep at a coarser step would miss.
 *
 * It includes summer offsets as well as standard ones — Newfoundland's −02:30,
 * Chatham's +13:45 — because a manual override is a fixed offset, so somebody
 * overriding in a zone that observes DST needs to be able to pick whichever
 * half of the year they are in.
 */
export const OFFSET_CHOICES: number[] = [
	-720, -660, -600, -570, -540, -480, -420, -360, -300, -240, -210, -180, -150, -120, -60, 0, 60,
	120, 180, 210, 240, 270, 300, 330, 345, 360, 390, 420, 480, 525, 540, 570, 600, 630, 660, 720,
	765, 780, 825, 840
];

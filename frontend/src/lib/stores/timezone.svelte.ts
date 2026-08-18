import { picoApi, type TimezoneDocument } from '$lib/api';
import { browserSchedule } from '$lib/timezone';

/**
 * The device's timezone: seeded from this browser, overridable by hand.
 *
 * `seed()` runs on every page load, from the layout, so any visit to any page
 * refreshes the device's DST horizon. It is deliberately silent — a scoreboard
 * that cannot be reached is a problem the page is already showing somewhere
 * more useful than a toast about timezones.
 *
 * # Read before write, always
 *
 * PUT /api/timezone replaces the whole document, so seeding a schedule without
 * first reading the manual override would silently clear it. The GET is
 * therefore not an optimization — it is what makes the two halves of this
 * document independent while sharing one endpoint.
 *
 * The seed does NOT compare before posting. The firmware already skips the
 * flash write when nothing changed (that check has to live there — flash
 * discipline is the device's, and it has other clients), and duplicating it
 * here would give the two copies somewhere to disagree. The cost of not
 * duplicating it is one LAN round-trip per page load.
 */
function createTimezoneStore() {
	let document = $state<TimezoneDocument | null>(null);
	let isSaving = $state(false);
	let error = $state<string | null>(null);

	/** What this browser would post, over whatever override is already stored. */
	function seeded(current: TimezoneDocument | null): TimezoneDocument {
		return {
			...browserSchedule(),
			manual_offset_minutes: current?.manual_offset_minutes ?? null
		};
	}

	return {
		get document() {
			return document;
		},
		get isSaving() {
			return isSaving;
		},
		get error() {
			return error;
		},

		/** The offset the device says it is using, or null if it has none. */
		get effectiveOffset(): number | null {
			return document?.effective_offset_minutes ?? null;
		},

		/**
		 * Refresh the device's DST horizon from this browser. Fire-and-forget:
		 * a failure leaves `document` null and the timezone card hidden.
		 */
		async seed() {
			try {
				const current = await picoApi.getTimezone();
				document = await picoApi.setTimezone(seeded(current));
			} catch {
				// Silent by design — see the module docs.
			}
		},

		/**
		 * Set or clear the manual override, keeping the seeded schedule
		 * underneath it so that clearing restores the seeded answer without
		 * waiting for another page load.
		 */
		async setManualOffset(minutes: number | null) {
			isSaving = true;
			error = null;
			try {
				document = await picoApi.setTimezone({
					...seeded(document),
					manual_offset_minutes: minutes
				});
			} catch (e) {
				error = e instanceof Error ? e.message : 'Failed to save the timezone';
			} finally {
				isSaving = false;
			}
		},

		clearError() {
			error = null;
		}
	};
}

export const timezoneStore = createTimezoneStore();

<script lang="ts">
	import { timezoneStore } from "$lib/stores/timezone.svelte";
	import { OFFSET_CHOICES, formatOffset } from "$lib/timezone";

	// The seeded schedule, as the device is holding it. Distinct from the
	// override below: the override wins, but the schedule stays underneath it
	// so that switching the override off restores the seeded answer at once.
	let seeded = $derived(timezoneStore.document?.offset_minutes ?? null);
	let nextOffset = $derived(timezoneStore.document?.next_offset_minutes ?? null);
	let transitionAt = $derived(timezoneStore.document?.transition_epoch_s ?? null);
	let manual = $derived(timezoneStore.document?.manual_offset_minutes ?? null);

	let transitionLabel = $derived(
		transitionAt === null
			? null
			: new Date(transitionAt * 1000).toLocaleDateString(undefined, {
					year: "numeric",
					month: "short",
					day: "numeric",
				}),
	);

	function toggleOverride() {
		// Switching on starts from whatever the device is actually using, so
		// the first thing the picker shows is never a surprise.
		timezoneStore.setManualOffset(
			manual === null ? (timezoneStore.effectiveOffset ?? 0) : null,
		);
	}
</script>

<section class="card">
	<header class="card-header">
		<h3 class="card-title">Time Zone</h3>
		<p class="card-description">
			Used for game start times on the pregame screen
		</p>
	</header>
	<div class="card-content">
		{#if timezoneStore.document === null}
			<p class="hint">
				Reading the device's time zone…
			</p>
		{:else}
			<div class="field-group">
				<span class="label-text">Detected from this browser</span>
				<p class="hint">
					{#if seeded === null}
						Nothing seeded yet.
					{:else}
						{formatOffset(seeded)}{#if nextOffset !== null && transitionLabel}, changing to
							{formatOffset(nextOffset)} on {transitionLabel}{/if}.
					{/if}
					Refreshed automatically every time this page is opened — no need to
					save anything.
				</p>
			</div>

			<hr class="separator" />

			<div class="row-between">
				<div class="label-group">
					<span class="label-text">Set the time zone manually</span>
					<p class="text-sm text-muted">
						For a scoreboard that lives somewhere other than the device you are
						setting it up from. A manual offset is fixed — it does not follow
						daylight saving, so it needs changing twice a year in a zone that
						does.
					</p>
				</div>
				<label class="switch">
					<input
						type="checkbox"
						checked={manual !== null}
						disabled={timezoneStore.isSaving}
						onchange={toggleOverride}
					/>
					<span class="switch-track"><span class="switch-thumb"></span></span>
				</label>
			</div>

			{#if manual !== null}
				<div class="field-group nested">
					<label for="manual-offset">UTC Offset</label>
					<select
						id="manual-offset"
						value={manual}
						disabled={timezoneStore.isSaving}
						onchange={(e) =>
							timezoneStore.setManualOffset(
								Number((e.currentTarget as HTMLSelectElement).value),
							)}
					>
						{#each OFFSET_CHOICES as minutes}
							<option value={minutes}>{formatOffset(minutes)}</option>
						{/each}
					</select>
					<p class="hint">Applies to the display immediately.</p>
				</div>
			{/if}

			{#if timezoneStore.error}
				<div class="alert destructive" role="alert">
					<div class="row-between">
						<span>{timezoneStore.error}</span>
						<button class="btn ghost sm" onclick={() => timezoneStore.clearError()}>
							Dismiss
						</button>
					</div>
				</div>
			{/if}
		{/if}
	</div>
</section>

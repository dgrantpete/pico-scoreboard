<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import ColorRow from "$lib/components/ColorRow.svelte";
	import type { ColorsConfig } from "$lib/api/types";

	// Mirrors geometry::SCROLL_SPEEDS: only speeds that divide the panel's
	// 60 FPS refresh evenly stay smooth. 40 px/s was on this list when the
	// panel ran at 20 FPS and is not a divisor of 60 — a device that still has
	// it stored renders at the default instead, and `storedSpeedIsLegal` below
	// is what stops the form from quietly showing a value the panel is not using.
	const SCROLL_SPEED_OPTIONS = [
		{ value: 5, label: "5 px/s — Slowest" },
		{ value: 10, label: "10 px/s — Slow" },
		{ value: 20, label: "20 px/s — Steady" },
		{ value: 30, label: "30 px/s — Default" },
		{ value: 60, label: "60 px/s — Fastest" },
	];
	const DEFAULT_SCROLL_SPEED = 30;

	const COLOR_ROWS: { key: keyof ColorsConfig; label: string; description: string }[] = [
		{ key: "primary", label: "Primary", description: "Scores, period and status text, counts" },
		{ key: "secondary", label: "Secondary", description: "Venue text, dividers, subtle elements" },
		{ key: "accent", label: "Accent", description: "Highlights, final labels, start time" },
		{ key: "clock_normal", label: "Clock (Normal)", description: "Game clock when time remaining" },
		{ key: "clock_warning", label: "Clock (Warning)", description: "Low time warning, errors" },
	];
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	{@const storedSpeedIsLegal = SCROLL_SPEED_OPTIONS.some(
		(option) => option.value === config.display.scroll_speed_px_per_sec,
	)}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Appearance</h3>
			<p class="card-description">
				How text moves and what colors the LED matrix draws in. Applies right
				after saving.
			</p>
		</header>
		<div class="card-content">
			<div class="field-group">
				<label for="scroll-speed">Text Scroll Speed</label>
				<select
					id="scroll-speed"
					value={storedSpeedIsLegal
						? config.display.scroll_speed_px_per_sec
						: DEFAULT_SCROLL_SPEED}
					onchange={(e) =>
						settingsStore.updateDisplay(
							"scroll_speed_px_per_sec",
							Number((e.currentTarget as HTMLSelectElement).value),
						)}
				>
					{#each SCROLL_SPEED_OPTIONS as option (option.value)}
						<option value={option.value}>{option.label}</option>
					{/each}
				</select>
				<p class="hint">
					Play-by-play and goal-scorer text. Limited to speeds that stay
					smooth at the panel's 60 FPS.
				</p>
				{#if !storedSpeedIsLegal}
					<p class="hint">
						Saved as {config.display.scroll_speed_px_per_sec} px/s, which
						stutters at 60 FPS — the panel is drawing
						{DEFAULT_SCROLL_SPEED} px/s instead. Save to make it stick.
					</p>
				{/if}
			</div>

			{#each COLOR_ROWS as row (row.key)}
				<hr class="separator" />
				<ColorRow
					label={row.label}
					description={row.description}
					value={config.colors[row.key]}
					onchange={(color) => settingsStore.updateColors(row.key, color)}
				/>
			{/each}
		</div>
	</section>
{/if}

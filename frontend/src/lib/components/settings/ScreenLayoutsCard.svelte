<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import type { VariantsConfig } from "$lib/api/types";

	// Mirrors the variant tables in firmware scoreboard/screen_geometry.py.
	// Keys are per sport × screen so every sport's look can be tuned
	// independently. Applied live on save, so layouts can be compared on the
	// panel quickly. The option lists are shared while the sports still ship
	// the same designs; they fork alongside the firmware tables.
	// Pregame has no row: the "Big time" design was locked in for every
	// sport on 2026-07-15 and the other pregame variants were deleted.
	const FINAL_OPTIONS = [
		{ value: "A", label: "A — Marquee + boxscore" },
		{ value: "B", label: "B — Stacked ledger" },
		{ value: "C", label: "C — Line-score forward" },
	];
	const LAYOUTS: {
		key: keyof VariantsConfig;
		label: string;
		hint: string;
		options: { value: string; label: string }[];
	}[] = [
		{
			key: "mlb_final",
			label: "MLB Final",
			hint: "Baseball final screen with the line score",
			options: FINAL_OPTIONS,
		},
		{
			key: "nba_final",
			label: "NBA Final",
			hint: "Basketball final screen with the quarter line score",
			options: FINAL_OPTIONS,
		},
		{
			key: "soccer_live",
			label: "Soccer Live",
			hint: "Live match screen with the running clock",
			options: [
				{ value: "A", label: "A — Phase ledger" },
				{ value: "B", label: "B — Clock + phase" },
				{ value: "C", label: "C — Broadcast corners" },
			],
		},
	];

	function setVariant(key: keyof VariantsConfig, value: string) {
		const current = settingsStore.config?.display.variants;
		if (!current) return;
		settingsStore.updateDisplay("variants", { ...current, [key]: value });
	}

	// Mirrors screen_geometry._SCROLL_SPEEDS: only speeds that evenly divide
	// the panel's 20 FPS refresh stay smooth.
	const SCROLL_SPEED_OPTIONS = [
		{ value: 5, label: "5 px/s — Slowest" },
		{ value: 10, label: "10 px/s — Slow" },
		{ value: 20, label: "20 px/s — Default" },
		{ value: 40, label: "40 px/s — Fast" },
	];
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Screen Layouts</h3>
			<p class="card-description">
				Layout variant per screen. Applies right after saving — flip between
				them to compare on the panel.
			</p>
		</header>
		<div class="card-content">
			{#each LAYOUTS as layout, i (layout.key)}
				{#if i > 0}
					<hr class="separator" />
				{/if}
				<div class="field-group">
					<label for={"variant-" + layout.key}>{layout.label}</label>
					<select
						id={"variant-" + layout.key}
						value={config.display.variants[layout.key]}
						onchange={(e) =>
							setVariant(layout.key, (e.currentTarget as HTMLSelectElement).value)}
					>
						{#each layout.options as option}
							<option value={option.value}>{option.label}</option>
						{/each}
					</select>
					<p class="hint">{layout.hint}</p>
				</div>
			{/each}

			<hr class="separator" />

			<div class="row-between">
				<div class="label-group">
					<span class="label-text">Divider Lines</span>
					<p class="text-sm text-muted">
						Thin gray lines between sections on the game screens. Applies
						right after saving — toggle to compare on the panel.
					</p>
				</div>
				<label class="switch">
					<input
						type="checkbox"
						checked={config.display.show_dividers}
						onchange={() =>
							settingsStore.updateDisplay(
								"show_dividers",
								!settingsStore.config?.display.show_dividers,
							)}
					/>
					<span class="switch-track"><span class="switch-thumb"></span></span>
				</label>
			</div>

			<hr class="separator" />

			<div class="field-group">
				<label for="scroll-speed">Text Scroll Speed</label>
				<select
					id="scroll-speed"
					value={config.display.scroll_speed_px_per_sec}
					onchange={(e) =>
						settingsStore.updateDisplay(
							"scroll_speed_px_per_sec",
							Number((e.currentTarget as HTMLSelectElement).value),
						)}
				>
					{#each SCROLL_SPEED_OPTIONS as option}
						<option value={option.value}>{option.label}</option>
					{/each}
				</select>
				<p class="hint">
					Play-by-play and goal-scorer text. Limited to speeds that stay
					smooth at the panel's 20 FPS. Applies live.
				</p>
			</div>
		</div>
	</section>
{/if}

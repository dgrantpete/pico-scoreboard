<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import type { VariantsConfig } from "$lib/api/types";

	// Mirrors the variant tables in firmware scoreboard/screen_geometry.py.
	// Applied live on save, so layouts can be compared on the panel quickly.
	const LAYOUTS: {
		key: keyof VariantsConfig;
		label: string;
		hint: string;
		options: { value: string; label: string }[];
	}[] = [
		{
			key: "pregame",
			label: "Pregame",
			hint: "Upcoming-game screen (both sports)",
			options: [
				{ value: "A", label: "A — Cycling ledger" },
				{ value: "B", label: "B — All at once" },
				{ value: "C", label: "C — Big time" },
			],
		},
		{
			key: "final",
			label: "Baseball Final",
			hint: "MLB final screen with the line score",
			options: [
				{ value: "A", label: "A — Marquee + boxscore" },
				{ value: "B", label: "B — Stacked ledger" },
				{ value: "C", label: "C — Line-score forward" },
			],
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
		</div>
	</section>
{/if}

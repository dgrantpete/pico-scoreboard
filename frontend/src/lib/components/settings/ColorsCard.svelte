<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import ColorRow from "$lib/components/ColorRow.svelte";
	import type { ColorsConfig } from "$lib/api/types";

	const COLOR_ROWS: { key: keyof ColorsConfig; label: string; description: string }[] = [
		{ key: "primary", label: "Primary", description: "Dividers, status text, period display" },
		{ key: "secondary", label: "Secondary", description: "Venue text, subtle elements" },
		{ key: "accent", label: "Accent", description: "Highlights, start time" },
		{ key: "clock_normal", label: "Clock (Normal)", description: "Game clock when time remaining" },
		{ key: "clock_warning", label: "Clock (Warning)", description: "Low time warning, errors" },
	];
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Display Colors</h3>
			<p class="card-description">Customize UI colors on the LED matrix</p>
		</header>
		<div class="card-content">
			{#each COLOR_ROWS as row, i}
				{#if i > 0}
					<hr class="separator" />
				{/if}
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

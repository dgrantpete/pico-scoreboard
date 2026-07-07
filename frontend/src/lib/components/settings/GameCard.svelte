<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import NumberField from "$lib/components/NumberField.svelte";
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Games</h3>
			<p class="card-description">Score fetching and game rotation</p>
		</header>
		<div class="card-content">
			<NumberField
				id="poll-interval"
				label="Poll Interval (seconds)"
				hint="How often to fetch game updates from the API"
				min={1}
				value={config.display.poll_interval_seconds}
				validate={(v) => v < (settingsStore.config?.display.game_rotation_seconds ?? Infinity)}
				oncommit={(value) => settingsStore.updateDisplay("poll_interval_seconds", value)}
			/>

			<NumberField
				id="game-rotation"
				label="Game Rotation (seconds)"
				hint="How often to cycle between games when multiple are live"
				min={1}
				value={config.display.game_rotation_seconds}
				validate={(v) => v > (settingsStore.config?.display.poll_interval_seconds ?? 0)}
				oncommit={(value) => settingsStore.updateDisplay("game_rotation_seconds", value)}
			/>
		</div>
	</section>
{/if}

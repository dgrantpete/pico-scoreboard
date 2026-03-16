<script lang="ts">
	import { memoryTelemetryStore } from '$lib/stores/memory-telemetry.svelte';

	const percent = $derived(
		memoryTelemetryStore.latestStatus
			? (() => {
					const { memory_used, memory_free } = memoryTelemetryStore.latestStatus;
					const total = memory_used + memory_free;
					return total > 0 ? Math.round((memory_used / total) * 100) : 0;
				})()
			: 0
	);
</script>

<progress value={percent} max={100}></progress>

<style>
	progress {
		width: 100%;
		height: 0.5rem;
		border-radius: 9999px;
		overflow: hidden;
		appearance: none;

		&::-webkit-progress-bar {
			background: var(--secondary);
			border-radius: 9999px;
		}

		&::-webkit-progress-value {
			background: var(--primary);
			border-radius: 9999px;
			transition: width 0.3s ease;
		}
	}
</style>

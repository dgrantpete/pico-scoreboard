<script lang="ts">
	import Cpu from "@lucide/svelte/icons/cpu";
	import HardDrive from "@lucide/svelte/icons/hard-drive";
	import { settingsStore } from "$lib/stores/settings.svelte";

	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
	}

	function calcPercent(used: number, free: number): number {
		const total = used + free;
		if (total === 0) return 0;
		return Math.round((used / total) * 100);
	}

	function flashColor(percent: number): string {
		if (percent >= 90) return "var(--color-crit)";
		if (percent >= 70) return "var(--color-warn)";
		return "var(--color-ok)";
	}
</script>

{#if settingsStore.status}
	{@const status = settingsStore.status}
	{@const memPercent = calcPercent(status.memory_used, status.memory_free)}
	{@const flashPercent = calcPercent(status.flash_used, status.flash_free)}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">System Resources</h3>
			<p class="card-description">Memory and storage usage</p>
		</header>
		<div class="card-content gap-lg">
			<!-- Memory: deliberately NOT colored by percentage. MicroPython's GC
			     lets the heap fill before collecting, so the number sweeps a
			     sawtooth and high readings are normal, not alarming. -->
			<div class="field-group">
				<div class="row-between">
					<div class="row-center">
						<Cpu class="icon-muted" />
						<span class="text-sm-medium">Memory</span>
					</div>
					<span class="text-sm-medium text-muted">{memPercent}%</span>
				</div>
				<progress value={memPercent} max={100}></progress>
				<div class="row-between text-xs text-muted">
					<span>{formatBytes(status.memory_used)} used</span>
					<span>{formatBytes(status.memory_free)} free</span>
				</div>
				<p class="hint">
					Instantaneous reading — the heap fills between garbage collections
					by design, so high peaks are normal.
				</p>
			</div>

			<hr class="separator" />

			<div class="field-group">
				<div class="row-between">
					<div class="row-center">
						<HardDrive class="icon-muted" />
						<span class="text-sm-medium">Flash Storage</span>
					</div>
					<span class="text-sm-medium" style="color: {flashColor(flashPercent)}">
						{flashPercent}%
					</span>
				</div>
				<progress value={flashPercent} max={100}></progress>
				<div class="row-between text-xs text-muted">
					<span>{formatBytes(status.flash_used)} used</span>
					<span>{formatBytes(status.flash_free)} free</span>
				</div>
			</div>
		</div>
	</section>
{/if}

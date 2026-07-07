<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import NumberField from "$lib/components/NumberField.svelte";
	import type { LogLevel } from "$lib/api/types";

	const LOG_LEVEL_OPTIONS: { value: LogLevel; label: string }[] = [
		{ value: "debug", label: "Debug (all activity)" },
		{ value: "error", label: "Errors only" },
		{ value: "none", label: "Off" },
	];
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Advanced</h3>
			<p class="card-description">Server, logging, and recovery configuration</p>
		</header>
		<div class="card-content">
			<NumberField
				id="cache-max-age"
				label="Cache Max Age (seconds)"
				hint="HTTP cache duration for static content (0 = no caching)"
				min={0}
				value={config.server.cache_max_age_seconds}
				oncommit={(value) => settingsStore.updateServer("cache_max_age_seconds", value)}
			/>

			<hr class="separator" />

			<div class="field-group">
				<label for="log-level">Log Level</label>
				<select
					id="log-level"
					value={config.log.level}
					onchange={(e) =>
						settingsStore.updateLog("level", (e.currentTarget as HTMLSelectElement).value as LogLevel)}
				>
					{#each LOG_LEVEL_OPTIONS as option}
						<option value={option.value}>{option.label}</option>
					{/each}
				</select>
				<p class="hint">
					Device log verbosity (see the Logs page). Applies immediately — no
					reboot needed.
				</p>
			</div>

			<hr class="separator" />

			<div class="row-between">
				<div class="label-group">
					<span class="label-text">Hardware Watchdog</span>
					<p class="text-sm text-muted">
						Auto-reboot if the firmware wedges. Leave off while developing
						over USB — an armed watchdog reboots shortly after the script
						is interrupted. Takes effect after a reboot.
					</p>
				</div>
				<label class="switch">
					<input
						type="checkbox"
						checked={config.watchdog.enabled}
						onchange={() =>
							settingsStore.updateWatchdog("enabled", !settingsStore.config?.watchdog.enabled)}
					/>
					<span class="switch-track"><span class="switch-thumb"></span></span>
				</label>
			</div>

			{#if config.watchdog.enabled}
				<NumberField
					id="watchdog-timeout"
					label="Watchdog Timeout (ms)"
					hint="Reboot after this long without a healthy heartbeat (hardware max ~8300)"
					min={2000}
					max={8300}
					step={100}
					value={config.watchdog.timeout_ms}
					oncommit={(value) => settingsStore.updateWatchdog("timeout_ms", value)}
				/>
			{/if}
		</div>
	</section>
{/if}

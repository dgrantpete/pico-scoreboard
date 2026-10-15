<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import NumberField from "$lib/components/NumberField.svelte";
	import type { LogLevel } from "$lib/api/types";

	const LOG_LEVEL_OPTIONS: { value: LogLevel; label: string }[] = [
		{ value: "debug", label: "Debug (all activity)" },
		{ value: "error", label: "Errors only" },
		{ value: "none", label: "Off" },
	];

	// The device says whether the watchdog switch means anything on it: on an
	// over-the-air unit the watchdog and its recovery path are always on. Until
	// status arrives this is false and the switch shows, which is the older
	// and bench behaviour.
	const watchdogAlwaysOn = $derived(settingsStore.status?.watchdog_always_on === true);
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
					<span class="label-text">Automatic Updates</span>
					<p class="text-sm text-muted">
						Check daily for scoreboard app updates and install them
						automatically. The device restarts briefly when an update is
						applied.
					</p>
				</div>
				<label class="switch">
					<input
						type="checkbox"
						checked={config.ota.enabled}
						onchange={() =>
							settingsStore.updateOta("enabled", !settingsStore.config?.ota.enabled)}
					/>
					<span class="switch-track"><span class="switch-thumb"></span></span>
				</label>
			</div>

			<hr class="separator" />

			{#if watchdogAlwaysOn}
				<!-- No switch: on a scoreboard that updates itself the device
				     ignores watchdog.enabled, and a control that does nothing is
				     worse than none. The timeout still applies, so it stays. -->
				<div class="label-group">
					<span class="label-text">Hardware Watchdog</span>
					<p class="text-sm text-muted">
						Always on. If the display freezes, or the scoreboard stops
						hearing back from the internet for three poll intervals, it
						restarts itself and reconnects. Scoreboards that update
						themselves keep this on — there is nobody nearby to restart one
						by hand.
					</p>
				</div>
			{:else}
				<div class="row-between">
					<div class="label-group">
						<span class="label-text">Hardware Watchdog</span>
						<p class="text-sm text-muted">
							Restart automatically if the display freezes or the network
							connection is lost. Leave off while debugging — an armed
							watchdog resets a core halted at a breakpoint a few seconds
							later. Takes effect after a reboot.
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
			{/if}

			{#if watchdogAlwaysOn || config.watchdog.enabled}
				<NumberField
					id="watchdog-timeout"
					label="Watchdog Timeout (ms)"
					hint="Restart after this long without a healthy check-in (hardware max ~8300). Takes effect after a reboot."
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

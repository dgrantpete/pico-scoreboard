<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import Save from "@lucide/svelte/icons/save";
	import Eye from "@lucide/svelte/icons/eye";
	import EyeOff from "@lucide/svelte/icons/eye-off";
	import RotateCcw from "@lucide/svelte/icons/rotate-ccw";
	import Wifi from "@lucide/svelte/icons/wifi";
	import WifiOff from "@lucide/svelte/icons/wifi-off";
	import RefreshCw from "@lucide/svelte/icons/refresh-cw";
	import Cpu from "@lucide/svelte/icons/cpu";
	import HardDrive from "@lucide/svelte/icons/hard-drive";
	import { settingsStore } from "$lib/stores/settings.svelte";
	import { rebootStore } from "$lib/stores/reboot.svelte";
	import { memoryTelemetryStore } from "$lib/stores/memory-telemetry.svelte";
	import { picoApi } from "$lib/api";
	import RebootOverlay from "$lib/components/RebootOverlay.svelte";
	import MemoryChart from "$lib/components/MemoryChart.svelte";
	import type { NetworkStatus, Config, Color, GammaConfig } from "$lib/api/types";

	// Password visibility toggles
	let showWifiPassword = $state(false);
	let showApiKey = $state(false);

	// Reset network dialog state
	let showResetDialog = $state(false);

	// Dialog element refs
	let rebootDialog: HTMLDialogElement;
	let resetDialog: HTMLDialogElement;

	// Local binding for brightness slider
	let brightnessValue = $derived(
		settingsStore.config ? settingsStore.config.display.brightness : 100,
	);

	// Local state for frequency slider (prevents lag during dragging)
	let freqSliderValue = $state<number | null>(null);
	let freqDisplayKhz = $derived(
		freqSliderValue !== null
			? sliderToFreq(freqSliderValue)
			: (settingsStore.config?.display.data_frequency_khz ?? 20000)
	);

	// Status refresh interval
	let refreshInterval: ReturnType<typeof setInterval> | null = null;

	// Memory telemetry helpers
	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
	}

	// Logarithmic slider helpers for data frequency (2 kHz to 50 MHz)
	const FREQ_MIN = 2; // kHz
	const FREQ_MAX = 50000; // kHz

	function freqToSlider(freqKhz: number): number {
		// Convert frequency to 0-100 slider position (logarithmic)
		return (100 * Math.log(freqKhz / FREQ_MIN)) / Math.log(FREQ_MAX / FREQ_MIN);
	}

	function sliderToFreq(sliderValue: number): number {
		// Convert 0-100 slider position to frequency (logarithmic)
		return Math.round(
			FREQ_MIN * Math.pow(FREQ_MAX / FREQ_MIN, sliderValue / 100),
		);
	}

	function formatFrequency(freqKhz: number): string {
		if (freqKhz >= 1000) {
			return `${(freqKhz / 1000).toFixed(freqKhz % 1000 === 0 ? 0 : 1)} MHz`;
		}
		return `${freqKhz} kHz`;
	}

	function calcPercent(used: number, free: number): number {
		const total = used + free;
		if (total === 0) return 0;
		return Math.round((used / total) * 100);
	}

	function getUsageColor(percent: number): string {
		if (percent >= 90) return 'oklch(0.637 0.237 25.331)';
		if (percent >= 70) return 'oklch(0.769 0.188 70.08)';
		return 'oklch(0.723 0.219 149.579)';
	}

	// Color conversion helpers
	function rgbToHex(color: Color): string {
		const toHex = (n: number) => n.toString(16).padStart(2, "0");
		return `#${toHex(color.r)}${toHex(color.g)}${toHex(color.b)}`;
	}

	function hexToRgb(hex: string): Color {
		const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
		return result
			? {
					r: parseInt(result[1], 16),
					g: parseInt(result[2], 16),
					b: parseInt(result[3], 16),
				}
			: { r: 255, g: 255, b: 255 };
	}

	// Gamma type options for the Select dropdown
	const GAMMA_TYPE_OPTIONS = [
		{ value: "srgb", label: "sRGB" },
		{ value: "power", label: "Power" },
		{ value: "none", label: "None (Linear)" },
	] as const;

	function gammaTypeLabel(config: GammaConfig): string {
		return GAMMA_TYPE_OPTIONS.find((o) => o.value === config.type)?.label ?? config.type;
	}

	function handleGammaTypeChange(newType: string) {
		if (newType === "power") {
			settingsStore.updateDisplay("gamma", { type: "power", value: 2.2 });
		} else if (newType === "none") {
			settingsStore.updateDisplay("gamma", { type: "none" });
		} else {
			settingsStore.updateDisplay("gamma", { type: "srgb" });
		}
	}

	function handleGammaPowerValueChange(value: number) {
		settingsStore.updateDisplay("gamma", {
			type: "power",
			value: Math.round(value * 10) / 10,
		});
	}

	// Manage dialog open/close via $effect
	$effect(() => {
		if (!rebootDialog) return;
		if (settingsStore.showRebootPrompt && !rebootDialog.open) rebootDialog.showModal();
		else if (!settingsStore.showRebootPrompt && rebootDialog.open) rebootDialog.close();
	});

	$effect(() => {
		if (!resetDialog) return;
		if (showResetDialog && !resetDialog.open) resetDialog.showModal();
		else if (!showResetDialog && resetDialog.open) resetDialog.close();
	});

	onMount(() => {
		settingsStore.load().then(() => {
			// Seed memory telemetry with initial status and start polling
			if (settingsStore.status) {
				memoryTelemetryStore.seedFromStatus(settingsStore.status);
			}
			memoryTelemetryStore.startPolling();
		});

		// Start status refresh interval (15 seconds) for other status data
		refreshInterval = setInterval(() => {
			settingsStore.refreshStatus();
		}, 15000);
	});

	onDestroy(() => {
		if (refreshInterval) {
			clearInterval(refreshInterval);
			refreshInterval = null;
		}
		memoryTelemetryStore.stopPolling();
	});

	function handleBrightnessChange(value: number) {
		settingsStore.updateDisplay("brightness", value);
	}

	async function handleResetNetwork() {
		showResetDialog = false;
		await picoApi.resetNetwork();
		// Use explicit 'network_reset' scenario since we know device will enter AP mode
		await rebootStore.initiateRebootWithScenario(
			settingsStore.status as NetworkStatus,
			settingsStore.config as Config,
			'network_reset'
		);
	}
</script>

<div class="settings-page">
	<div>
		<h2 class="page-title">Settings</h2>
		<p class="page-description">Configure your Pi Pico scoreboard</p>
	</div>

	{#if settingsStore.isLoading}
		<!-- Loading skeleton -->
		<div class="card-stack">
			{#each { length: 4 } as _}
				<section class="card">
					<header class="card-header">
						<div class="skeleton" style="height: 1.5rem; width: 8rem;"></div>
						<div class="skeleton" style="height: 1rem; width: 12rem;"></div>
					</header>
					<div class="card-content">
						{#each { length: 2 } as _}
							<div class="field-group">
								<div class="skeleton" style="height: 1rem; width: 6rem;"></div>
								<div class="skeleton" style="height: 2.25rem; width: 100%;"></div>
							</div>
						{/each}
					</div>
				</section>
			{/each}
		</div>
	{:else if settingsStore.error && !settingsStore.config}
		<!-- Error state -->
		<section class="card border-destructive">
			<header class="card-header">
				<h3 class="card-title text-destructive">Connection Error</h3>
				<p class="card-description">{settingsStore.error}</p>
			</header>
			<div class="card-content">
				<button class="btn default" onclick={() => settingsStore.load()}>
					<RefreshCw />
					Retry
				</button>
			</div>
		</section>
	{:else if settingsStore.config}
		<!-- Device Status -->
		<section class="card" class:border-warning={settingsStore.status?.setup_mode}>
			<header class="card-header">
				<h3 class="card-title">
					{#if settingsStore.status?.mode === "station" && settingsStore.status?.connected}
						<Wifi class="icon-green" />
						Connected to WiFi
					{:else if settingsStore.status?.setup_mode && settingsStore.status?.setup_reason === "connection_failed"}
						<WifiOff class="icon-amber" />
						Connection Failed
					{:else if settingsStore.status?.setup_mode}
						<WifiOff class="icon-muted" />
						Network Not Configured
					{:else}
						<WifiOff class="icon-muted" />
						Not Connected
					{/if}
				</h3>
				<p class="card-description">
					{#if settingsStore.status?.setup_mode && settingsStore.status?.setup_reason === "connection_failed"}
						Could not connect to "{settingsStore.status.configured_ssid}"
					{:else if settingsStore.status?.setup_mode}
						WiFi setup is required to fetch scores
					{:else if settingsStore.status?.connected}
						{settingsStore.status.ip} &bull; {settingsStore.status.hostname}
					{:else}
						Current network connection status
					{/if}
				</p>
			</header>
			<div class="card-content">
				{#if settingsStore.status?.setup_mode}
					<a href="#/setup" class="btn default">Complete Setup</a>
				{:else if settingsStore.status}
					<div class="status-grid">
						<div>
							<span class="text-muted">Mode:</span>
							<span class="status-value capitalize">{settingsStore.status.mode}</span>
						</div>
						<div>
							<span class="text-muted">Connected:</span>
							<span class="status-value">{settingsStore.status.connected ? "Yes" : "No"}</span>
						</div>
						{#if settingsStore.status.mode === "station" && settingsStore.status.ip}
							<div>
								<span class="text-muted">IP Address:</span>
								<span class="status-value">{settingsStore.status.ip}</span>
							</div>
							<div>
								<span class="text-muted">Hostname:</span>
								<span class="status-value">{settingsStore.status.hostname}</span>
							</div>
						{:else if settingsStore.status.mode === "ap"}
							<div>
								<span class="text-muted">AP Network:</span>
								<span class="status-value">{settingsStore.status.ap_ssid}</span>
							</div>
							<div>
								<span class="text-muted">AP IP:</span>
								<span class="status-value">{settingsStore.status.ap_ip}</span>
							</div>
						{/if}
					</div>
				{:else}
					<p class="text-sm text-muted">Status unavailable</p>
				{/if}
			</div>
		</section>

		<!-- System Resources -->
		{#if settingsStore.status}
			{@const flashPercent = calcPercent(
				settingsStore.status.flash_used,
				settingsStore.status.flash_free
			)}
			<section class="card">
				<header class="card-header">
					<h3 class="card-title">System Resources</h3>
					<p class="card-description">Memory and storage usage over time</p>
				</header>
				<div class="card-content gap-lg">
					<!-- Memory Usage Chart -->
					<div class="field-group">
						<div class="row-between">
							<div class="row-center">
								<Cpu class="icon-muted" />
								<span class="text-sm-medium">Memory</span>
							</div>
							{#if memoryTelemetryStore.latestStatus}
								{@const memPercent = calcPercent(
									memoryTelemetryStore.latestStatus.memory_used,
									memoryTelemetryStore.latestStatus.memory_free
								)}
								<span class="text-sm-medium" style="color: {getUsageColor(memPercent)}">
									{memPercent}%
								</span>
							{/if}
						</div>
						<MemoryChart />
						{#if memoryTelemetryStore.latestStatus}
							<div class="row-between text-xs text-muted">
								<span>{formatBytes(memoryTelemetryStore.latestStatus.memory_used)} used</span>
								<span>{formatBytes(memoryTelemetryStore.latestStatus.memory_free)} free</span>
							</div>
						{/if}
					</div>

					<hr class="separator" />

					<!-- Flash Storage Usage -->
					<div class="field-group">
						<div class="row-between">
							<div class="row-center">
								<HardDrive class="icon-muted" />
								<span class="text-sm-medium">Flash Storage</span>
							</div>
							<span class="text-sm-medium" style="color: {getUsageColor(flashPercent)}">
								{flashPercent}%
							</span>
						</div>
						<progress value={flashPercent} max={100}></progress>
						<div class="row-between text-xs text-muted">
							<span>{formatBytes(settingsStore.status.flash_used)} used</span>
							<span>{formatBytes(settingsStore.status.flash_free)} free</span>
						</div>
					</div>
				</div>
			</section>
		{/if}

		<!-- Network Configuration -->
		<section class="card">
			<header class="card-header">
				<h3 class="card-title">Network</h3>
				<p class="card-description">WiFi connection settings</p>
			</header>
			<div class="card-content">
				<!-- WiFi Settings -->
				<div class="field-group">
					<label for="wifi-ssid">WiFi Network (SSID)</label>
					<input
						id="wifi-ssid"
						type="text"
						placeholder="Enter network name"
						value={settingsStore.config.network.ssid}
						oninput={(e) =>
							settingsStore.updateNetwork(
								"ssid",
								(e.target as HTMLInputElement).value,
							)}
					/>
				</div>
				<div class="field-group">
					<label for="wifi-password">WiFi Password</label>
					<div class="input-wrapper">
						<input
							id="wifi-password"
							class="input-with-toggle"
							type={showWifiPassword ? "text" : "password"}
							placeholder="Enter password"
							value={settingsStore.config.network.password}
							oninput={(e) =>
								settingsStore.updateNetwork(
									"password",
									(e.target as HTMLInputElement).value,
								)}
						/>
						<button
							class="btn ghost sm toggle-btn"
							onclick={() => (showWifiPassword = !showWifiPassword)}
						>
							{#if showWifiPassword}
								<EyeOff class="icon-muted" />
							{:else}
								<Eye class="icon-muted" />
							{/if}
						</button>
					</div>
				</div>

				<hr class="separator" />

				<div class="field-group">
					<label for="device-name">Device Name</label>
					<input
						id="device-name"
						type="text"
						placeholder="scoreboard"
						value={settingsStore.config.network.device_name}
						oninput={(e) =>
							settingsStore.updateNetwork(
								"device_name",
								(e.target as HTMLInputElement).value,
							)}
					/>
					<p class="hint">
						Access the device at {settingsStore.config.network.device_name}.local
					</p>
				</div>

				<div class="field-group">
					<label for="connect-timeout">Connection Timeout (seconds)</label>
					<input
						id="connect-timeout"
						type="number"
						min="1"
						value={settingsStore.config.network.connect_timeout_seconds}
					onchange={(e) => {
						const input = e.target as HTMLInputElement;
						const value = parseInt(input.value);
						if (!isNaN(value) && value >= 1) {
							settingsStore.updateNetwork("connect_timeout_seconds", value);
						} else {
							input.value = String(settingsStore.config?.network.connect_timeout_seconds ?? "");
						}
					}}
					/>
					<p class="hint">
						Time to wait before falling back to setup mode
					</p>
				</div>

				<hr class="separator" />

				<!-- Reset Network -->
				<div class="row-between">
					<div class="label-group">
						<label>Reset Network</label>
						<p class="text-sm text-muted">
							Clear WiFi credentials and return to setup mode
						</p>
					</div>
					<button
						class="btn destructive"
						onclick={() => (showResetDialog = true)}
						disabled={rebootStore.isActive}
					>
						Reset Network
					</button>
				</div>
			</div>
		</section>

		<!-- Backend API Configuration -->
		<section class="card">
			<header class="card-header">
				<h3 class="card-title">Backend API</h3>
				<p class="card-description">Connection settings for the scores API</p>
			</header>
			<div class="card-content">
				<div class="field-group">
					<label for="api-url">API URL</label>
					<input
						id="api-url"
						type="url"
						placeholder="https://api.example.com"
						value={settingsStore.config.api.url}
						oninput={(e) =>
							settingsStore.updateApi(
								"url",
								(e.target as HTMLInputElement).value,
							)}
					/>
				</div>
				<div class="field-group">
					<label for="api-key">API Key</label>
					<div class="input-wrapper">
						<input
							id="api-key"
							class="input-with-toggle"
							type={showApiKey ? "text" : "password"}
							placeholder="Enter API key"
							value={settingsStore.config.api.key}
							oninput={(e) =>
								settingsStore.updateApi(
									"key",
									(e.target as HTMLInputElement).value,
								)}
						/>
						<button
							class="btn ghost sm toggle-btn"
							onclick={() => (showApiKey = !showApiKey)}
						>
							{#if showApiKey}
								<EyeOff class="icon-muted" />
							{:else}
								<Eye class="icon-muted" />
							{/if}
						</button>
					</div>
				</div>

				<hr class="separator" />

				<div class="row-between">
					<div class="label-group">
						<label>Mock Mode</label>
						<p class="text-sm text-muted">
							Use procedurally generated test data instead of live ESPN data
						</p>
					</div>
					<label class="switch">
						<input type="checkbox" checked={settingsStore.config?.api.mock} onchange={() => settingsStore.updateApi("mock", !settingsStore.config?.api.mock)} />
						<span class="switch-track"><span class="switch-thumb"></span></span>
					</label>
				</div>
			</div>
		</section>

		<!-- Display Settings -->
		<section class="card">
			<header class="card-header">
				<h3 class="card-title">Display</h3>
				<p class="card-description">LED matrix brightness and refresh settings</p>
			</header>
			<div class="card-content gap-lg">
				<div class="field-group">
					<div class="row-between">
						<label>Brightness</label>
						<span class="text-sm text-muted">{settingsStore.config.display.brightness}%</span>
					</div>
					<input
						type="range"
						value={brightnessValue}
						oninput={(e) => handleBrightnessChange((e.currentTarget as HTMLInputElement).valueAsNumber)}
						max={100}
						step={1}
					/>
				</div>

				<hr class="separator" />

				<div class="field-group">
					<label for="poll-interval">Poll Interval (seconds)</label>
					<input
						id="poll-interval"
						type="number"
						min="1"
						value={settingsStore.config.display.poll_interval_seconds}
					onchange={(e) => {
						const input = e.target as HTMLInputElement;
						const value = parseInt(input.value);
						if (!isNaN(value) && value >= 1) {
							settingsStore.updateDisplay("poll_interval_seconds", value);
						} else {
							input.value = String(settingsStore.config?.display.poll_interval_seconds ?? "");
						}
					}}
					/>
					<p class="hint">
						How often to fetch game updates from the API
					</p>
				</div>

				<hr class="separator" />

				<!-- Data Frequency (logarithmic scale) -->
				<div class="field-group">
					<div class="row-between">
						<label>Data Frequency</label>
						<span class="text-sm text-muted">
							{formatFrequency(freqDisplayKhz)}
						</span>
					</div>
					<input
						type="range"
						value={freqSliderValue ?? freqToSlider(settingsStore.config.display.data_frequency_khz)}
						oninput={(e) => {
							freqSliderValue = (e.currentTarget as HTMLInputElement).valueAsNumber;
						}}
						onchange={(e) => {
							settingsStore.updateDisplay("data_frequency_khz", sliderToFreq((e.currentTarget as HTMLInputElement).valueAsNumber));
							freqSliderValue = null;
						}}
						min={0}
						max={100}
						step={0.1}
					/>
					<p class="hint">
						LED matrix data clock speed. Very low values allow observing bitplane scanning.
					</p>
				</div>

				<hr class="separator" />

				<!-- Target Refresh Rate -->
				<div class="field-group">
					<div class="row-between">
						<label>Refresh Rate</label>
						<span class="text-sm text-muted">
							{settingsStore.config.display.target_refresh_rate} Hz
						</span>
					</div>
					<input
						type="range"
						value={settingsStore.config.display.target_refresh_rate}
						oninput={(e) =>
							settingsStore.updateDisplay("target_refresh_rate", (e.currentTarget as HTMLInputElement).valueAsNumber)}
						min={30}
						max={240}
						step={1}
					/>
					<p class="hint">
						Target display refresh rate. Lower values save power but may cause flicker.
					</p>
				</div>

				<hr class="separator" />

				<!-- Gamma -->
				<div class="field-group">
					<div class="row-between">
						<label>Gamma Correction</label>
						<span class="text-sm text-muted">
							{#if settingsStore.config.display.gamma.type === "power"}
								Power ({settingsStore.config.display.gamma.value.toFixed(1)})
							{:else}
								{gammaTypeLabel(settingsStore.config.display.gamma)}
							{/if}
						</span>
					</div>
					<select
						value={settingsStore.config.display.gamma.type}
						onchange={(e) => handleGammaTypeChange((e.currentTarget as HTMLSelectElement).value)}
					>
						{#each GAMMA_TYPE_OPTIONS as option}
							<option value={option.value}>{option.label}</option>
						{/each}
					</select>
					{#if settingsStore.config.display.gamma.type === "power"}
						<div class="field-group nested">
							<div class="row-between">
								<label class="text-xs">Power Value</label>
								<span class="text-sm text-muted">
									{settingsStore.config.display.gamma.value.toFixed(1)}
								</span>
							</div>
							<input
								type="range"
								value={settingsStore.config.display.gamma.value}
								oninput={(e) => handleGammaPowerValueChange((e.currentTarget as HTMLInputElement).valueAsNumber)}
								min={1.0}
								max={3.0}
								step={0.1}
							/>
						</div>
					{/if}
					<p class="hint">
						{#if settingsStore.config.display.gamma.type === "srgb"}
							sRGB gamma with linear region. Best match for most content.
						{:else if settingsStore.config.display.gamma.type === "power"}
							Simple power function. 2.2 approximates sRGB.
						{:else}
							No gamma correction. Raw linear values sent to display.
						{/if}
					</p>
				</div>

				<hr class="separator" />

				<!-- Dead Time (Blanking Time) -->
				<div class="field-group">
					<div class="row-between">
						<label>Dead Time</label>
						<span class="text-sm text-muted">
							{settingsStore.config.display.blanking_time_ns} ns
						</span>
					</div>
					<input
						type="range"
						value={settingsStore.config.display.blanking_time_ns}
						oninput={(e) =>
							settingsStore.updateDisplay("blanking_time_ns", (e.currentTarget as HTMLInputElement).valueAsNumber)}
						min={0}
						max={3000}
						step={10}
					/>
					<p class="hint">
						Output-enable blanking time. Reduces ghosting but dims the display.
					</p>
				</div>
			</div>
		</section>

		<!-- Display Colors -->
		<section class="card">
			<header class="card-header">
				<h3 class="card-title">Display Colors</h3>
				<p class="card-description">Customize UI colors on the LED matrix</p>
			</header>
			<div class="card-content">
				<div class="row-between">
					<div class="label-group">
						<label>Primary</label>
						<p class="text-sm text-muted">Dividers, status text, period display</p>
					</div>
					<input
						type="color"
						value={rgbToHex(settingsStore.config.colors.primary)}
						oninput={(e) =>
							settingsStore.updateColors(
								"primary",
								hexToRgb((e.target as HTMLInputElement).value),
							)}
						class="color-picker"
					/>
				</div>

				<hr class="separator" />

				<div class="row-between">
					<div class="label-group">
						<label>Secondary</label>
						<p class="text-sm text-muted">Venue text, subtle elements</p>
					</div>
					<input
						type="color"
						value={rgbToHex(settingsStore.config.colors.secondary)}
						oninput={(e) =>
							settingsStore.updateColors(
								"secondary",
								hexToRgb((e.target as HTMLInputElement).value),
							)}
						class="color-picker"
					/>
				</div>

				<hr class="separator" />

				<div class="row-between">
					<div class="label-group">
						<label>Accent</label>
						<p class="text-sm text-muted">Highlights, start time</p>
					</div>
					<input
						type="color"
						value={rgbToHex(settingsStore.config.colors.accent)}
						oninput={(e) =>
							settingsStore.updateColors(
								"accent",
								hexToRgb((e.target as HTMLInputElement).value),
							)}
						class="color-picker"
					/>
				</div>

				<hr class="separator" />

				<div class="row-between">
					<div class="label-group">
						<label>Clock (Normal)</label>
						<p class="text-sm text-muted">Game clock when time remaining</p>
					</div>
					<input
						type="color"
						value={rgbToHex(settingsStore.config.colors.clock_normal)}
						oninput={(e) =>
							settingsStore.updateColors(
								"clock_normal",
								hexToRgb((e.target as HTMLInputElement).value),
							)}
						class="color-picker"
					/>
				</div>

				<hr class="separator" />

				<div class="row-between">
					<div class="label-group">
						<label>Clock (Warning)</label>
						<p class="text-sm text-muted">Low time warning, errors</p>
					</div>
					<input
						type="color"
						value={rgbToHex(settingsStore.config.colors.clock_warning)}
						oninput={(e) =>
							settingsStore.updateColors(
								"clock_warning",
								hexToRgb((e.target as HTMLInputElement).value),
							)}
						class="color-picker"
					/>
				</div>
			</div>
		</section>

		<!-- Advanced Settings -->
		<section class="card">
			<header class="card-header">
				<h3 class="card-title">Advanced</h3>
				<p class="card-description">Server and caching configuration</p>
			</header>
			<div class="card-content">
				<div class="field-group">
					<label for="cache-max-age">Cache Max Age (seconds)</label>
					<input
						id="cache-max-age"
						type="number"
						min="0"
						value={settingsStore.config.server.cache_max_age_seconds}
					onchange={(e) => {
						const input = e.target as HTMLInputElement;
						const value = parseInt(input.value);
						if (!isNaN(value) && value >= 0) {
							settingsStore.updateServer("cache_max_age_seconds", value);
						} else {
							input.value = String(settingsStore.config?.server.cache_max_age_seconds ?? "");
						}
					}}
					/>
					<p class="hint">
						HTTP cache duration for static content (0 = no caching)
					</p>
				</div>
			</div>
		</section>

		<!-- Error banner -->
		{#if settingsStore.error}
			<div class="alert destructive" role="alert">
				<div class="row-between">
					<span>{settingsStore.error}</span>
					<button
						class="btn ghost sm"
						onclick={() => settingsStore.clearError()}
					>
						Dismiss
					</button>
				</div>
			</div>
		{/if}

		<!-- Action Buttons -->
		<div class="action-bar">
			<button
				class="btn outline"
				onclick={() => settingsStore.reboot()}
				disabled={rebootStore.isActive}
			>
				<RotateCcw />
				Reboot Device
			</button>

			<div class="action-group">
				<button
					class="btn outline"
					onclick={() => settingsStore.discard()}
					disabled={!settingsStore.isDirty || settingsStore.isSaving}
				>
					Discard
				</button>
				<button
					class="btn default"
					onclick={() => settingsStore.save()}
					disabled={!settingsStore.isDirty || settingsStore.isSaving}
				>
					<Save />
					{settingsStore.isSaving ? "Saving..." : "Save Changes"}
				</button>
			</div>
		</div>
	{/if}
</div>

<!-- Reboot Prompt Dialog -->
<dialog bind:this={rebootDialog} class="dialog" onclose={() => settingsStore.dismissRebootPrompt()}>
	<h2>Network Settings Changed</h2>
	<p>
		Network configuration has been updated. A reboot is required for changes
		to take effect. Would you like to reboot now?
	</p>
	<footer class="dialog-footer">
		<button class="btn outline" onclick={() => settingsStore.dismissRebootPrompt()}>Later</button>
		<button class="btn default" onclick={() => settingsStore.reboot()}>Reboot Now</button>
	</footer>
</dialog>

<!-- Reset Network Confirmation Dialog -->
<dialog bind:this={resetDialog} class="dialog" onclose={() => (showResetDialog = false)}>
	<h2>Reset Network Settings?</h2>
	<p>
		This will clear your WiFi credentials and reboot the device into setup
		mode. You'll need to reconnect to the scoreboard's WiFi network to
		reconfigure it.
	</p>
	<footer class="dialog-footer">
		<button class="btn outline" onclick={() => (showResetDialog = false)}>Cancel</button>
		<button class="btn default" onclick={handleResetNetwork}>Reset & Reboot</button>
	</footer>
</dialog>

<!-- Reboot Overlay (handles the actual reboot process) -->
<RebootOverlay />

<style>
	/* Layout */
	.settings-page {
		max-width: 42rem;
		margin-inline: auto;
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
	}

	.page-title {
		font-size: 1.5rem;
		font-weight: 700;
	}

	.page-description {
		color: var(--muted-foreground);
	}

	.card-stack {
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
	}

	/* Card */
	.card {
		background: var(--card);
		color: var(--card-foreground);
		border: 1px solid var(--border);
		border-radius: 0.75rem;
		box-shadow: 0 1px 2px oklch(0 0 0 / 5%);
	}

	.card.border-destructive {
		border-color: var(--destructive);
	}

	.card.border-warning {
		border-color: oklch(0.769 0.188 70.08);
	}

	.card-header {
		padding: 1.5rem;
		padding-block-end: 0;
		display: flex;
		flex-direction: column;
		gap: 0.375rem;
	}

	.card-title {
		font-weight: 600;
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.card-title.text-destructive {
		color: var(--destructive);
	}

	.card-description {
		color: var(--muted-foreground);
		font-size: 0.875rem;
	}

	.card-content {
		padding: 1.5rem;
		display: flex;
		flex-direction: column;
		gap: 1rem;
	}

	.card-content.gap-lg {
		gap: 1.5rem;
	}

	/* Skeleton */
	.skeleton {
		background: var(--muted);
		border-radius: 0.375rem;
		animation: shimmer 2s infinite;
	}

	/* Form elements */
	.field-group {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.field-group.nested {
		padding-block-start: 0.5rem;
	}

	label {
		font-size: 0.875rem;
		font-weight: 500;
	}

	input[type="text"],
	input[type="password"],
	input[type="url"],
	input[type="number"] {
		height: 2.25rem;
		width: 100%;
		border-radius: 0.375rem;
		border: 1px solid var(--input);
		background: transparent;
		padding-inline: 0.75rem;
		font-size: 0.875rem;

		&::placeholder {
			color: var(--muted-foreground);
		}

		&:focus-visible {
			outline: none;
			border-color: var(--ring);
			box-shadow: 0 0 0 2px var(--background), 0 0 0 4px var(--ring);
		}
	}

	.input-with-toggle {
		padding-inline-end: 2.5rem;
	}

	.input-wrapper {
		position: relative;
	}

	.toggle-btn {
		position: absolute;
		right: 0;
		top: 0;
		height: 100%;
		padding-inline: 0.75rem;

		&:hover {
			background: transparent !important;
		}
	}

	input[type="range"] {
		appearance: none;
		-webkit-appearance: none;
		width: 100%;
		height: 0.5rem;
		border-radius: 9999px;
		background: var(--secondary);
		outline: none;

		&::-webkit-slider-thumb {
			-webkit-appearance: none;
			height: 1.25rem;
			width: 1.25rem;
			border-radius: 50%;
			background: var(--primary);
			cursor: pointer;
			border: 2px solid var(--background);
			box-shadow: 0 1px 3px oklch(0 0 0 / 15%);
		}
	}

	select {
		height: 2.25rem;
		width: 100%;
		border-radius: 0.375rem;
		border: 1px solid var(--input);
		background: var(--card);
		padding-inline: 0.75rem;
		font-size: 0.875rem;

		&:focus-visible {
			outline: none;
			border-color: var(--ring);
			box-shadow: 0 0 0 2px var(--background), 0 0 0 4px var(--ring);
		}
	}

	/* Switch */
	.switch {
		position: relative;
		display: inline-flex;
		cursor: pointer;
	}

	.switch input {
		position: absolute;
		opacity: 0;
		width: 0;
		height: 0;
	}

	.switch-track {
		width: 2.75rem;
		height: 1.5rem;
		border-radius: 9999px;
		background: var(--input);
		transition: background 0.2s;
		display: flex;
		align-items: center;
		padding: 0.125rem;
	}

	.switch input:checked + .switch-track {
		background: var(--primary);
	}

	.switch-thumb {
		width: 1.25rem;
		height: 1.25rem;
		border-radius: 50%;
		background: var(--background);
		transition: transform 0.2s;
		box-shadow: 0 1px 2px oklch(0 0 0 / 15%);
	}

	.switch input:checked + .switch-track .switch-thumb {
		transform: translateX(1.25rem);
	}

	/* Progress */
	progress {
		width: 100%;
		height: 0.5rem;
		border-radius: 9999px;
		overflow: hidden;
		appearance: none;
	}

	progress::-webkit-progress-bar {
		background: var(--secondary);
		border-radius: 9999px;
	}

	progress::-webkit-progress-value {
		background: var(--primary);
		border-radius: 9999px;
	}

	/* Separator */
	.separator {
		border: none;
		border-top: 1px solid var(--border);
	}

	/* Color picker */
	.color-picker {
		height: 2.25rem;
		width: 3.5rem;
		cursor: pointer;
		border-radius: 0.375rem;
		border: 1px solid var(--border);
	}

	/* Dialog */
	.dialog {
		border: 1px solid var(--border);
		border-radius: 0.75rem;
		background: var(--card);
		color: var(--card-foreground);
		padding: 1.5rem;
		max-width: 28rem;
		width: calc(100% - 2rem);
		box-shadow: 0 10px 25px oklch(0 0 0 / 25%);
	}

	.dialog::backdrop {
		background: oklch(0 0 0 / 80%);
	}

	.dialog h2 {
		font-size: 1.125rem;
		font-weight: 600;
		margin-block-end: 0.5rem;
	}

	.dialog p {
		color: var(--muted-foreground);
		font-size: 0.875rem;
		margin-block-end: 1rem;
	}

	.dialog-footer {
		display: flex;
		justify-content: flex-end;
		gap: 0.5rem;
	}

	/* Alert */
	.alert {
		border-radius: 0.75rem;
		padding: 1rem;
	}

	.alert.destructive {
		border: 1px solid var(--destructive);
		color: var(--destructive);
	}

	/* Buttons */
	.btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		border-radius: 0.375rem;
		font-size: 0.875rem;
		font-weight: 500;
		cursor: pointer;
		border: none;
		transition: background-color 0.15s;
		outline: none;
		height: 2.25rem;
		padding-inline: 1rem;

		&:disabled {
			opacity: 0.5;
			pointer-events: none;
		}

		&:focus-visible {
			box-shadow: 0 0 0 2px var(--background), 0 0 0 4px var(--ring);
		}

		&.default {
			background: var(--primary);
			color: var(--primary-foreground);
		}

		&.outline {
			background: var(--card);
			border: 1px solid var(--border);

			&:hover {
				background: var(--accent);
			}
		}

		&.ghost {
			background: transparent;

			&:hover {
				background: var(--accent);
			}
		}

		&.destructive {
			background: var(--destructive);
			color: white;
		}

		&.sm {
			height: 2rem;
			padding-inline: 0.75rem;
			font-size: 0.75rem;
		}
	}

	.btn :global(svg) {
		width: 1rem;
		height: 1rem;
	}

	/* Utility helpers */
	.row-between {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.row-center {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.status-grid {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1rem;
		font-size: 0.875rem;
	}

	.label-group {
		display: flex;
		flex-direction: column;
		gap: 0.125rem;
	}

	.text-muted {
		color: var(--muted-foreground);
	}

	.text-sm {
		font-size: 0.875rem;
	}

	.text-sm.text-muted {
		font-size: 0.875rem;
		color: var(--muted-foreground);
	}

	.text-xs {
		font-size: 0.75rem;
	}

	.text-xs.text-muted {
		font-size: 0.75rem;
		color: var(--muted-foreground);
	}

	.text-sm-medium {
		font-size: 0.875rem;
		font-weight: 500;
	}

	.hint {
		font-size: 0.75rem;
		color: var(--muted-foreground);
	}

	.capitalize {
		text-transform: capitalize;
	}

	.status-value {
		margin-inline-start: 0.5rem;
		font-weight: 500;
	}

	/* Icon helpers (applied via class on Lucide components) */
	:global(.icon-green) {
		width: 1.25rem;
		height: 1.25rem;
		color: oklch(0.723 0.219 149.579);
	}

	:global(.icon-amber) {
		width: 1.25rem;
		height: 1.25rem;
		color: oklch(0.769 0.188 70.08);
	}

	:global(.icon-muted) {
		width: 1rem;
		height: 1rem;
		color: var(--muted-foreground);
	}

	/* Action bar */
	.action-bar {
		display: flex;
		justify-content: space-between;
		padding-block-end: 2rem;
	}

	.action-group {
		display: flex;
		gap: 0.5rem;
	}
</style>

<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import { rebootStore } from "$lib/stores/reboot.svelte";
	import { picoApi } from "$lib/api";
	import SecretInput from "$lib/components/SecretInput.svelte";
	import NumberField from "$lib/components/NumberField.svelte";

	let showResetDialog = $state(false);
	let resetDialog: HTMLDialogElement;

	$effect(() => {
		if (!resetDialog) return;
		if (showResetDialog && !resetDialog.open) resetDialog.showModal();
		else if (!showResetDialog && resetDialog.open) resetDialog.close();
	});

	async function handleResetNetwork() {
		showResetDialog = false;
		if (!settingsStore.status || !settingsStore.config) return;
		await picoApi.resetNetwork();
		// Explicit 'network_reset' scenario: the device will boot into AP mode
		await rebootStore.initiateRebootWithScenario(
			settingsStore.status,
			settingsStore.config,
			"network_reset"
		);
	}
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Network</h3>
			<p class="card-description">WiFi connection settings</p>
		</header>
		<div class="card-content">
			<div class="field-group">
				<label for="wifi-ssid">WiFi Network (SSID)</label>
				<input
					id="wifi-ssid"
					type="text"
					placeholder="Enter network name"
					value={config.network.ssid}
					oninput={(e) =>
						settingsStore.updateNetwork("ssid", (e.target as HTMLInputElement).value)}
				/>
			</div>
			<SecretInput
				id="wifi-password"
				label="WiFi Password"
				placeholder="Enter password"
				value={config.network.password}
				oninput={(value) => settingsStore.updateNetwork("password", value)}
			/>

			<hr class="separator" />

			<div class="field-group">
				<label for="device-name">Device Name</label>
				<input
					id="device-name"
					type="text"
					placeholder="scoreboard"
					value={config.network.device_name}
					oninput={(e) =>
						settingsStore.updateNetwork("device_name", (e.target as HTMLInputElement).value)}
				/>
				<p class="hint">Access the device at {config.network.device_name}.local</p>
			</div>

			<NumberField
				id="connect-timeout"
				label="Connection Timeout (seconds)"
				hint="Per-attempt WiFi connection timeout before falling back to setup mode"
				min={1}
				value={config.network.connect_timeout_seconds}
				oncommit={(value) => settingsStore.updateNetwork("connect_timeout_seconds", value)}
			/>

			<hr class="separator" />

			<div class="row-between">
				<div class="label-group">
					<span class="label-text">Reset Network</span>
					<p class="text-sm text-muted">Clear WiFi credentials and return to setup mode</p>
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
{/if}

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

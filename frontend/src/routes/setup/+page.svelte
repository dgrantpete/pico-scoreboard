<script lang="ts">
	import { onMount } from "svelte";
	import Wifi from "@lucide/svelte/icons/wifi";
	import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
	import RefreshCw from "@lucide/svelte/icons/refresh-cw";
	import { picoApi } from "$lib/api";
	import type { NetworkStatus, Config } from "$lib/api/types";
	import { rebootStore } from "$lib/stores/reboot.svelte";
	import RebootOverlay from "$lib/components/RebootOverlay.svelte";
	import SecretInput from "$lib/components/SecretInput.svelte";
	import LoadingSkeleton from "$lib/components/LoadingSkeleton.svelte";

	// Loading and error states
	let isLoading = $state(true);
	let isSaving = $state(false);
	let error = $state<string | null>(null);

	// Data from API
	let status = $state<NetworkStatus | null>(null);
	let config = $state<Config | null>(null);

	// Form fields
	let ssid = $state("");
	let password = $state("");
	let apiUrl = $state("");
	let apiKey = $state("");

	// Validation
	const isValid = $derived(ssid.trim().length > 0);

	onMount(async () => {
		try {
			const [statusData, configData] = await Promise.all([
				picoApi.getStatus(),
				picoApi.getConfig()
			]);
			status = statusData;
			config = configData;

			// Pre-fill form from config
			ssid = configData.network.ssid;
			password = configData.network.password;
			apiUrl = configData.api.url;
			apiKey = configData.api.key;
		} catch (e) {
			error = e instanceof Error ? e.message : "Failed to load configuration";
		} finally {
			isLoading = false;
		}
	});

	async function handleSubmit() {
		if (!isValid || !status || !config) return;

		isSaving = true;
		error = null;

		try {
			// The device is still running with the config loaded at mount —
			// that's the "original" the reboot scenario is derived from.
			const originalConfig = config;

			const updatedConfig = await picoApi.updateConfig({
				network: { ssid, password },
				api: { url: apiUrl, key: apiKey }
			});
			config = updatedConfig;

			await rebootStore.initiateReboot(status, originalConfig, updatedConfig);
		} catch (e) {
			error = e instanceof Error ? e.message : "Failed to save configuration";
			isSaving = false;
		}
	}
</script>

<div class="setup-page">
	{#if isLoading}
		<LoadingSkeleton cards={2} />
	{:else if error && !config}
		<!-- Error state when we couldn't load at all -->
		<section class="card border-destructive">
			<header class="card-header">
				<h3 class="card-title text-destructive">Connection Error</h3>
				<p class="card-description">{error}</p>
			</header>
			<div class="card-content">
				<button class="btn default" onclick={() => window.location.reload()}>
					<RefreshCw />
					Retry
				</button>
			</div>
		</section>
	{:else}
		<!-- Header with context-aware messaging -->
		<div class="header-group">
			{#if status?.setup_reason === "bad_auth"}
				<div class="header-row">
					<div class="icon-circle warn">
						<TriangleAlert />
					</div>
					<h2 class="page-title">Wrong Password</h2>
				</div>
				<p class="subtitle">
					"<span class="font-medium">{status.configured_ssid}</span>" rejected the
					password. Re-enter it below and reconnect.
				</p>
			{:else if status?.setup_reason === "connection_failed"}
				<div class="header-row">
					<div class="icon-circle warn">
						<TriangleAlert />
					</div>
					<h2 class="page-title">Connection Issue</h2>
				</div>
				<p class="subtitle">
					We couldn't connect to "<span class="font-medium">{status.configured_ssid}</span
					>". Check your credentials or try a different network.
				</p>
			{:else if status?.setup_mode}
				<div class="header-row">
					<div class="icon-circle primary">
						<Wifi />
					</div>
					<h2 class="page-title">Welcome to Scoreboard Setup</h2>
				</div>
				<p class="subtitle">
					Let's get your scoreboard connected to WiFi so it can fetch live game
					scores.
				</p>
			{:else}
				<div class="header-row">
					<div class="icon-circle ok">
						<Wifi />
					</div>
					<h2 class="page-title">Network Configuration</h2>
				</div>
				<p class="subtitle">
					Your scoreboard is already connected. You can update your network
					settings below if needed.
				</p>
			{/if}
		</div>

		<!-- WiFi Configuration -->
		<section class="card">
			<header class="card-header">
				<h3 class="card-title">WiFi Configuration</h3>
				<p class="card-description">
					Connect your scoreboard to your home WiFi network
				</p>
			</header>
			<div class="card-content stack">
				<div class="field-group">
					<label for="wifi-ssid">WiFi Network (SSID)</label>
					<input
						id="wifi-ssid"
						type="text"
						placeholder="Enter network name"
						bind:value={ssid}
					/>
				</div>
				<SecretInput
					id="wifi-password"
					label="WiFi Password"
					placeholder="Enter password"
					value={password}
					oninput={(value) => (password = value)}
				/>
			</div>
		</section>

		<!-- API Configuration -->
		<section class="card">
			<header class="card-header">
				<h3 class="card-title">API Configuration</h3>
				<p class="card-description">
					Connection settings for fetching live scores
				</p>
			</header>
			<div class="card-content stack">
				<div class="field-group">
					<label for="api-url">API URL</label>
					<input
						id="api-url"
						type="url"
						placeholder="https://api.example.com"
						bind:value={apiUrl}
					/>
				</div>
				<SecretInput
					id="api-key"
					label="API Key"
					placeholder="Enter API key"
					value={apiKey}
					oninput={(value) => (apiKey = value)}
				/>
			</div>
		</section>

		<!-- Error banner -->
		{#if error}
			<div class="alert destructive" role="alert">
				<p>{error}</p>
			</div>
		{/if}

		<!-- Submit button -->
		<div class="submit-row">
			<button
				class="btn default lg"
				onclick={handleSubmit}
				disabled={!isValid || isSaving || rebootStore.isActive}
			>
				{#if isSaving}
					Saving...
				{:else}
					Connect & Restart
				{/if}
			</button>
		</div>
	{/if}
</div>

<!-- Reboot Overlay -->
<RebootOverlay />

<style>
	/* Page-specific layout only — shared component styles live in app.css */
	.setup-page {
		max-width: 42rem;
		margin-inline: auto;
		display: flex;
		flex-direction: column;
		gap: 1.5rem;
	}

	/* Header */
	.header-group {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.header-row {
		display: flex;
		align-items: center;
		gap: 0.75rem;
	}

	.page-title {
		font-size: 1.5rem;
		font-weight: 700;
	}

	.subtitle {
		color: var(--muted-foreground);
	}

	.font-medium {
		font-weight: 500;
	}

	/* Submit */
	.submit-row {
		display: flex;
		justify-content: flex-end;
		padding-block-end: 2rem;
	}
</style>

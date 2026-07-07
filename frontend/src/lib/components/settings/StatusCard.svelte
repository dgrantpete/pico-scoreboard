<script lang="ts">
	import Wifi from "@lucide/svelte/icons/wifi";
	import WifiOff from "@lucide/svelte/icons/wifi-off";
	import { settingsStore } from "$lib/stores/settings.svelte";

	const status = $derived(settingsStore.status);
</script>

<section class="card" class:border-warning={status?.setup_mode}>
	<header class="card-header">
		<h3 class="card-title">
			{#if status?.mode === "station" && status?.connected}
				<Wifi class="icon-ok" />
				Connected to WiFi
			{:else if status?.setup_mode && status?.setup_reason === "bad_auth"}
				<WifiOff class="icon-warn" />
				Wrong WiFi Password
			{:else if status?.setup_mode && status?.setup_reason === "connection_failed"}
				<WifiOff class="icon-warn" />
				Connection Failed
			{:else if status?.setup_mode}
				<WifiOff class="icon-muted" />
				Network Not Configured
			{:else}
				<WifiOff class="icon-muted" />
				Not Connected
			{/if}
		</h3>
		<p class="card-description">
			{#if status?.setup_mode && status?.setup_reason === "bad_auth"}
				The password for "{status.configured_ssid}" was rejected
			{:else if status?.setup_mode && status?.setup_reason === "connection_failed"}
				Could not connect to "{status.configured_ssid}"
			{:else if status?.setup_mode}
				WiFi setup is required to fetch scores
			{:else if status?.connected}
				{status.ip} &bull; {status.hostname} &bull; app
				<span class="app-version">{status.app_version?.slice(0, 8) ?? "dev"}</span>
			{:else}
				Current network connection status
			{/if}
		</p>
	</header>
	{#if status?.setup_mode}
		<div class="card-content">
			<a href="#/setup" class="btn default">Complete Setup</a>
		</div>
	{:else if status?.mode === "ap"}
		<div class="card-content">
			<div class="status-grid">
				<div>
					<span class="text-muted">AP Network:</span>
					<span class="status-value">{status.ap_ssid}</span>
				</div>
				<div>
					<span class="text-muted">AP IP:</span>
					<span class="status-value">{status.ap_ip}</span>
				</div>
			</div>
		</div>
	{:else if !status}
		<div class="card-content">
			<p class="text-sm text-muted">Status unavailable</p>
		</div>
	{/if}
</section>

<style>
	.app-version {
		font-family: ui-monospace, monospace;
	}
</style>

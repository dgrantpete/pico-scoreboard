<script lang="ts">
	import Wifi from "@lucide/svelte/icons/wifi";
	import WifiOff from "@lucide/svelte/icons/wifi-off";
	import RefreshCw from "@lucide/svelte/icons/refresh-cw";
	import { picoApi, type CheckUpdateResponse } from "$lib/api";
	import { settingsStore } from "$lib/stores/settings.svelte";

	const status = $derived(settingsStore.status);

	// --- On-demand OTA check -------------------------------------------------
	// 'installing' covers the whole download -> restart -> re-apply window;
	// the device is intermittently unreachable throughout, so progress is
	// observed purely by polling /api/status until app_version changes.
	type UpdatePhase = "idle" | "checking" | "current" | "installing" | "done" | "failed";
	let phase = $state<UpdatePhase>("idle");
	let note = $state("");

	const busy = $derived(phase === "checking" || phase === "installing");

	function sleep(ms: number): Promise<void> {
		return new Promise((resolve) => setTimeout(resolve, ms));
	}

	function terminalMessage(res: CheckUpdateResponse): string {
		switch (res.status) {
			case "disabled":
				return "Automatic updates are disabled in device settings";
			case "dev_deploy":
				return "Development build installed — updates paused";
			case "no_network":
				return "The scoreboard has no internet connection";
			case "error":
				return `Check failed: ${res.message ?? "unknown error"}`;
			default:
				return `Unexpected response: ${res.status}`;
		}
	}

	async function checkForUpdates() {
		const before = status?.app_version ?? null;
		phase = "checking";
		note = "Checking…";

		let confirmed = false;
		try {
			const res = await picoApi.checkUpdate();
			if (res.status === "current") {
				phase = "current";
				note = `Up to date (${res.version?.slice(0, 8) ?? "?"})`;
				return;
			}
			if (res.status !== "updating") {
				phase = "failed";
				note = terminalMessage(res);
				return;
			}
			confirmed = true;
		} catch (e) {
			if (e && typeof e === "object" && "status" in e && (e as { status: number }).status === 501) {
				phase = "failed";
				note = "Device software too old for remote checks — update over USB";
				return;
			}
			// A dropped response can mean the download already started and
			// froze the device's loop — fall through and watch for the restart.
		}

		phase = "installing";
		note = "Installing update — the scoreboard will restart…";
		const deadline = Date.now() + 180_000;
		while (Date.now() < deadline) {
			await sleep(5000);
			try {
				const s = await picoApi.getStatus({ timeoutMs: 4000 });
				if (s.app_version && s.app_version !== before) {
					phase = "done";
					note = `Updated to ${s.app_version.slice(0, 8)} — reloading…`;
					// The web app itself ships inside the update; reload to serve it.
					setTimeout(() => window.location.reload(), 2500);
					return;
				}
				if (!confirmed) {
					// Device answered with the same version and no update was
					// ever confirmed: the check request just got lost.
					phase = "failed";
					note = "Check did not complete — try again";
					return;
				}
			} catch {
				// Unreachable while downloading/rebooting — keep waiting.
			}
		}
		phase = "failed";
		note = "Still waiting on the update — refresh this page to re-check";
	}
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
	{:else if status?.mode === "station" && status?.connected}
		<div class="card-content">
			<div class="update-row">
				<button class="btn default" onclick={checkForUpdates} disabled={busy}>
					<RefreshCw class={busy ? "icon-muted spinner" : "icon-muted"} />
					Check for updates
				</button>
				{#if phase !== "idle"}
					<span
						class="update-note"
						class:ok={phase === "current" || phase === "done"}
						class:warn={phase === "failed"}
					>
						{note}
					</span>
				{/if}
			</div>
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

	.update-row {
		display: flex;
		align-items: center;
		gap: 0.75rem;
		flex-wrap: wrap;
	}

	.update-note {
		font-size: 0.875rem;
		color: var(--muted-foreground);
	}

	.update-note.ok {
		color: var(--color-ok);
	}

	.update-note.warn {
		color: var(--color-warn);
	}
</style>

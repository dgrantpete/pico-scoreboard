<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import Save from "@lucide/svelte/icons/save";
	import RotateCcw from "@lucide/svelte/icons/rotate-ccw";
	import RefreshCw from "@lucide/svelte/icons/refresh-cw";
	import { settingsStore } from "$lib/stores/settings.svelte";
	import { rebootStore } from "$lib/stores/reboot.svelte";
	import LoadingSkeleton from "$lib/components/LoadingSkeleton.svelte";
	import RebootOverlay from "$lib/components/RebootOverlay.svelte";
	import StatusCard from "$lib/components/settings/StatusCard.svelte";
	import ResourcesCard from "$lib/components/settings/ResourcesCard.svelte";
	import NetworkCard from "$lib/components/settings/NetworkCard.svelte";
	import BackendApiCard from "$lib/components/settings/BackendApiCard.svelte";
	import GameCard from "$lib/components/settings/GameCard.svelte";
	import SportsCard from "$lib/components/settings/SportsCard.svelte";
	import ScreenLayoutsCard from "$lib/components/settings/ScreenLayoutsCard.svelte";
	import DisplayCard from "$lib/components/settings/DisplayCard.svelte";
	import ColorsCard from "$lib/components/settings/ColorsCard.svelte";
	import AdvancedCard from "$lib/components/settings/AdvancedCard.svelte";

	let rebootDialog: HTMLDialogElement;

	$effect(() => {
		if (!rebootDialog) return;
		if (settingsStore.showRebootPrompt && !rebootDialog.open) rebootDialog.showModal();
		else if (!settingsStore.showRebootPrompt && rebootDialog.open) rebootDialog.close();
	});

	onMount(() => {
		settingsStore.load().then(() => {
			// One shared poller feeds the status card and the memory meters
			settingsStore.startStatusPolling();
		});
	});

	onDestroy(() => {
		settingsStore.stopStatusPolling();
	});
</script>

<div class="settings-page stack">
	<div>
		<h2 class="page-title">Settings</h2>
		<p class="page-description">Configure your Pi Pico scoreboard</p>
	</div>

	{#if settingsStore.isLoading}
		<LoadingSkeleton />
	{:else if settingsStore.error && !settingsStore.config}
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
		<StatusCard />
		<ResourcesCard />
		<NetworkCard />
		<BackendApiCard />
		<GameCard />
		<SportsCard />
		<ScreenLayoutsCard />
		<DisplayCard />
		<ColorsCard />
		<AdvancedCard />

		{#if settingsStore.error}
			<div class="alert destructive" role="alert">
				<div class="row-between">
					<span>{settingsStore.error}</span>
					<button class="btn ghost sm" onclick={() => settingsStore.clearError()}>
						Dismiss
					</button>
				</div>
			</div>
		{/if}

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

<!-- Reboot Prompt Dialog: shown after saving settings that only apply at boot -->
<dialog bind:this={rebootDialog} class="dialog" onclose={() => settingsStore.dismissRebootPrompt()}>
	<h2>Reboot Required</h2>
	<p>
		The saved changes (network, watchdog, or sports settings) only take
		effect after a reboot. Would you like to reboot now?
	</p>
	<footer class="dialog-footer">
		<button class="btn outline" onclick={() => settingsStore.dismissRebootPrompt()}>Later</button>
		<button class="btn default" onclick={() => settingsStore.reboot()}>Reboot Now</button>
	</footer>
</dialog>

<!-- Reboot Overlay (handles the actual reboot process) -->
<RebootOverlay />

<style>
	/* Page-specific layout only — shared component styles live in app.css */
	.settings-page {
		max-width: 42rem;
		margin-inline: auto;
	}

	.page-title {
		font-size: 1.5rem;
		font-weight: 700;
	}

	.page-description {
		color: var(--muted-foreground);
	}

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

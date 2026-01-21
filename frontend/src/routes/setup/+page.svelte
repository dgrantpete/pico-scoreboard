<script lang="ts">
	import { onMount } from "svelte";
	import * as Card from "$lib/components/ui/card";
	import * as Alert from "$lib/components/ui/alert";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Skeleton } from "$lib/components/ui/skeleton";
	import Eye from "@lucide/svelte/icons/eye";
	import EyeOff from "@lucide/svelte/icons/eye-off";
	import Wifi from "@lucide/svelte/icons/wifi";
	import WifiOff from "@lucide/svelte/icons/wifi-off";
	import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
	import RefreshCw from "@lucide/svelte/icons/refresh-cw";
	import { picoApi } from "$lib/api";
	import type { NetworkStatus, Config } from "$lib/api/types";
	import { rebootStore } from "$lib/stores/reboot.svelte";
	import RebootOverlay from "$lib/components/RebootOverlay.svelte";

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

	// Visibility toggles
	let showPassword = $state(false);
	let showApiKey = $state(false);

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
			// Update config with form values
			const updatedConfig = await picoApi.updateConfig({
				network: { ssid, password },
				api: { url: apiUrl, key: apiKey }
			});

			// Update local config reference for reboot store
			config = updatedConfig;

			// Initiate reboot with the updated config
			await rebootStore.initiateReboot(status, updatedConfig);
		} catch (e) {
			error = e instanceof Error ? e.message : "Failed to save configuration";
			isSaving = false;
		}
	}
</script>

<div class="mx-auto max-w-2xl space-y-6">
	{#if isLoading}
		<!-- Loading skeleton -->
		<div class="space-y-6">
			<Skeleton class="h-8 w-64" />
			<Skeleton class="h-4 w-96" />
			{#each { length: 2 } as _}
				<Card.Root>
					<Card.Header>
						<Skeleton class="h-6 w-32" />
					</Card.Header>
					<Card.Content class="space-y-4">
						{#each { length: 2 } as _}
							<div class="space-y-2">
								<Skeleton class="h-4 w-24" />
								<Skeleton class="h-9 w-full" />
							</div>
						{/each}
					</Card.Content>
				</Card.Root>
			{/each}
		</div>
	{:else if error && !config}
		<!-- Error state when we couldn't load at all -->
		<Card.Root class="border-destructive">
			<Card.Header>
				<Card.Title class="text-destructive">Connection Error</Card.Title>
				<Card.Description>{error}</Card.Description>
			</Card.Header>
			<Card.Content>
				<Button onclick={() => window.location.reload()}>
					<RefreshCw class="mr-2 h-4 w-4" />
					Retry
				</Button>
			</Card.Content>
		</Card.Root>
	{:else}
		<!-- Header with context-aware messaging -->
		<div class="space-y-2">
			{#if status?.setup_reason === "connection_failed"}
				<div class="flex items-center gap-3">
					<div class="rounded-full bg-amber-100 p-2 dark:bg-amber-900">
						<TriangleAlert class="h-6 w-6 text-amber-600 dark:text-amber-400" />
					</div>
					<h2 class="text-2xl font-bold">Connection Issue</h2>
				</div>
				<p class="text-muted-foreground">
					We couldn't connect to "<span class="font-medium">{status.configured_ssid}</span
					>". Check your credentials or try a different network.
				</p>
			{:else if status?.setup_mode}
				<div class="flex items-center gap-3">
					<div class="rounded-full bg-primary/10 p-2">
						<Wifi class="h-6 w-6 text-primary" />
					</div>
					<h2 class="text-2xl font-bold">Welcome to Scoreboard Setup</h2>
				</div>
				<p class="text-muted-foreground">
					Let's get your scoreboard connected to WiFi so it can fetch live game
					scores.
				</p>
			{:else}
				<div class="flex items-center gap-3">
					<div class="rounded-full bg-green-100 p-2 dark:bg-green-900">
						<Wifi class="h-6 w-6 text-green-600 dark:text-green-400" />
					</div>
					<h2 class="text-2xl font-bold">Network Configuration</h2>
				</div>
				<p class="text-muted-foreground">
					Your scoreboard is already connected. You can update your network
					settings below if needed.
				</p>
			{/if}
		</div>

		<!-- WiFi Configuration -->
		<Card.Root>
			<Card.Header>
				<Card.Title>WiFi Configuration</Card.Title>
				<Card.Description>
					Connect your scoreboard to your home WiFi network
				</Card.Description>
			</Card.Header>
			<Card.Content class="space-y-4">
				<div class="space-y-2">
					<Label for="wifi-ssid">WiFi Network (SSID)</Label>
					<Input
						id="wifi-ssid"
						type="text"
						placeholder="Enter network name"
						bind:value={ssid}
					/>
				</div>
				<div class="space-y-2">
					<Label for="wifi-password">WiFi Password</Label>
					<div class="relative">
						<Input
							id="wifi-password"
							type={showPassword ? "text" : "password"}
							placeholder="Enter password"
							bind:value={password}
							class="pr-10"
						/>
						<Button
							variant="ghost"
							size="sm"
							class="absolute right-0 top-0 h-full px-3 hover:bg-transparent"
							onclick={() => (showPassword = !showPassword)}
						>
							{#if showPassword}
								<EyeOff class="h-4 w-4 text-muted-foreground" />
							{:else}
								<Eye class="h-4 w-4 text-muted-foreground" />
							{/if}
						</Button>
					</div>
				</div>
			</Card.Content>
		</Card.Root>

		<!-- API Configuration -->
		<Card.Root>
			<Card.Header>
				<Card.Title>API Configuration</Card.Title>
				<Card.Description>
					Connection settings for fetching live scores
				</Card.Description>
			</Card.Header>
			<Card.Content class="space-y-4">
				<div class="space-y-2">
					<Label for="api-url">API URL</Label>
					<Input
						id="api-url"
						type="url"
						placeholder="https://api.example.com"
						bind:value={apiUrl}
					/>
				</div>
				<div class="space-y-2">
					<Label for="api-key">API Key</Label>
					<div class="relative">
						<Input
							id="api-key"
							type={showApiKey ? "text" : "password"}
							placeholder="Enter API key"
							bind:value={apiKey}
							class="pr-10"
						/>
						<Button
							variant="ghost"
							size="sm"
							class="absolute right-0 top-0 h-full px-3 hover:bg-transparent"
							onclick={() => (showApiKey = !showApiKey)}
						>
							{#if showApiKey}
								<EyeOff class="h-4 w-4 text-muted-foreground" />
							{:else}
								<Eye class="h-4 w-4 text-muted-foreground" />
							{/if}
						</Button>
					</div>
				</div>
			</Card.Content>
		</Card.Root>

		<!-- Error banner -->
		{#if error}
			<Alert.Root variant="destructive">
				<Alert.Description>{error}</Alert.Description>
			</Alert.Root>
		{/if}

		<!-- Submit button -->
		<div class="flex justify-end pb-8">
			<Button
				onclick={handleSubmit}
				disabled={!isValid || isSaving || rebootStore.isActive}
				size="lg"
			>
				{#if isSaving}
					Saving...
				{:else}
					Connect & Restart
				{/if}
			</Button>
		</div>
	{/if}
</div>

<!-- Reboot Overlay -->
<RebootOverlay />

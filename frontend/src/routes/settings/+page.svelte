<script lang="ts">
	import { onMount } from "svelte";
	import * as Card from "$lib/components/ui/card";
	import * as AlertDialog from "$lib/components/ui/alert-dialog";
	import * as Alert from "$lib/components/ui/alert";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Slider } from "$lib/components/ui/slider";
	import { Separator } from "$lib/components/ui/separator";
	import { Label } from "$lib/components/ui/label";
	import { Skeleton } from "$lib/components/ui/skeleton";
	import Save from "@lucide/svelte/icons/save";
	import Eye from "@lucide/svelte/icons/eye";
	import EyeOff from "@lucide/svelte/icons/eye-off";
	import RotateCcw from "@lucide/svelte/icons/rotate-ccw";
	import Wifi from "@lucide/svelte/icons/wifi";
	import WifiOff from "@lucide/svelte/icons/wifi-off";
	import RefreshCw from "@lucide/svelte/icons/refresh-cw";
	import { settingsStore } from "$lib/stores/settings.svelte";
	import { rebootStore } from "$lib/stores/reboot.svelte";
	import { picoApi } from "$lib/api";
	import RebootOverlay from "$lib/components/RebootOverlay.svelte";

	// Password visibility toggles
	let showWifiPassword = $state(false);
	let showApiKey = $state(false);

	// Reset network dialog state
	let showResetDialog = $state(false);

	// Local binding for brightness slider
	let brightnessValue = $derived(
		settingsStore.config ? settingsStore.config.display.brightness : 100,
	);

	onMount(() => {
		settingsStore.load();
	});

	function handleBrightnessChange(value: number) {
		settingsStore.updateDisplay("brightness", value);
	}

	async function handleResetNetwork() {
		showResetDialog = false;
		await picoApi.resetNetwork();
		await settingsStore.reboot();
	}
</script>

<div class="mx-auto max-w-2xl space-y-6">
	<div>
		<h2 class="text-2xl font-bold">Settings</h2>
		<p class="text-muted-foreground">Configure your Pi Pico scoreboard</p>
	</div>

	{#if settingsStore.isLoading}
		<!-- Loading skeleton -->
		<div class="space-y-6">
			{#each { length: 4 } as _}
				<Card.Root>
					<Card.Header>
						<Skeleton class="h-6 w-32" />
						<Skeleton class="h-4 w-48" />
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
	{:else if settingsStore.error && !settingsStore.config}
		<!-- Error state -->
		<Card.Root class="border-destructive">
			<Card.Header>
				<Card.Title class="text-destructive">Connection Error</Card.Title>
				<Card.Description>{settingsStore.error}</Card.Description>
			</Card.Header>
			<Card.Content>
				<Button onclick={() => settingsStore.load()}>
					<RefreshCw class="mr-2 h-4 w-4" />
					Retry
				</Button>
			</Card.Content>
		</Card.Root>
	{:else if settingsStore.config}
		<!-- Device Status -->
		<Card.Root
			class={settingsStore.status?.setup_mode ? "border-amber-500" : ""}
		>
			<Card.Header>
				<Card.Title class="flex items-center gap-2">
					{#if settingsStore.status?.mode === "station" && settingsStore.status?.connected}
						<Wifi class="h-5 w-5 text-green-500" />
						Connected to WiFi
					{:else if settingsStore.status?.setup_mode && settingsStore.status?.setup_reason === "connection_failed"}
						<WifiOff class="h-5 w-5 text-amber-500" />
						Connection Failed
					{:else if settingsStore.status?.setup_mode}
						<WifiOff class="h-5 w-5 text-muted-foreground" />
						Network Not Configured
					{:else}
						<WifiOff class="h-5 w-5 text-muted-foreground" />
						Not Connected
					{/if}
				</Card.Title>
				<Card.Description>
					{#if settingsStore.status?.setup_mode && settingsStore.status?.setup_reason === "connection_failed"}
						Could not connect to "{settingsStore.status.configured_ssid}"
					{:else if settingsStore.status?.setup_mode}
						WiFi setup is required to fetch scores
					{:else if settingsStore.status?.connected}
						{settingsStore.status.ip} &bull; {settingsStore.status.hostname}
					{:else}
						Current network connection status
					{/if}
				</Card.Description>
			</Card.Header>
			<Card.Content>
				{#if settingsStore.status?.setup_mode}
					<Button href="#/setup">Complete Setup</Button>
				{:else if settingsStore.status}
					<div class="grid grid-cols-2 gap-4 text-sm">
						<div>
							<span class="text-muted-foreground">Mode:</span>
							<span class="ml-2 font-medium capitalize"
								>{settingsStore.status.mode}</span
							>
						</div>
						<div>
							<span class="text-muted-foreground">Connected:</span>
							<span class="ml-2 font-medium"
								>{settingsStore.status.connected ? "Yes" : "No"}</span
							>
						</div>
						{#if settingsStore.status.mode === "station" && settingsStore.status.ip}
							<div>
								<span class="text-muted-foreground">IP Address:</span>
								<span class="ml-2 font-medium">{settingsStore.status.ip}</span>
							</div>
							<div>
								<span class="text-muted-foreground">Hostname:</span>
								<span class="ml-2 font-medium"
									>{settingsStore.status.hostname}</span
								>
							</div>
						{:else if settingsStore.status.mode === "ap"}
							<div>
								<span class="text-muted-foreground">AP Network:</span>
								<span class="ml-2 font-medium"
									>{settingsStore.status.ap_ssid}</span
								>
							</div>
							<div>
								<span class="text-muted-foreground">AP IP:</span>
								<span class="ml-2 font-medium"
									>{settingsStore.status.ap_ip}</span
								>
							</div>
						{/if}
					</div>
				{:else}
					<p class="text-sm text-muted-foreground">Status unavailable</p>
				{/if}
			</Card.Content>
		</Card.Root>

		<!-- Network Configuration -->
		<Card.Root>
			<Card.Header>
				<Card.Title>Network</Card.Title>
				<Card.Description>WiFi connection settings</Card.Description>
			</Card.Header>
			<Card.Content class="space-y-4">
				<!-- WiFi Settings -->
				<div class="space-y-2">
					<Label for="wifi-ssid">WiFi Network (SSID)</Label>
					<Input
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
				<div class="space-y-2">
					<Label for="wifi-password">WiFi Password</Label>
					<div class="relative">
						<Input
							id="wifi-password"
							type={showWifiPassword ? "text" : "password"}
							placeholder="Enter password"
							value={settingsStore.config.network.password}
							oninput={(e) =>
								settingsStore.updateNetwork(
									"password",
									(e.target as HTMLInputElement).value,
								)}
							class="pr-10"
						/>
						<Button
							variant="ghost"
							size="sm"
							class="absolute right-0 top-0 h-full px-3 hover:bg-transparent"
							onclick={() => (showWifiPassword = !showWifiPassword)}
						>
							{#if showWifiPassword}
								<EyeOff class="h-4 w-4 text-muted-foreground" />
							{:else}
								<Eye class="h-4 w-4 text-muted-foreground" />
							{/if}
						</Button>
					</div>
				</div>

				<Separator />

				<div class="space-y-2">
					<Label for="device-name">Device Name</Label>
					<Input
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
					<p class="text-xs text-muted-foreground">
						Access the device at {settingsStore.config.network.device_name}.local
					</p>
				</div>

				<div class="space-y-2">
					<Label for="connect-timeout">Connection Timeout (seconds)</Label>
					<Input
						id="connect-timeout"
						type="number"
						min="1"
						value={settingsStore.config.network.connect_timeout_seconds}
						oninput={(e) =>
							settingsStore.updateNetwork(
								"connect_timeout_seconds",
								parseInt((e.target as HTMLInputElement).value) || 15,
							)}
					/>
					<p class="text-xs text-muted-foreground">
						Time to wait before falling back to setup mode
					</p>
				</div>

				<Separator />

				<!-- Reset Network -->
				<div class="flex items-center justify-between">
					<div class="space-y-0.5">
						<Label>Reset Network</Label>
						<p class="text-sm text-muted-foreground">
							Clear WiFi credentials and return to setup mode
						</p>
					</div>
					<Button
						variant="destructive"
						onclick={() => (showResetDialog = true)}
						disabled={rebootStore.isActive}
					>
						Reset Network
					</Button>
				</div>
			</Card.Content>
		</Card.Root>

		<!-- Backend API Configuration -->
		<Card.Root>
			<Card.Header>
				<Card.Title>Backend API</Card.Title>
				<Card.Description
					>Connection settings for the scores API</Card.Description
				>
			</Card.Header>
			<Card.Content class="space-y-4">
				<div class="space-y-2">
					<Label for="api-url">API URL</Label>
					<Input
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
				<div class="space-y-2">
					<Label for="api-key">API Key</Label>
					<div class="relative">
						<Input
							id="api-key"
							type={showApiKey ? "text" : "password"}
							placeholder="Enter API key"
							value={settingsStore.config.api.key}
							oninput={(e) =>
								settingsStore.updateApi(
									"key",
									(e.target as HTMLInputElement).value,
								)}
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

		<!-- Display Settings -->
		<Card.Root>
			<Card.Header>
				<Card.Title>Display</Card.Title>
				<Card.Description
					>LED matrix brightness and refresh settings</Card.Description
				>
			</Card.Header>
			<Card.Content class="space-y-6">
				<div class="space-y-2">
					<div class="flex items-center justify-between">
						<Label>Brightness</Label>
						<span class="text-sm text-muted-foreground"
							>{settingsStore.config.display.brightness}%</span
						>
					</div>
					<Slider
						type="single"
						value={brightnessValue}
						onValueChange={handleBrightnessChange}
						max={100}
						step={1}
					/>
				</div>

				<Separator />

				<div class="space-y-2">
					<Label for="poll-interval">Poll Interval (seconds)</Label>
					<Input
						id="poll-interval"
						type="number"
						min="1"
						value={settingsStore.config.display.poll_interval_seconds}
						oninput={(e) =>
							settingsStore.updateDisplay(
								"poll_interval_seconds",
								parseInt((e.target as HTMLInputElement).value) || 30,
							)}
					/>
					<p class="text-xs text-muted-foreground">
						How often to fetch game updates from the API
					</p>
				</div>
			</Card.Content>
		</Card.Root>

		<!-- Advanced Settings -->
		<Card.Root>
			<Card.Header>
				<Card.Title>Advanced</Card.Title>
				<Card.Description>Server and caching configuration</Card.Description>
			</Card.Header>
			<Card.Content class="space-y-4">
				<div class="space-y-2">
					<Label for="cache-max-age">Cache Max Age (seconds)</Label>
					<Input
						id="cache-max-age"
						type="number"
						min="0"
						value={settingsStore.config.server.cache_max_age_seconds}
						oninput={(e) =>
							settingsStore.updateServer(
								"cache_max_age_seconds",
								parseInt((e.target as HTMLInputElement).value) || 0,
							)}
					/>
					<p class="text-xs text-muted-foreground">
						HTTP cache duration for static content (0 = no caching)
					</p>
				</div>
			</Card.Content>
		</Card.Root>

		<!-- Error banner -->
		{#if settingsStore.error}
			<Alert.Root variant="destructive">
				<Alert.Description class="flex items-center justify-between">
					<span>{settingsStore.error}</span>
					<Button
						variant="ghost"
						size="sm"
						onclick={() => settingsStore.clearError()}
					>
						Dismiss
					</Button>
				</Alert.Description>
			</Alert.Root>
		{/if}

		<!-- Action Buttons -->
		<div class="flex justify-between pb-8">
			<Button
				variant="outline"
				onclick={() => settingsStore.reboot()}
				disabled={rebootStore.isActive}
			>
				<RotateCcw class="mr-2 h-4 w-4" />
				Reboot Device
			</Button>

			<div class="flex gap-2">
				<Button
					variant="outline"
					onclick={() => settingsStore.discard()}
					disabled={!settingsStore.isDirty || settingsStore.isSaving}
				>
					Discard
				</Button>
				<Button
					onclick={() => settingsStore.save()}
					disabled={!settingsStore.isDirty || settingsStore.isSaving}
				>
					<Save class="mr-2 h-4 w-4" />
					{settingsStore.isSaving ? "Saving..." : "Save Changes"}
				</Button>
			</div>
		</div>
	{/if}
</div>

<!-- Reboot Prompt Dialog -->
<AlertDialog.Root open={settingsStore.showRebootPrompt}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Network Settings Changed</AlertDialog.Title>
			<AlertDialog.Description>
				Network configuration has been updated. A reboot is required for changes
				to take effect. Would you like to reboot now?
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel onclick={() => settingsStore.dismissRebootPrompt()}>
				Later
			</AlertDialog.Cancel>
			<AlertDialog.Action onclick={() => settingsStore.reboot()}
				>Reboot Now</AlertDialog.Action
			>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

<!-- Reset Network Confirmation Dialog -->
<AlertDialog.Root open={showResetDialog}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Reset Network Settings?</AlertDialog.Title>
			<AlertDialog.Description>
				This will clear your WiFi credentials and reboot the device into setup
				mode. You'll need to reconnect to the scoreboard's WiFi network to
				reconfigure it.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel onclick={() => (showResetDialog = false)}>
				Cancel
			</AlertDialog.Cancel>
			<AlertDialog.Action onclick={handleResetNetwork}
				>Reset & Reboot</AlertDialog.Action
			>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

<!-- Reboot Overlay (handles the actual reboot process) -->
<RebootOverlay />

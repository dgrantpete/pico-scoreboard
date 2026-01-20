<script lang="ts">
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import { rebootStore } from '$lib/stores/reboot.svelte';
	import Loader2 from '@lucide/svelte/icons/loader-2';
	import Wifi from '@lucide/svelte/icons/wifi';
	import CheckCircle2 from '@lucide/svelte/icons/check-circle-2';
	import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import Copy from '@lucide/svelte/icons/copy';
	import Check from '@lucide/svelte/icons/check';

	let copiedUrl = $state<string | null>(null);

	async function copyToClipboard(text: string) {
		try {
			await navigator.clipboard.writeText(text);
			copiedUrl = text;
			setTimeout(() => (copiedUrl = null), 2000);
		} catch {
			// Clipboard API not available
		}
	}

	// Compute progress percentage for polling
	const progressPercent = $derived(
		rebootStore.maxAttempts > 0
			? Math.min((rebootStore.attemptNumber / rebootStore.maxAttempts) * 100, 100)
			: 0
	);

	// Get target AP SSID (device name becomes the AP SSID)
	const targetApSsid = $derived(rebootStore.targetConfig?.network.device_name ?? 'scoreboard');

	// Get target station SSID
	const targetStationSsid = $derived(rebootStore.targetConfig?.network.ssid ?? 'WiFi');

	// Get target hostname for mDNS
	const targetHostname = $derived(
		`${rebootStore.targetConfig?.network.device_name ?? 'scoreboard'}.local`
	);
</script>

<AlertDialog.Root open={rebootStore.isActive}>
	<AlertDialog.Content class="sm:max-w-md" data-closable="false">
		{#if rebootStore.state === 'initiating'}
			<!-- Initiating reboot -->
			<div class="flex flex-col items-center py-6 text-center">
				<Loader2 class="h-12 w-12 animate-spin text-muted-foreground" />
				<p class="mt-4 text-lg font-medium">Initiating reboot...</p>
			</div>
		{:else if rebootStore.state === 'polling'}
			<!-- Polling for reconnection -->
			<div class="flex flex-col items-center py-6 text-center">
				<Loader2 class="h-12 w-12 animate-spin text-primary" />
				<p class="mt-4 text-lg font-medium">Rebooting device...</p>
				<p class="mt-2 text-sm text-muted-foreground">
					Waiting for device to come back online.
				</p>
				<div class="mt-6 w-full space-y-2">
					<div class="flex items-center justify-between text-sm text-muted-foreground">
						<span>Attempt {rebootStore.attemptNumber}</span>
					</div>
					<div class="h-2 w-full overflow-hidden rounded-full bg-muted">
						<div
							class="h-full bg-primary transition-all duration-500"
							style="width: {progressPercent}%"
						></div>
					</div>
				</div>
			</div>
		{:else if rebootStore.state === 'switching_to_ap'}
			<!-- Switching to AP mode -->
			<AlertDialog.Header>
				<div class="flex justify-center">
					<div class="rounded-full bg-primary/10 p-3">
						<Wifi class="h-8 w-8 text-primary" />
					</div>
				</div>
				<AlertDialog.Title class="text-center">Connect to the device's network</AlertDialog.Title>
				<AlertDialog.Description class="text-center">
					The device is rebooting into Access Point mode.
				</AlertDialog.Description>
			</AlertDialog.Header>

			<div class="space-y-4 py-4">
				<div class="space-y-3">
					<div class="flex items-start gap-3">
						<span
							class="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted text-sm font-medium"
							>1</span
						>
						<p class="text-sm">Open your WiFi settings</p>
					</div>

					<div class="flex items-start gap-3">
						<span
							class="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted text-sm font-medium"
							>2</span
						>
						<div class="flex-1">
							<p class="text-sm">Connect to:</p>
							<div
								class="mt-1 flex items-center gap-2 rounded-md border bg-muted/50 px-3 py-2 font-mono text-sm"
							>
								<Wifi class="h-4 w-4 text-muted-foreground" />
								<span class="font-medium">{targetApSsid}</span>
							</div>
						</div>
					</div>

					<div class="flex items-start gap-3">
						<span
							class="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted text-sm font-medium"
							>3</span
						>
						<div class="flex-1">
							<p class="text-sm">Then open:</p>
							<div
								class="mt-1 flex items-center justify-between rounded-md border bg-muted/50 px-3 py-2 font-mono text-sm"
							>
								<span>http://192.168.4.1</span>
								<Button
									variant="ghost"
									size="sm"
									class="h-6 w-6 p-0"
									onclick={() => copyToClipboard('http://192.168.4.1')}
								>
									{#if copiedUrl === 'http://192.168.4.1'}
										<Check class="h-4 w-4 text-green-500" />
									{:else}
										<Copy class="h-4 w-4" />
									{/if}
								</Button>
							</div>
						</div>
					</div>
				</div>
			</div>

			<AlertDialog.Footer class="sm:justify-center">
				<Button onclick={() => rebootStore.userConfirmedConnection()} class="w-full sm:w-auto">
					I'm Connected
				</Button>
			</AlertDialog.Footer>
		{:else if rebootStore.state === 'switching_to_station'}
			<!-- Switching to Station mode -->
			<AlertDialog.Header>
				<div class="flex justify-center">
					<div class="rounded-full bg-primary/10 p-3">
						<Wifi class="h-8 w-8 text-primary" />
					</div>
				</div>
				<AlertDialog.Title class="text-center">Connect to your WiFi network</AlertDialog.Title>
				<AlertDialog.Description class="text-center">
					The device is rebooting and will connect to your WiFi.
				</AlertDialog.Description>
			</AlertDialog.Header>

			<div class="space-y-4 py-4">
				<div class="space-y-3">
					<div class="flex items-start gap-3">
						<span
							class="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted text-sm font-medium"
							>1</span
						>
						<div class="flex-1">
							<p class="text-sm">Connect to your network:</p>
							<div
								class="mt-1 flex items-center gap-2 rounded-md border bg-muted/50 px-3 py-2 font-mono text-sm"
							>
								<Wifi class="h-4 w-4 text-muted-foreground" />
								<span class="font-medium">{targetStationSsid}</span>
							</div>
						</div>
					</div>

					<div class="flex items-start gap-3">
						<span
							class="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-muted text-sm font-medium"
							>2</span
						>
						<div class="flex-1">
							<p class="text-sm">Then open:</p>
							<div
								class="mt-1 flex items-center justify-between rounded-md border bg-muted/50 px-3 py-2 font-mono text-sm"
							>
								<span>http://{targetHostname}</span>
								<Button
									variant="ghost"
									size="sm"
									class="h-6 w-6 p-0"
									onclick={() => copyToClipboard(`http://${targetHostname}`)}
								>
									{#if copiedUrl === `http://${targetHostname}`}
										<Check class="h-4 w-4 text-green-500" />
									{:else}
										<Copy class="h-4 w-4" />
									{/if}
								</Button>
							</div>
						</div>
					</div>
				</div>

				<div class="rounded-md border border-amber-200 bg-amber-50 p-3 dark:border-amber-900 dark:bg-amber-950">
					<div class="flex gap-2">
						<AlertTriangle class="h-5 w-5 shrink-0 text-amber-600 dark:text-amber-500" />
						<p class="text-sm text-amber-800 dark:text-amber-200">
							If the device can't connect to WiFi, it will create a
							<span class="font-medium">"{targetApSsid}"</span> network instead.
						</p>
					</div>
				</div>
			</div>

			<AlertDialog.Footer class="sm:justify-center">
				<Button onclick={() => rebootStore.userConfirmedConnection()} class="w-full sm:w-auto">
					I'm Connected
				</Button>
			</AlertDialog.Footer>
		{:else if rebootStore.state === 'hostname_changed'}
			<!-- Hostname is changing - auto redirect -->
			<AlertDialog.Header>
				<div class="flex justify-center">
					<div class="rounded-full bg-primary/10 p-3">
						<ExternalLink class="h-8 w-8 text-primary" />
					</div>
				</div>
				<AlertDialog.Title class="text-center">Device address is changing</AlertDialog.Title>
				<AlertDialog.Description class="text-center">
					The device is rebooting with a new hostname.
				</AlertDialog.Description>
			</AlertDialog.Header>

			<div class="space-y-4 py-4">
				<p class="text-center text-sm text-muted-foreground">
					Redirecting to the new address in {rebootStore.countdownSeconds} seconds...
				</p>

				<div class="flex items-center justify-center gap-2">
					<div
						class="flex items-center justify-between rounded-md border bg-muted/50 px-3 py-2 font-mono text-sm"
					>
						<span>http://{targetHostname}</span>
						<Button
							variant="ghost"
							size="sm"
							class="ml-2 h-6 w-6 p-0"
							onclick={() => copyToClipboard(`http://${targetHostname}`)}
						>
							{#if copiedUrl === `http://${targetHostname}`}
								<Check class="h-4 w-4 text-green-500" />
							{:else}
								<Copy class="h-4 w-4" />
							{/if}
						</Button>
					</div>
				</div>
			</div>

			<AlertDialog.Footer class="sm:justify-center">
				<Button onclick={() => rebootStore.redirectToTarget()} class="w-full sm:w-auto">
					Go Now
				</Button>
			</AlertDialog.Footer>
		{:else if rebootStore.state === 'redirecting'}
			<!-- Redirecting to new address -->
			<div class="flex flex-col items-center py-6 text-center">
				<Loader2 class="h-12 w-12 animate-spin text-primary" />
				<p class="mt-4 text-lg font-medium">Redirecting...</p>
				<p class="mt-2 text-sm text-muted-foreground">
					Taking you to {rebootStore.targetUrl}
				</p>
			</div>
		{:else if rebootStore.state === 'connected'}
			<!-- Device is back online -->
			<div class="flex flex-col items-center py-6 text-center">
				<div class="rounded-full bg-green-100 p-3 dark:bg-green-900">
					<CheckCircle2 class="h-12 w-12 text-green-600 dark:text-green-400" />
				</div>
				<p class="mt-4 text-lg font-medium">Device is back online!</p>
				<p class="mt-2 text-sm text-muted-foreground">
					Refreshing in {rebootStore.countdownSeconds}...
				</p>
				<Button onclick={() => rebootStore.refreshNow()} variant="outline" class="mt-4">
					Refresh Now
				</Button>
			</div>
		{:else if rebootStore.state === 'timeout'}
			<!-- Timeout - device didn't respond -->
			<div class="flex flex-col items-center py-6 text-center">
				<div class="rounded-full bg-amber-100 p-3 dark:bg-amber-900">
					<AlertTriangle class="h-12 w-12 text-amber-600 dark:text-amber-400" />
				</div>
				<p class="mt-4 text-lg font-medium">Device not responding</p>
				<p class="mt-2 text-sm text-muted-foreground">
					The device didn't respond within 2 minutes. It may still be rebooting, or the network
					configuration may have changed.
				</p>
				<div class="mt-6 flex gap-2">
					<Button onclick={() => rebootStore.retry()} variant="outline">Try Again</Button>
					<Button onclick={() => rebootStore.close()}>Close</Button>
				</div>
			</div>
		{:else if rebootStore.state === 'error'}
			<!-- Error state -->
			<div class="flex flex-col items-center py-6 text-center">
				<div class="rounded-full bg-destructive/10 p-3">
					<AlertTriangle class="h-12 w-12 text-destructive" />
				</div>
				<p class="mt-4 text-lg font-medium">Reboot failed</p>
				<p class="mt-2 text-sm text-muted-foreground">
					{rebootStore.errorMessage ?? 'An unknown error occurred.'}
				</p>
				<Button onclick={() => rebootStore.close()} class="mt-6">Close</Button>
			</div>
		{/if}
	</AlertDialog.Content>
</AlertDialog.Root>

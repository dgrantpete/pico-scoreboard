<script lang="ts">
	import { rebootStore } from '$lib/stores/reboot.svelte';
	import Loader2 from '@lucide/svelte/icons/loader-2';
	import Wifi from '@lucide/svelte/icons/wifi';
	import CheckCircle2 from '@lucide/svelte/icons/check-circle-2';
	import AlertTriangle from '@lucide/svelte/icons/alert-triangle';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import Copy from '@lucide/svelte/icons/copy';
	import Check from '@lucide/svelte/icons/check';

	let copiedUrl = $state<string | null>(null);
	let dialogEl: HTMLDialogElement;

	$effect(() => {
		if (!dialogEl) return;
		if (rebootStore.isActive && !dialogEl.open) dialogEl.showModal();
		else if (!rebootStore.isActive && dialogEl.open) dialogEl.close();
	});

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

	// mDNS fallback: some clients (stock Android, some Windows setups) can't
	// resolve .local names. When the device stays on the same network its IP
	// almost always survives the reboot (DHCP re-lease), so offer it.
	const fallbackIp = $derived(rebootStore.preRebootStatus?.ip ?? null);
</script>

<dialog
	bind:this={dialogEl}
	class="dialog"
	oncancel={(e) => {
		if (rebootStore.state !== 'timeout' && rebootStore.state !== 'error' && rebootStore.state !== 'connected') e.preventDefault();
	}}
>
	{#if rebootStore.state === 'initiating'}
		<!-- Initiating reboot -->
		<div class="center-content">
			<Loader2 class="spinner icon-lg" style="color: var(--muted-foreground)" />
			<p class="title">Initiating reboot...</p>
		</div>
	{:else if rebootStore.state === 'polling'}
		<!-- Polling for reconnection -->
		<div class="center-content">
			<Loader2 class="spinner icon-lg" style="color: var(--primary)" />
			<p class="title">Rebooting device...</p>
			<p class="description">
				Waiting for device to come back online.
			</p>
			<div class="progress-section">
				<div class="progress-info">
					<span>Attempt {rebootStore.attemptNumber}</span>
				</div>
				<progress value={progressPercent} max="100"></progress>
			</div>
		</div>
	{:else if rebootStore.state === 'setup_complete'}
		<!-- Setup Complete - Switching from AP to Station mode -->
		<header class="dialog-header">
			<div class="icon-row">
				<div class="icon-circle ok">
					<CheckCircle2 class="icon-md" style="color: var(--color-ok)" />
				</div>
			</div>
			<h2 class="dialog-title">Setup Complete!</h2>
			<p class="dialog-description">
				Your scoreboard is connecting to "{targetStationSsid}".
			</p>
		</header>

		<div class="body-section">
			<div class="steps">
				<div class="step">
					<span class="step-number">1</span>
					<div class="step-content">
						<p class="step-text">Connect to your WiFi network:</p>
						<div class="code-block">
							<Wifi class="icon-sm" style="color: var(--muted-foreground)" />
							<span class="code-bold">{targetStationSsid}</span>
						</div>
					</div>
				</div>

				<div class="step">
					<span class="step-number">2</span>
					<div class="step-content">
						<p class="step-text">Then access your scoreboard at:</p>
						<div class="code-block spread">
							<span>http://{targetHostname}</span>
							<button class="btn-icon" onclick={() => copyToClipboard(`http://${targetHostname}`)}>
								{#if copiedUrl === `http://${targetHostname}`}
									<Check class="icon-sm" style="color: var(--color-ok)" />
								{:else}
									<Copy class="icon-sm" />
								{/if}
							</button>
						</div>
					</div>
				</div>
			</div>

			<div class="alert warning" role="alert">
				<AlertTriangle class="icon-alert" style="color: var(--color-warn)" />
				<p>
					If the device can't connect to WiFi, it will create a
					<span class="bold">"{targetApSsid}"</span> network for setup.
				</p>
			</div>
		</div>

		<footer class="dialog-footer">
			<button class="btn default" onclick={() => rebootStore.userConfirmedConnection()}>
				I'm Connected
			</button>
		</footer>
	{:else if rebootStore.state === 'hostname_changed'}
		<!-- Hostname is changing - auto redirect -->
		<header class="dialog-header">
			<div class="icon-row">
				<div class="icon-circle primary">
					<ExternalLink class="icon-md" style="color: var(--primary)" />
				</div>
			</div>
			<h2 class="dialog-title">Device address is changing</h2>
			<p class="dialog-description">
				The device is rebooting with a new hostname.
			</p>
		</header>

		<div class="body-section">
			<p class="center-text muted-text">
				Redirecting to the new address in {rebootStore.countdownSeconds} seconds...
			</p>

			<div class="center-row">
				<div class="code-block spread">
					<span>http://{targetHostname}</span>
					<button class="btn-icon" onclick={() => copyToClipboard(`http://${targetHostname}`)}>
						{#if copiedUrl === `http://${targetHostname}`}
							<Check class="icon-sm" style="color: var(--color-ok)" />
						{:else}
							<Copy class="icon-sm" />
						{/if}
					</button>
				</div>
			</div>

			{#if fallbackIp}
				<p class="center-text muted-text">
					If that address doesn't load, try http://{fallbackIp} — the device
					usually keeps its IP on the same network.
				</p>
			{/if}
		</div>

		<footer class="dialog-footer">
			<button class="btn default" onclick={() => rebootStore.redirectToTarget()}>
				Go Now
			</button>
		</footer>
	{:else if rebootStore.state === 'network_reset'}
		<!-- Network Reset - Device entering AP mode -->
		<header class="dialog-header">
			<div class="icon-row">
				<div class="icon-circle warn">
					<AlertTriangle class="icon-md" style="color: var(--color-warn)" />
				</div>
			</div>
			<h2 class="dialog-title">Network Reset</h2>
			<p class="dialog-description">
				WiFi credentials cleared. Device is restarting in setup mode.
			</p>
		</header>

		<div class="body-section">
			<div class="steps">
				<div class="step">
					<span class="step-number">1</span>
					<div class="step-content">
						<p class="step-text">Connect to the scoreboard's WiFi network:</p>
						<div class="code-block">
							<Wifi class="icon-sm" style="color: var(--muted-foreground)" />
							<span class="code-bold">{targetApSsid}</span>
						</div>
					</div>
				</div>

				<div class="step">
					<span class="step-number">2</span>
					<div class="step-content">
						<p class="step-text">Open your browser to:</p>
						<div class="code-block spread">
							<span>{rebootStore.targetApUrl}</span>
							<button class="btn-icon" onclick={() => copyToClipboard(rebootStore.targetApUrl)}>
								{#if copiedUrl === rebootStore.targetApUrl}
									<Check class="icon-sm" style="color: var(--color-ok)" />
								{:else}
									<Copy class="icon-sm" />
								{/if}
							</button>
						</div>
					</div>
				</div>
			</div>
		</div>

		<footer class="dialog-footer">
			<button class="btn default" onclick={() => rebootStore.redirectToAp()}>
				I'm Connected
			</button>
		</footer>
	{:else if rebootStore.state === 'ssid_changed'}
		<!-- SSID Changed - User needs to switch networks -->
		<header class="dialog-header">
			<div class="icon-row">
				<div class="icon-circle primary">
					<Wifi class="icon-md" style="color: var(--primary)" />
				</div>
			</div>
			<h2 class="dialog-title">Switching Networks</h2>
			<p class="dialog-description">
				Your scoreboard is connecting to "{targetStationSsid}".
			</p>
		</header>

		<div class="body-section">
			<div class="steps">
				<div class="step">
					<span class="step-number">1</span>
					<div class="step-content">
						<p class="step-text">Connect your device to:</p>
						<div class="code-block">
							<Wifi class="icon-sm" style="color: var(--muted-foreground)" />
							<span class="code-bold">{targetStationSsid}</span>
						</div>
					</div>
				</div>

				<div class="step">
					<span class="step-number">2</span>
					<div class="step-content">
						<p class="step-text">Then access your scoreboard at:</p>
						<div class="code-block spread">
							<span>http://{targetHostname}</span>
							<button class="btn-icon" onclick={() => copyToClipboard(`http://${targetHostname}`)}>
								{#if copiedUrl === `http://${targetHostname}`}
									<Check class="icon-sm" style="color: var(--color-ok)" />
								{:else}
									<Copy class="icon-sm" />
								{/if}
							</button>
						</div>
					</div>
				</div>
			</div>

			<div class="alert warning" role="alert">
				<AlertTriangle class="icon-alert" style="color: var(--color-warn)" />
				<p>
					If the connection fails, device will create a
					<span class="bold">"{targetApSsid}"</span> network for setup.
				</p>
			</div>
		</div>

		<footer class="dialog-footer">
			<button class="btn default" onclick={() => rebootStore.redirectToTarget()}>
				I'm Connected
			</button>
		</footer>
	{:else if rebootStore.state === 'password_changed'}
		<!-- Password Changed - Might succeed or fail -->
		<header class="dialog-header">
			<div class="icon-row">
				<div class="icon-circle primary">
					<Wifi class="icon-md" style="color: var(--primary)" />
				</div>
			</div>
			<h2 class="dialog-title">WiFi Password Changed</h2>
			<p class="dialog-description">
				Your scoreboard is reconnecting with the new password.
			</p>
		</header>

		<div class="body-section">
			<div class="outcome-box">
				<div>
					<p class="outcome-label">If successful:</p>
					<p class="outcome-sub">
						Access your scoreboard at{#if fallbackIp}&nbsp;(or http://{fallbackIp}){/if}:
					</p>
					<div class="code-block spread">
						<span>http://{targetHostname}</span>
						<button class="btn-icon" onclick={() => copyToClipboard(`http://${targetHostname}`)}>
							{#if copiedUrl === `http://${targetHostname}`}
								<Check class="icon-sm" style="color: var(--color-ok)" />
							{:else}
								<Copy class="icon-sm" />
							{/if}
						</button>
					</div>
				</div>

				<div class="outcome-divider">
					<p class="outcome-label">If password is incorrect:</p>
					<p class="outcome-sub">
						Connect to the <span class="bold">"{targetApSsid}"</span> network and open:
					</p>
					<div class="code-block spread">
						<span>{rebootStore.targetApUrl}</span>
						<button class="btn-icon" onclick={() => copyToClipboard(rebootStore.targetApUrl)}>
							{#if copiedUrl === rebootStore.targetApUrl}
								<Check class="icon-sm" style="color: var(--color-ok)" />
							{:else}
								<Copy class="icon-sm" />
							{/if}
						</button>
					</div>
				</div>
			</div>
		</div>

		<footer class="dialog-footer">
			<button class="btn default" onclick={() => rebootStore.redirectToTarget()}>
				I'm Connected
			</button>
		</footer>
	{:else if rebootStore.state === 'redirecting'}
		<!-- Redirecting to new address -->
		<div class="center-content">
			<Loader2 class="spinner icon-lg" style="color: var(--primary)" />
			<p class="title">Redirecting...</p>
			<p class="description">
				Taking you to {rebootStore.targetUrl}
			</p>
		</div>
	{:else if rebootStore.state === 'connected'}
		<!-- Device is back online -->
		<div class="center-content">
			<div class="icon-circle ok">
				<CheckCircle2 class="icon-lg" style="color: var(--color-ok)" />
			</div>
			<p class="title">Device is back online!</p>
			<p class="description">
				Refreshing in {rebootStore.countdownSeconds}...
			</p>
			<button class="btn outline" onclick={() => rebootStore.refreshNow()}>
				Refresh Now
			</button>
		</div>
	{:else if rebootStore.state === 'timeout'}
		<!-- Timeout - device didn't respond -->
		<div class="center-content">
			<div class="icon-circle warn">
				<AlertTriangle class="icon-lg" style="color: var(--color-warn)" />
			</div>
			<p class="title">Device not responding</p>
			<p class="description">
				The device didn't respond within 2 minutes. It may still be rebooting, or the network
				configuration may have changed.
			</p>
			<div class="button-row">
				<button class="btn outline" onclick={() => rebootStore.retry()}>Try Again</button>
				<button class="btn default" onclick={() => rebootStore.close()}>Close</button>
			</div>
		</div>
	{:else if rebootStore.state === 'error'}
		<!-- Error state -->
		<div class="center-content">
			<div class="icon-circle destructive">
				<AlertTriangle class="icon-lg" style="color: var(--destructive)" />
			</div>
			<p class="title">Reboot failed</p>
			<p class="description">
				{rebootStore.errorMessage ?? 'An unknown error occurred.'}
			</p>
			<button class="btn default" onclick={() => rebootStore.close()}>Close</button>
		</div>
	{/if}
</dialog>

<style>
	/* .dialog, .icon-circle, .alert.warning, progress, and .btn all come from
	   the shared styles in app.css. Only overlay-specific layout lives here. */

	/* Layout helpers */
	.center-content {
		display: flex;
		flex-direction: column;
		align-items: center;
		padding: 1.5rem 0;
		text-align: center;
	}

	.center-row {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
	}

	.center-text {
		text-align: center;
	}

	/* Header */
	.dialog-header {
		text-align: center;
	}

	.dialog-title {
		font-size: 1.125rem;
		font-weight: 600;
		line-height: 1.4;
		margin-top: 0.5rem;
	}

	.dialog-description {
		font-size: 0.875rem;
		color: var(--muted-foreground);
		margin-top: 0.25rem;
	}

	/* Footer */
	.dialog-footer {
		display: flex;
		justify-content: center;
		margin-top: 1rem;
	}

	/* Icon patterns */
	.icon-row {
		display: flex;
		justify-content: center;
	}

	/* Icons (sized via global since lucide uses class) */
	.dialog :global(.icon-sm) {
		width: 1rem;
		height: 1rem;
		flex-shrink: 0;
	}

	.dialog :global(.icon-md) {
		width: 2rem;
		height: 2rem;
	}

	.dialog :global(.icon-lg) {
		width: 3rem;
		height: 3rem;
	}

	.dialog :global(.spinner) {
		animation: spin 1s linear infinite;
	}

	/* Typography */
	.title {
		font-size: 1.125rem;
		font-weight: 500;
		margin-top: 1rem;
	}

	.description {
		font-size: 0.875rem;
		color: var(--muted-foreground);
		margin-top: 0.5rem;
	}

	.muted-text {
		font-size: 0.875rem;
		color: var(--muted-foreground);
	}

	.bold {
		font-weight: 500;
	}

	/* Body section */
	.body-section {
		display: flex;
		flex-direction: column;
		gap: 1rem;
		padding: 1rem 0;
	}

	/* Steps */
	.steps {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}

	.step {
		display: flex;
		align-items: flex-start;
		gap: 0.75rem;
	}

	.step-number {
		display: flex;
		width: 1.5rem;
		height: 1.5rem;
		flex-shrink: 0;
		align-items: center;
		justify-content: center;
		border-radius: 50%;
		background: var(--muted);
		font-size: 0.875rem;
		font-weight: 500;
	}

	.step-content {
		flex: 1;
		min-width: 0;
	}

	.step-text {
		font-size: 0.875rem;
	}

	/* Code blocks */
	.code-block {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		margin-top: 0.25rem;
		border: 1px solid var(--border);
		background: oklch(from var(--muted) l c h / 50%);
		border-radius: 0.375rem;
		padding: 0.5rem 0.75rem;
		font-family: ui-monospace, monospace;
		font-size: 0.875rem;

		&.spread {
			justify-content: space-between;
		}
	}

	.code-bold {
		font-weight: 500;
	}

	/* Progress */
	.progress-section {
		width: 100%;
		margin-top: 1.5rem;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
	}

	.progress-info {
		display: flex;
		align-items: center;
		justify-content: space-between;
		font-size: 0.875rem;
		color: var(--muted-foreground);
	}

	/* Outcome box (password changed state) */
	.outcome-box {
		display: flex;
		flex-direction: column;
		gap: 1rem;
		border: 1px solid var(--border);
		border-radius: 0.375rem;
		padding: 1rem;
	}

	.outcome-label {
		font-size: 0.875rem;
		font-weight: 500;
	}

	.outcome-sub {
		font-size: 0.875rem;
		color: var(--muted-foreground);
		margin-top: 0.25rem;
	}

	.outcome-divider {
		border-top: 1px solid var(--border);
		padding-top: 1rem;
	}

	.btn-icon {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 1.5rem;
		height: 1.5rem;
		padding: 0;
		border: none;
		background: transparent;
		cursor: pointer;
		border-radius: 0.25rem;
		color: var(--muted-foreground);
		flex-shrink: 0;

		&:hover {
			background: var(--accent);
		}
	}

	.button-row {
		display: flex;
		gap: 0.5rem;
		margin-top: 1.5rem;
	}
</style>

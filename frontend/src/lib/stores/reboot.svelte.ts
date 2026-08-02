import { picoApi, type Config, type NetworkStatus } from '$lib/api';

// Reboot flow states
export type RebootState =
	| 'idle'
	| 'initiating'
	| 'polling'
	| 'setup_complete'
	| 'hostname_changed'
	| 'network_reset'
	| 'ssid_changed'
	| 'password_changed'
	| 'redirecting'
	| 'connected'
	| 'timeout'
	| 'error';

export type RebootScenario =
	| 'same_connection'
	| 'setup_complete'
	| 'hostname_changed'
	| 'network_reset'
	| 'ssid_changed'
	| 'password_changed';

// Polling configuration
const POLLING_CONFIG = {
	initialDelayMs: 1000, // Start at 1 second
	maxDelayMs: 10000, // Cap at 10 seconds
	totalTimeoutMs: 120000, // Give up after 2 minutes
	backoffMultiplier: 1.5, // Increase by 50% each time
	requestTimeoutMs: 5000 // Each request times out after 5 seconds
};

/**
 * Determine which reboot scenario applies, by comparing the config the
 * device WAS running (original) against the config it will boot with
 * (updated). This is the single scenario resolver — both the settings page
 * and the setup page go through it, so an SSID change is detected the same
 * way everywhere (a previous setup-page-only resolver missed ssid/password
 * changes and polled the old, unreachable address for the full timeout).
 */
export function determineRebootScenario(
	status: NetworkStatus,
	original: Config,
	updated: Config
): RebootScenario {
	// In setup (AP) mode with an SSID configured, the reboot completes setup:
	// the device leaves the AP and the browser is stranded on it.
	if (status.setup_mode && updated.network.ssid) {
		return 'setup_complete';
	}

	// SSID change takes priority - user must switch networks
	if (updated.network.ssid !== original.network.ssid) {
		return 'ssid_changed';
	}

	// Password-only change - risky, might fail
	if (updated.network.password !== original.network.password) {
		return 'password_changed';
	}

	// Hostname-only change - redirect to new address
	if (updated.network.device_name !== original.network.device_name) {
		return 'hostname_changed';
	}

	return 'same_connection';
}

/**
 * Get the URL to redirect to after reboot based on the target config
 */
function getTargetUrl(config: Config): string {
	// Always target station mode URL (no more AP mode as destination)
	return `http://${config.network.device_name}.local`;
}

/**
 * Get the AP mode URL (used when device enters setup mode).
 * Must match the Pico W's AP interface address (see firmware main.py / dns.py).
 */
const AP_URL = 'http://192.168.4.1';

function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Poll for device reconnection with exponential backoff.
 * Each request has a short timeout so we don't hang waiting for unreachable devices.
 */
async function pollForReconnection(
	onAttempt: (attempt: number) => void,
	signal?: AbortSignal
): Promise<{ success: boolean; status?: NetworkStatus }> {
	let delay = POLLING_CONFIG.initialDelayMs;
	let attempt = 0;
	const startTime = Date.now();

	while (Date.now() - startTime < POLLING_CONFIG.totalTimeoutMs) {
		if (signal?.aborted) return { success: false };

		// Wait before attempting (gives device time to reboot)
		await sleep(delay);

		if (signal?.aborted) return { success: false };

		attempt++;
		onAttempt(attempt);

		try {
			// Use a short timeout per request so we don't hang
			const status = await picoApi.getStatus({
				timeoutMs: POLLING_CONFIG.requestTimeoutMs,
				signal
			});
			return { success: true, status };
		} catch (e) {
			// Check if we were aborted
			if (signal?.aborted) return { success: false };
			// Otherwise, device not ready - continue polling
		}

		// Exponential backoff for next attempt
		delay = Math.min(delay * POLLING_CONFIG.backoffMultiplier, POLLING_CONFIG.maxDelayMs);
	}

	return { success: false };
}

function createRebootStore() {
	let state = $state<RebootState>('idle');
	let scenario = $state<RebootScenario | null>(null);
	let targetConfig = $state<Config | null>(null);
	let preRebootStatus = $state<NetworkStatus | null>(null);
	let attemptNumber = $state(0);
	let errorMessage = $state<string | null>(null);
	let countdownSeconds = $state(0);

	let abortController: AbortController | null = null;
	let countdownInterval: ReturnType<typeof setInterval> | null = null;

	const isActive = $derived(state !== 'idle');

	// Estimate max attempts based on polling config
	const maxAttempts = $derived.by(() => {
		let total = 0;
		let delay = POLLING_CONFIG.initialDelayMs;
		let elapsed = 0;
		while (elapsed < POLLING_CONFIG.totalTimeoutMs) {
			elapsed += delay;
			total++;
			delay = Math.min(delay * POLLING_CONFIG.backoffMultiplier, POLLING_CONFIG.maxDelayMs);
		}
		return total;
	});

	function clearCountdown() {
		if (countdownInterval) {
			clearInterval(countdownInterval);
			countdownInterval = null;
		}
	}

	function startCountdown(seconds: number, onComplete: () => void) {
		countdownSeconds = seconds;
		clearCountdown();
		countdownInterval = setInterval(() => {
			countdownSeconds--;
			if (countdownSeconds <= 0) {
				clearCountdown();
				onComplete();
			}
		}, 1000);
	}

	// Computed: target URL based on config
	const targetUrl = $derived(targetConfig ? getTargetUrl(targetConfig) : null);

	// AP URL for setup/reset scenarios
	const targetApUrl = AP_URL;

	return {
		// Getters
		get state() {
			return state;
		},
		get scenario() {
			return scenario;
		},
		get targetConfig() {
			return targetConfig;
		},
		get preRebootStatus() {
			return preRebootStatus;
		},
		get attemptNumber() {
			return attemptNumber;
		},
		get maxAttempts() {
			return maxAttempts;
		},
		get errorMessage() {
			return errorMessage;
		},
		get isActive() {
			return isActive;
		},
		get countdownSeconds() {
			return countdownSeconds;
		},
		get targetUrl() {
			return targetUrl;
		},
		get targetApUrl() {
			return targetApUrl;
		},

		/**
		 * Initiate reboot with context for handling reconnection.
		 * The scenario is derived from what changed between the config the
		 * device was running (original) and the config it will boot with.
		 */
		async initiateReboot(
			currentStatus: NetworkStatus,
			originalConfig: Config,
			updatedConfig: Config
		) {
			const detectedScenario = determineRebootScenario(
				currentStatus,
				originalConfig,
				updatedConfig
			);
			await this.initiateRebootWithScenario(currentStatus, updatedConfig, detectedScenario);
		},

		/**
		 * Initiate reboot with an explicit scenario.
		 * Use this when you know the scenario ahead of time (e.g., network reset).
		 */
		async initiateRebootWithScenario(
			currentStatus: NetworkStatus,
			currentConfig: Config,
			explicitScenario: RebootScenario
		) {
			// Capture pre-reboot state
			preRebootStatus = currentStatus;
			targetConfig = currentConfig;
			scenario = explicitScenario;
			attemptNumber = 0;
			errorMessage = null;

			state = 'initiating';

			try {
				await picoApi.reboot();

				// Transition based on scenario
				switch (scenario) {
					case 'same_connection':
						state = 'polling';
						this.startPolling();
						break;

					case 'setup_complete':
						// Completing setup - switching from AP to station mode
						state = 'setup_complete';
						break;

					case 'hostname_changed':
						// Hostname is changing but we're on the same network
						// Auto-redirect after giving the device time to reboot
						state = 'hostname_changed';
						startCountdown(15, () => {
							this.redirectToTarget();
						});
						break;

					case 'network_reset':
						// Network credentials cleared - device will enter AP mode
						state = 'network_reset';
						break;

					case 'ssid_changed':
						// SSID changed - user needs to switch networks
						state = 'ssid_changed';
						break;

					case 'password_changed':
						// Password changed - might succeed or fail
						state = 'password_changed';
						break;
				}
			} catch (e) {
				errorMessage = e instanceof Error ? e.message : 'Failed to initiate reboot';
				state = 'error';
			}
		},

		/**
		 * Start polling for device reconnection
		 */
		async startPolling() {
			abortController = new AbortController();
			state = 'polling';
			attemptNumber = 1; // Start at 1 so progress bar shows initial progress

			const result = await pollForReconnection((attempt) => {
				attemptNumber = attempt;
			}, abortController.signal);

			if (result.success) {
				state = 'connected';
				// Auto-refresh after 3 seconds
				startCountdown(3, () => {
					window.location.reload();
				});
			} else if (!abortController.signal.aborted) {
				state = 'timeout';
			}
		},

		/**
		 * User clicked "I'm connected" - redirect to the new address
		 * (for switching scenarios where we can't poll the old address)
		 */
		userConfirmedConnection() {
			this.redirectToTarget();
		},

		/**
		 * Retry from timeout state
		 */
		async retry() {
			attemptNumber = 0;
			await this.startPolling();
		},

		/**
		 * Redirect to the target URL (new device address)
		 */
		redirectToTarget() {
			clearCountdown();
			if (targetUrl) {
				state = 'redirecting';
				window.location.href = targetUrl;
			}
		},

		/**
		 * Redirect to the AP URL (for network reset scenarios)
		 */
		redirectToAp() {
			clearCountdown();
			state = 'redirecting';
			window.location.href = targetApUrl;
		},

		/**
		 * Close the overlay (from timeout/error/connected state)
		 */
		close() {
			if (abortController) {
				abortController.abort();
				abortController = null;
			}
			clearCountdown();
			state = 'idle';
			scenario = null;
			targetConfig = null;
			preRebootStatus = null;
			attemptNumber = 0;
			errorMessage = null;
		},

		/**
		 * Immediately refresh the page
		 */
		refreshNow() {
			clearCountdown();
			window.location.reload();
		}
	};
}

export const rebootStore = createRebootStore();

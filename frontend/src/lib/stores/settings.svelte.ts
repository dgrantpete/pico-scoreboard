import { picoApi, type Config, type ConfigUpdate, type NetworkStatus } from '$lib/api';
import { rebootStore, determineRebootScenario } from './reboot.svelte';

/**
 * Extract a nested value from config using a dot-notation path
 */
function getValueByPath(obj: Config, path: string): unknown {
	const parts = path.split('.');
	let current: unknown = obj;
	for (const part of parts) {
		if (current && typeof current === 'object' && part in current) {
			current = (current as Record<string, unknown>)[part];
		} else {
			return undefined;
		}
	}
	return current;
}

/**
 * Build a ConfigUpdate object from the current config and touched fields.
 * Sections are generic: any "section.field" touched path lands in the
 * partial update, so new config sections need no code here.
 */
function buildUpdateFromTouched(config: Config, touchedFields: Set<string>): ConfigUpdate {
	const update: Record<string, Record<string, unknown>> = {};

	for (const path of touchedFields) {
		const [section, field] = path.split('.');
		if (!(section in config)) continue;
		(update[section] ??= {})[field] = getValueByPath(config, path);
	}

	return update as ConfigUpdate;
}

/**
 * Sections whose settings only take effect at boot. `network.*` (WiFi
 * reconnect), `watchdog.*` (the hardware WDT is armed once at startup), and
 * `sports.*` (the game poller builds its league sources once at startup) —
 * saving these must prompt for a reboot or the change silently does nothing.
 */
function needsRebootToApply(touchedFields: Set<string>): boolean {
	for (const path of touchedFields) {
		if (
			path.startsWith('network.') ||
			path.startsWith('watchdog.') ||
			path.startsWith('sports.')
		) {
			return true;
		}
	}
	return false;
}

export function createSettingsStore() {
	// Current config (working copy)
	let config = $state<Config | null>(null);

	// Original config (snapshot from last load/save, used to detect changes)
	let originalConfig = $state<Config | null>(null);

	// Network status
	let status = $state<NetworkStatus | null>(null);

	// Set of touched field paths (e.g., "network.ssid", "display.brightness")
	let touchedFields = $state<Set<string>>(new Set());

	// Loading states
	let isLoading = $state(false);
	let isSaving = $state(false);

	// Error state
	let error = $state<string | null>(null);

	// Whether we just saved network changes (triggers reboot prompt)
	let showRebootPrompt = $state(false);

	// Single status poller — the status card and the memory/flash meters all
	// read `status`, so one interval serves every consumer.
	let statusPollInterval: ReturnType<typeof setInterval> | null = null;

	// Computed: whether any field has been touched
	const isDirty = $derived(touchedFields.size > 0);

	// Computed: the pending changes to send
	const pendingChanges = $derived(config ? buildUpdateFromTouched(config, touchedFields) : {});

	return {
		// Getters
		get config() {
			return config;
		},
		get status() {
			return status;
		},
		get isLoading() {
			return isLoading;
		},
		get isSaving() {
			return isSaving;
		},
		get error() {
			return error;
		},
		get isDirty() {
			return isDirty;
		},
		get pendingChanges() {
			return pendingChanges;
		},
		get showRebootPrompt() {
			return showRebootPrompt;
		},

		/**
		 * Mark a field as touched (dirty)
		 */
		markTouched(path: string) {
			touchedFields = new Set(touchedFields).add(path);
		},

		/**
		 * Update a network field and mark it as touched
		 */
		updateNetwork<K extends keyof Config['network']>(key: K, value: Config['network'][K]) {
			if (config) {
				config.network[key] = value;
				this.markTouched(`network.${key}`);
			}
		},

		/**
		 * Update an API field and mark it as touched
		 */
		updateApi<K extends keyof Config['api']>(key: K, value: Config['api'][K]) {
			if (config) {
				config.api[key] = value;
				this.markTouched(`api.${key}`);
			}
		},

		/**
		 * Update a display field and mark it as touched
		 */
		updateDisplay<K extends keyof Config['display']>(key: K, value: Config['display'][K]) {
			if (config) {
				config.display[key] = value;
				this.markTouched(`display.${key}`);
			}
		},

		/**
		 * Update a colors field and mark it as touched
		 */
		updateColors<K extends keyof Config['colors']>(key: K, value: Config['colors'][K]) {
			if (config) {
				config.colors[key] = value;
				this.markTouched(`colors.${key}`);
			}
		},

		/**
		 * Update a server field and mark it as touched
		 */
		updateServer<K extends keyof Config['server']>(key: K, value: Config['server'][K]) {
			if (config) {
				config.server[key] = value;
				this.markTouched(`server.${key}`);
			}
		},

		/**
		 * Update a watchdog field and mark it as touched
		 */
		updateWatchdog<K extends keyof Config['watchdog']>(key: K, value: Config['watchdog'][K]) {
			if (config) {
				config.watchdog[key] = value;
				this.markTouched(`watchdog.${key}`);
			}
		},

		/**
		 * Update a log field and mark it as touched
		 */
		updateLog<K extends keyof Config['log']>(key: K, value: Config['log'][K]) {
			if (config) {
				config.log[key] = value;
				this.markTouched(`log.${key}`);
			}
		},

		/**
		 * Update an OTA field and mark it as touched
		 */
		updateOta<K extends keyof Config['ota']>(key: K, value: Config['ota'][K]) {
			if (config) {
				config.ota[key] = value;
				this.markTouched(`ota.${key}`);
			}
		},

		/**
		 * Update a sports sub-config and mark it as touched. The touched path
		 * is two-level (`sports.<sport>`) on purpose: the update batcher
		 * splits paths into exactly [section, field], so the whole
		 * sub-object ships in the PUT body.
		 */
		updateSports<K extends keyof Config['sports']>(key: K, value: Config['sports'][K]) {
			if (config) {
				config.sports[key] = value;
				this.markTouched(`sports.${key}`);
			}
		},

		/**
		 * Load config and status from API
		 */
		async load() {
			isLoading = true;
			error = null;
			touchedFields = new Set();
			showRebootPrompt = false;

			try {
				const [configData, statusData] = await Promise.all([
					picoApi.getConfig(),
					picoApi.getStatus()
				]);
				config = configData;
				// Deep clone to preserve original values for comparison
				originalConfig = JSON.parse(JSON.stringify(configData));
				status = statusData;
			} catch (e) {
				error = e instanceof Error ? e.message : 'Failed to load configuration';
			} finally {
				isLoading = false;
			}
		},

		/**
		 * Save only touched fields to API
		 */
		async save() {
			if (!config || touchedFields.size === 0) return;

			const rebootRequired = needsRebootToApply(touchedFields);
			const changes = buildUpdateFromTouched(config, touchedFields);

			isSaving = true;
			error = null;

			try {
				const updatedConfig = await picoApi.updateConfig(changes);
				config = updatedConfig;
				touchedFields = new Set();

				// Prompt for reboot when boot-time-only settings changed
				// (network credentials, hardware watchdog, sports selection)
				if (rebootRequired) {
					showRebootPrompt = true;
				}
			} catch (e) {
				error = e instanceof Error ? e.message : 'Failed to save configuration';
			} finally {
				isSaving = false;
			}
		},

		/**
		 * Discard changes by reloading from API
		 */
		async discard() {
			await this.load();
		},

		/**
		 * Dismiss the reboot prompt
		 */
		dismissRebootPrompt() {
			showRebootPrompt = false;
		},

		/**
		 * Refresh status only
		 */
		async refreshStatus() {
			try {
				status = await picoApi.getStatus({ timeoutMs: 4000 });
			} catch (e) {
				console.error('Failed to refresh status:', e);
			}
		},

		/**
		 * Start polling /api/status on an interval (no-op if already polling).
		 * Skips ticks while a reboot flow is active (the reboot store runs its
		 * own reconnect poller) and while the tab is hidden.
		 */
		startStatusPolling(intervalMs = 5000) {
			if (statusPollInterval) return;
			this.refreshStatus();
			statusPollInterval = setInterval(() => {
				if (document.hidden || rebootStore.isActive) return;
				this.refreshStatus();
			}, intervalMs);
		},

		/**
		 * Stop the status polling interval
		 */
		stopStatusPolling() {
			if (statusPollInterval) {
				clearInterval(statusPollInterval);
				statusPollInterval = null;
			}
		},

		/**
		 * Reboot device - delegates to reboot store for graceful handling.
		 * The scenario comes from the shared resolver, comparing the config
		 * snapshot from load/save time against the current one.
		 */
		async reboot() {
			if (!config || !originalConfig || !status) return;
			showRebootPrompt = false;

			const scenario = determineRebootScenario(status, originalConfig, config);
			await rebootStore.initiateRebootWithScenario(status, config, scenario);
		},

		/**
		 * Clear error state
		 */
		clearError() {
			error = null;
		}
	};
}

// Singleton instance for the app
export const settingsStore = createSettingsStore();

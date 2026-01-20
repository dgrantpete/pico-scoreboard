import { picoApi, type Config, type ConfigUpdate, type NetworkStatus } from '$lib/api';
import { rebootStore } from './reboot.svelte';

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
 * Build a ConfigUpdate object from the current config and touched fields
 */
function buildUpdateFromTouched(config: Config, touchedFields: Set<string>): ConfigUpdate {
	const update: ConfigUpdate = {};

	for (const path of touchedFields) {
		const [section, field] = path.split('.') as [keyof Config, string];
		const value = getValueByPath(config, path);

		if (section === 'network') {
			update.network = update.network || {};
			(update.network as Record<string, unknown>)[field] = value;
		} else if (section === 'api') {
			update.api = update.api || {};
			(update.api as Record<string, unknown>)[field] = value;
		} else if (section === 'display') {
			update.display = update.display || {};
			(update.display as Record<string, unknown>)[field] = value;
		} else if (section === 'server') {
			update.server = update.server || {};
			(update.server as Record<string, unknown>)[field] = value;
		}
	}

	return update;
}

/**
 * Check if any touched field is in the network section
 */
function hasNetworkChanges(touchedFields: Set<string>): boolean {
	for (const path of touchedFields) {
		if (path.startsWith('network.')) {
			return true;
		}
	}
	return false;
}

export function createSettingsStore() {
	// Current config (working copy)
	let config = $state<Config | null>(null);

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
		 * Update a server field and mark it as touched
		 */
		updateServer<K extends keyof Config['server']>(key: K, value: Config['server'][K]) {
			if (config) {
				config.server[key] = value;
				this.markTouched(`server.${key}`);
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

			const hadNetworkChanges = hasNetworkChanges(touchedFields);
			const changes = buildUpdateFromTouched(config, touchedFields);

			isSaving = true;
			error = null;

			try {
				const updatedConfig = await picoApi.updateConfig(changes);
				config = updatedConfig;
				touchedFields = new Set();

				// Show reboot prompt if network settings were changed
				if (hadNetworkChanges) {
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
				status = await picoApi.getStatus();
			} catch (e) {
				console.error('Failed to refresh status:', e);
			}
		},

		/**
		 * Reboot device - delegates to reboot store for graceful handling
		 */
		async reboot() {
			if (!config || !status) return;
			showRebootPrompt = false;
			await rebootStore.initiateReboot(status, config);
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

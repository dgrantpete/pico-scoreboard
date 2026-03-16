import { picoApi, type NetworkStatus } from '$lib/api';

const POLL_INTERVAL_MS = 5000;

function createMemoryTelemetryStore() {
	let latestStatus = $state<NetworkStatus | null>(null);
	let isPolling = $state(false);
	let error = $state<string | null>(null);

	let pollInterval: ReturnType<typeof setInterval> | null = null;

	const currentPercent = $derived(() => {
		if (!latestStatus) return 0;
		const total = latestStatus.memory_used + latestStatus.memory_free;
		return total > 0 ? Math.round((latestStatus.memory_used / total) * 100) : 0;
	});

	async function fetchAndRecord(): Promise<void> {
		try {
			const status = await picoApi.getStatus({ timeoutMs: 4000 });
			latestStatus = status;
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to fetch status';
		}
	}

	return {
		get latestStatus() {
			return latestStatus;
		},
		get isPolling() {
			return isPolling;
		},
		get error() {
			return error;
		},
		get currentPercent() {
			return currentPercent;
		},

		startPolling() {
			if (isPolling) return;

			isPolling = true;
			fetchAndRecord();
			pollInterval = setInterval(fetchAndRecord, POLL_INTERVAL_MS);
		},

		stopPolling() {
			if (pollInterval) {
				clearInterval(pollInterval);
				pollInterval = null;
			}
			isPolling = false;
		},

		seedFromStatus(status: NetworkStatus) {
			latestStatus = status;
		}
	};
}

// Singleton export
export const memoryTelemetryStore = createMemoryTelemetryStore();

import { picoApi, type NetworkStatus } from '$lib/api';

/**
 * A single memory telemetry data point
 */
export interface MemoryDataPoint {
	timestamp: Date;
	memoryUsed: number;
	memoryFree: number;
	memoryPercent: number;
}

const MAX_DATA_POINTS = 2000;
const POLL_INTERVAL_MS = 5000;

function createMemoryTelemetryStore() {
	// Time-series data buffer
	let dataPoints = $state<MemoryDataPoint[]>([]);

	// Latest status (for current values display)
	let latestStatus = $state<NetworkStatus | null>(null);

	// Polling state
	let isPolling = $state(false);
	let error = $state<string | null>(null);

	// Internal polling handle
	let pollInterval: ReturnType<typeof setInterval> | null = null;

	// Computed: memory percentage from latest
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

			const total = status.memory_used + status.memory_free;
			const percent = total > 0 ? Math.round((status.memory_used / total) * 100) : 0;

			const newPoint: MemoryDataPoint = {
				timestamp: new Date(),
				memoryUsed: status.memory_used,
				memoryFree: status.memory_free,
				memoryPercent: percent
			};

			// Append and trim to max size
			dataPoints = [...dataPoints, newPoint].slice(-MAX_DATA_POINTS);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to fetch status';
		}
	}

	return {
		// Getters
		get dataPoints() {
			return dataPoints;
		},
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

		/**
		 * Get data points within a time range
		 */
		getDataInRange(startTime: Date, endTime: Date): MemoryDataPoint[] {
			return dataPoints.filter((p) => p.timestamp >= startTime && p.timestamp <= endTime);
		},

		/**
		 * Get data points for the last N minutes
		 */
		getLastMinutes(minutes: number): MemoryDataPoint[] {
			const cutoff = new Date(Date.now() - minutes * 60 * 1000);
			return dataPoints.filter((p) => p.timestamp >= cutoff);
		},

		/**
		 * Start polling for memory data.
		 * Safe to call multiple times - will not create duplicate intervals.
		 */
		startPolling() {
			if (isPolling) return;

			isPolling = true;
			// Fetch immediately, then on interval
			fetchAndRecord();
			pollInterval = setInterval(fetchAndRecord, POLL_INTERVAL_MS);
		},

		/**
		 * Stop polling.
		 */
		stopPolling() {
			if (pollInterval) {
				clearInterval(pollInterval);
				pollInterval = null;
			}
			isPolling = false;
		},

		/**
		 * Clear all collected data points.
		 */
		clearData() {
			dataPoints = [];
		},

		/**
		 * Initialize with existing status (e.g., from settingsStore).
		 * Useful to seed initial data point without waiting for first poll.
		 */
		seedFromStatus(status: NetworkStatus) {
			latestStatus = status;
			const total = status.memory_used + status.memory_free;
			const percent = total > 0 ? Math.round((status.memory_used / total) * 100) : 0;

			if (dataPoints.length === 0) {
				dataPoints = [
					{
						timestamp: new Date(),
						memoryUsed: status.memory_used,
						memoryFree: status.memory_free,
						memoryPercent: percent
					}
				];
			}
		}
	};
}

// Singleton export
export const memoryTelemetryStore = createMemoryTelemetryStore();

import type { Config, ConfigUpdate, LogEntry, NetworkStatus, RebootResponse } from './types';

class ApiError extends Error {
	status: number;

	constructor(message: string, status: number) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
	}
}

async function handleResponse<T>(response: Response): Promise<T> {
	if (!response.ok) {
		throw new ApiError(`HTTP ${response.status}: ${response.statusText}`, response.status);
	}
	return response.json();
}

/**
 * Create an AbortSignal that combines a timeout with an optional external signal.
 * The request will abort if either the timeout expires or the external signal aborts.
 */
function createTimeoutSignal(timeoutMs: number, externalSignal?: AbortSignal): AbortSignal {
	if (!externalSignal) {
		return AbortSignal.timeout(timeoutMs);
	}

	// Combine timeout and external signal using AbortSignal.any()
	return AbortSignal.any([AbortSignal.timeout(timeoutMs), externalSignal]);
}

export const picoApi = {
	/**
	 * GET /api/config - Fetch full device configuration
	 */
	async getConfig(signal?: AbortSignal): Promise<Config> {
		const response = await fetch('/api/config', { signal });
		return handleResponse<Config>(response);
	},

	/**
	 * PUT /api/config - Merge update configuration
	 * Only fields present in the update are changed; omitted fields remain unchanged.
	 */
	async updateConfig(update: ConfigUpdate, signal?: AbortSignal): Promise<Config> {
		const response = await fetch('/api/config', {
			method: 'PUT',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify(update),
			signal
		});
		return handleResponse<Config>(response);
	},

	/**
	 * GET /api/status - Get current network status
	 * @param timeoutMs - Optional timeout in milliseconds (default: no timeout)
	 * @param signal - Optional AbortSignal for cancellation
	 */
	async getStatus(options?: { timeoutMs?: number; signal?: AbortSignal }): Promise<NetworkStatus> {
		const fetchSignal = options?.timeoutMs
			? createTimeoutSignal(options.timeoutMs, options.signal)
			: options?.signal;

		const response = await fetch('/api/status', { signal: fetchSignal });
		return handleResponse<NetworkStatus>(response);
	},

	/**
	 * POST /api/reboot - Trigger device restart
	 * Device will reboot after a 1-second delay.
	 */
	async reboot(signal?: AbortSignal): Promise<RebootResponse> {
		const response = await fetch('/api/reboot', { method: 'POST', signal });
		return handleResponse<RebootResponse>(response);
	},

	/**
	 * POST /api/reset-network - Clear network credentials
	 * Clears SSID and password to trigger fresh setup mode on next boot.
	 */
	async resetNetwork(signal?: AbortSignal): Promise<{ message: string }> {
		const response = await fetch('/api/reset-network', { method: 'POST', signal });
		return handleResponse<{ message: string }>(response);
	},

	/**
	 * GET /api/logs?since=<seq> - Device log ring as NDJSON.
	 * Returns entries with seq > since; use the last entry's seq as the
	 * next `since` for tail-follow polling.
	 */
	async getLogs(since = 0, signal?: AbortSignal): Promise<LogEntry[]> {
		const response = await fetch(`/api/logs?since=${since}`, { signal });
		if (!response.ok) {
			throw new ApiError(`HTTP ${response.status}: ${response.statusText}`, response.status);
		}
		const text = await response.text();
		return text
			.split('\n')
			.filter((line) => line.length > 0)
			.map((line) => JSON.parse(line) as LogEntry);
	},

	/**
	 * GET /api/logs/previous - Previous boot's flushed log file (plain text).
	 * Returns null when no previous-boot log exists on flash.
	 */
	async getPreviousLog(signal?: AbortSignal): Promise<string | null> {
		const response = await fetch('/api/logs/previous', { signal });
		if (response.status === 404) return null;
		if (!response.ok) {
			throw new ApiError(`HTTP ${response.status}: ${response.statusText}`, response.status);
		}
		return response.text();
	},

};

import type { Config, ConfigUpdate, NetworkStatus, RebootResponse } from './types';

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
	}
};

import type { Config, ConfigUpdate, LogEntry, NetworkStatus, RebootResponse } from './types';

// Every request gets a finite timeout: the device can silently drop
// connections (tiny lwip socket pool, reboots, WiFi drops) and a hung fetch
// would otherwise stall the UI forever. Callers can override per request.
const DEFAULT_TIMEOUT_MS = 8000;

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
		throw new ApiError(await errorMessage(response), response.status);
	}
	return response.json();
}

/**
 * Build an error message from a failed response, preferring the firmware's
 * JSON error body ({ error, message }) over the bare status line — e.g. the
 * config cadence-validation message from PUT /api/config.
 */
async function errorMessage(response: Response): Promise<string> {
	const fallback = `HTTP ${response.status}: ${response.statusText}`;
	try {
		const body = await response.json();
		if (body && typeof body === 'object' && (body.message || body.error)) {
			return String(body.message || body.error);
		}
	} catch {
		// Non-JSON body — keep the status line
	}
	return fallback;
}

/**
 * fetch with a timeout and an optional external abort signal.
 *
 * When OUR timeout fires, the raw DOMException reads "The user aborted a
 * request" — cryptic and wrong-sounding in a UI — so it's translated to a
 * readable ApiError. An external abort (e.g. the reboot store cancelling
 * its reconnect poll) propagates unchanged so callers can recognize it.
 */
async function apiFetch(
	input: string,
	init: RequestInit,
	timeoutMs: number,
	externalSignal?: AbortSignal
): Promise<Response> {
	const timeout = AbortSignal.timeout(timeoutMs);
	const signal = externalSignal ? AbortSignal.any([timeout, externalSignal]) : timeout;
	try {
		return await fetch(input, { ...init, signal });
	} catch (e) {
		if (timeout.aborted && !externalSignal?.aborted) {
			throw new ApiError('Device did not respond in time', 0);
		}
		throw e;
	}
}

export const picoApi = {
	/**
	 * GET /api/config - Fetch full device configuration
	 */
	async getConfig(signal?: AbortSignal): Promise<Config> {
		const response = await apiFetch('/api/config', {}, DEFAULT_TIMEOUT_MS, signal);
		return handleResponse<Config>(response);
	},

	/**
	 * PUT /api/config - Merge update configuration
	 * Only fields present in the update are changed; omitted fields remain unchanged.
	 */
	async updateConfig(update: ConfigUpdate, signal?: AbortSignal): Promise<Config> {
		const response = await apiFetch(
			'/api/config',
			{
				method: 'PUT',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(update)
			},
			DEFAULT_TIMEOUT_MS,
			signal
		);
		return handleResponse<Config>(response);
	},

	/**
	 * GET /api/status - Get current network status
	 * @param timeoutMs - Optional timeout override in milliseconds
	 * @param signal - Optional AbortSignal for cancellation
	 */
	async getStatus(options?: { timeoutMs?: number; signal?: AbortSignal }): Promise<NetworkStatus> {
		const response = await apiFetch(
			'/api/status',
			{},
			options?.timeoutMs ?? DEFAULT_TIMEOUT_MS,
			options?.signal
		);
		return handleResponse<NetworkStatus>(response);
	},

	/**
	 * POST /api/reboot - Trigger device restart
	 * Device will reboot after a 1-second delay.
	 */
	async reboot(signal?: AbortSignal): Promise<RebootResponse> {
		const response = await apiFetch('/api/reboot', { method: 'POST' }, DEFAULT_TIMEOUT_MS, signal);
		return handleResponse<RebootResponse>(response);
	},

	/**
	 * POST /api/reset-network - Clear network credentials
	 * Clears SSID and password to trigger fresh setup mode on next boot.
	 */
	async resetNetwork(signal?: AbortSignal): Promise<{ message: string }> {
		const response = await apiFetch(
			'/api/reset-network',
			{ method: 'POST' },
			DEFAULT_TIMEOUT_MS,
			signal
		);
		return handleResponse<{ message: string }>(response);
	},

	/**
	 * GET /api/logs?since=<seq> - Device log ring as NDJSON.
	 * Returns entries with seq > since; use the last entry's seq as the
	 * next `since` for tail-follow polling. Short timeout: this is called
	 * from a 3s poll loop, so a hung request must fail before the next tick.
	 */
	async getLogs(since = 0, signal?: AbortSignal): Promise<LogEntry[]> {
		const response = await apiFetch(`/api/logs?since=${since}`, {}, 5000, signal);
		if (!response.ok) {
			throw new ApiError(await errorMessage(response), response.status);
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
		const response = await apiFetch('/api/logs/previous', {}, DEFAULT_TIMEOUT_MS, signal);
		if (response.status === 404) return null;
		if (!response.ok) {
			throw new ApiError(await errorMessage(response), response.status);
		}
		return response.text();
	}
};

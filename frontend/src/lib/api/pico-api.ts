import type {
	CheckUpdateResponse,
	Config,
	ConfigUpdate,
	LogEntry,
	NetworkStatus,
	RebootResponse,
	TimezoneDocument
} from './types';

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
	return readBody(() => response.json());
}

/**
 * Read a response body, translating an abort mid-read into the same
 * readable error as a connect-phase timeout. The request timeout can fire
 * AFTER fetch() resolves — headers arrived but the device is streaming the
 * body slowly — and that abort surfaces from .json()/.text(), not fetch().
 */
async function readBody<T>(read: () => Promise<T>): Promise<T> {
	try {
		return await read();
	} catch (e) {
		if (e instanceof DOMException && (e.name === 'AbortError' || e.name === 'TimeoutError')) {
			throw new ApiError('Device did not respond in time', 0);
		}
		throw e;
	}
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
	 * GET /api/timezone - The UTC offset schedule the device is holding.
	 * The response is a valid body for setTimezone(): the device ignores the
	 * derived effective_offset_minutes field on the way back in.
	 */
	async getTimezone(signal?: AbortSignal): Promise<TimezoneDocument> {
		const response = await apiFetch('/api/timezone', {}, DEFAULT_TIMEOUT_MS, signal);
		return handleResponse<TimezoneDocument>(response);
	},

	/**
	 * PUT /api/timezone - Replace the offset schedule and the manual override.
	 *
	 * REPLACES, does not merge: absent fields are absent values. Always send
	 * the whole document, which is why the seed flow reads before it writes.
	 * The device writes flash only when a value actually changed, so posting
	 * an unchanged document on every page load costs it nothing.
	 */
	async setTimezone(document: TimezoneDocument, signal?: AbortSignal): Promise<TimezoneDocument> {
		const response = await apiFetch(
			'/api/timezone',
			{
				method: 'PUT',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(document)
			},
			DEFAULT_TIMEOUT_MS,
			signal
		);
		return handleResponse<TimezoneDocument>(response);
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
	 * POST /api/check-update - On-demand OTA check.
	 * The device fetches the backend manifest synchronously (a few seconds),
	 * hence the generous timeout. On 'updating' the device starts a blocking
	 * download shortly after responding and then reboots — callers should
	 * poll getStatus() until app_version changes. A timeout or dropped
	 * response can ALSO mean an update started (the download freezes the
	 * device's event loop), so treat those as "possibly updating", not error.
	 */
	async checkUpdate(signal?: AbortSignal): Promise<CheckUpdateResponse> {
		const response = await apiFetch('/api/check-update', { method: 'POST' }, 25000, signal);
		return handleResponse<CheckUpdateResponse>(response);
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
		const text = await readBody(() => response.text());
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
		return readBody(() => response.text());
	}
};

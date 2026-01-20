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

export const picoApi = {
	/**
	 * GET /api/config - Fetch full device configuration
	 */
	async getConfig(): Promise<Config> {
		const response = await fetch('/api/config');
		return handleResponse<Config>(response);
	},

	/**
	 * PUT /api/config - Merge update configuration
	 * Only fields present in the update are changed; omitted fields remain unchanged.
	 */
	async updateConfig(update: ConfigUpdate): Promise<Config> {
		const response = await fetch('/api/config', {
			method: 'PUT',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify(update)
		});
		return handleResponse<Config>(response);
	},

	/**
	 * GET /api/status - Get current network status
	 */
	async getStatus(): Promise<NetworkStatus> {
		const response = await fetch('/api/status');
		return handleResponse<NetworkStatus>(response);
	},

	/**
	 * POST /api/reboot - Trigger device restart
	 * Device will reboot after a 1-second delay.
	 */
	async reboot(): Promise<RebootResponse> {
		const response = await fetch('/api/reboot', { method: 'POST' });
		return handleResponse<RebootResponse>(response);
	}
};

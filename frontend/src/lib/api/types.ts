// Network configuration
export interface NetworkConfig {
	ssid: string;
	password: string;
	ap_mode: boolean;
	device_name: string;
	connect_timeout_seconds: number;
}

export type NetworkConfigUpdate = Partial<NetworkConfig>;

// API configuration
export interface ApiConfig {
	url: string;
	key: string;
}

export type ApiConfigUpdate = Partial<ApiConfig>;

// Display configuration
export interface DisplayConfig {
	brightness: number; // 0-100
	poll_interval_seconds: number; // min: 1
}

export type DisplayConfigUpdate = Partial<DisplayConfig>;

// Server configuration
export interface ServerConfig {
	cache_max_age_seconds: number; // min: 0
}

export type ServerConfigUpdate = Partial<ServerConfig>;

// Full configuration
export interface Config {
	network: NetworkConfig;
	api: ApiConfig;
	display: DisplayConfig;
	server: ServerConfig;
}

// Partial configuration for PUT requests
export interface ConfigUpdate {
	network?: NetworkConfigUpdate;
	api?: ApiConfigUpdate;
	display?: DisplayConfigUpdate;
	server?: ServerConfigUpdate;
}

// Network status response
export interface NetworkStatus {
	mode: 'ap' | 'station' | 'unknown';
	connected: boolean;
	is_fallback: boolean;
	configured_ssid?: string | null;
	ip?: string | null;
	hostname?: string | null;
	ap_ip?: string | null;
	ap_ssid?: string;
}

// Reboot response
export interface RebootResponse {
	message: string;
}

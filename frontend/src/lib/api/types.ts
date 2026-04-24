// Network configuration
export interface NetworkConfig {
	ssid: string;
	password: string;
	device_name: string;
	connect_timeout_seconds: number;
}

export type NetworkConfigUpdate = Partial<NetworkConfig>;

// API configuration
export interface ApiConfig {
	url: string;
	key: string;
	mock: boolean;
}

export type ApiConfigUpdate = Partial<ApiConfig>;

// Gamma correction configuration (discriminated union)
export type GammaConfig =
	| { type: "power"; value: number }
	| { type: "srgb" }
	| { type: "none" };

// Display configuration
export interface DisplayConfig {
	brightness: number; // 0-100
	poll_interval_seconds: number; // min: 1
	game_rotation_seconds: number; // min: 1, default: 60
	data_frequency_khz: number; // min: 2, max: 50000, default: 20000
	target_refresh_rate: number; // 30-240 Hz
	gamma: GammaConfig;
	blanking_time_ns: number; // 0-3000 nanoseconds
}

export type DisplayConfigUpdate = Partial<DisplayConfig>;

// Server configuration
export interface ServerConfig {
	cache_max_age_seconds: number; // min: 0
}

export type ServerConfigUpdate = Partial<ServerConfig>;

// Colors configuration (RGB 0-255)
export interface ColorsConfig {
	primary: Color;
	secondary: Color;
	accent: Color;
	clock_normal: Color;
	clock_warning: Color;
}

export type ColorsConfigUpdate = Partial<ColorsConfig>;

// Full configuration
export interface Config {
	network: NetworkConfig;
	api: ApiConfig;
	display: DisplayConfig;
	colors: ColorsConfig;
	server: ServerConfig;
}

// Partial configuration for PUT requests
export interface ConfigUpdate {
	network?: NetworkConfigUpdate;
	api?: ApiConfigUpdate;
	display?: DisplayConfigUpdate;
	colors?: ColorsConfigUpdate;
	server?: ServerConfigUpdate;
}

// Network status response
export interface NetworkStatus {
	mode: 'ap' | 'station' | 'unknown';
	connected: boolean;
	setup_mode: boolean;
	setup_reason: 'no_network_configured' | 'connection_failed' | null;
	configured_ssid?: string | null;
	ip?: string | null;
	hostname?: string | null;
	ap_ip?: string | null;
	ap_ssid?: string;
	// Memory telemetry
	memory_used: number;
	memory_free: number;
	flash_used: number;
	flash_free: number;
}

// Reboot response
export interface RebootResponse {
	message: string;
}

export interface Color {
	r: number;
	g: number;
	b: number;
}


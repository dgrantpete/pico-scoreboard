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

// Game API types
export interface Color {
	r: number;
	g: number;
	b: number;
}

export interface Team {
	abbreviation: string;
	color: Color;
	record?: string;
}

export interface TeamWithScore extends Team {
	score: number;
	timeouts: number;
}

export interface Situation {
	down: 'first' | 'second' | 'third' | 'fourth';
	distance: number;
	yard_line: number;
	possession: 'home' | 'away';
	red_zone: boolean;
}

export interface PregameGame {
	state: 'pregame';
	event_id: string;
	home: Team;
	away: Team;
	start_time: string;
	venue?: string;
	broadcast?: string;
	weather?: { temp: number; description: string };
}

export interface LiveGame {
	state: 'live';
	event_id: string;
	home: TeamWithScore;
	away: TeamWithScore;
	quarter: 'first' | 'second' | 'third' | 'fourth' | 'OT' | 'OT2';
	clock: string;
	situation?: Situation;
}

export interface FinalGame {
	state: 'final';
	event_id: string;
	home: TeamWithScore;
	away: TeamWithScore;
	status: 'final' | 'final/OT';
	winner: 'home' | 'away' | 'tie';
}

export type GameResponse = PregameGame | LiveGame | FinalGame;

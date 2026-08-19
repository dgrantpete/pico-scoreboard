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

// Gamma correction configuration (discriminated union)
export type GammaConfig =
	| { type: "power"; value: number }
	| { type: "srgb" }
	| { type: "none" };

// Display configuration
// Screen layout variant letters, keyed per sport × screen (see firmware
// scoreboard/screen_geometry.py tables). Applied live on save — no reboot.
// Screens with a single design (pregame — "Big time" locked in 2026-07-15 —
// soccer final, NBA live, football live) gain a key here only once a second
// design exists.
export interface VariantsConfig {
	mlb_final: string;
	nba_final: string;
	football_final: string;
	soccer_live: string;
}

export interface DisplayConfig {
	brightness: number; // 0-100
	poll_interval_seconds: number; // min: 1
	game_rotation_seconds: number; // min: 1, default: 60
	data_frequency_khz: number; // min: 2, max: 50000, default: 20000
	target_refresh_rate: number; // 30-240 Hz
	gamma: GammaConfig;
	blanking_time_ns: number; // 0-3000 nanoseconds
	variants: VariantsConfig;
	show_dividers: boolean; // divider lines on game screens; applied live
	scroll_speed_px_per_sec: number; // 5 | 10 | 20 | 30 | 60; applied live
}

export type DisplayConfigUpdate = Partial<DisplayConfig>;

// Server configuration
export interface ServerConfig {
	cache_max_age_seconds: number; // min: 0
}

export type ServerConfigUpdate = Partial<ServerConfig>;

// Hardware watchdog configuration
export interface WatchdogConfig {
	enabled: boolean; // default false: an armed WDT reboots ~8s after mpremote interrupts the script
	timeout_ms: number; // clamped on-device to 2000..8300 (RP2350 hardware max)
}

export type WatchdogConfigUpdate = Partial<WatchdogConfig>;

// Colors configuration (RGB 0-255)
export interface ColorsConfig {
	primary: Color;
	secondary: Color;
	accent: Color;
	clock_normal: Color;
	clock_warning: Color;
}

export type ColorsConfigUpdate = Partial<ColorsConfig>;

// Device logging configuration
export type LogLevel = 'none' | 'error' | 'debug';

export interface LogConfig {
	level: LogLevel;
}

export type LogConfigUpdate = Partial<LogConfig>;

// Over-the-air app update configuration
export interface OtaConfig {
	enabled: boolean;
}

export type OtaConfigUpdate = Partial<OtaConfig>;

// Sports / league selection. Football and soccer leagues are ESPN slugs
// (see the firmware LEAGUE_NAMES tables in scoreboard/football.py and
// scoreboard/soccer.py); empty list = that sport off.
export interface SportsConfig {
	mlb: { enabled: boolean };
	nba: { enabled: boolean };
	football: { leagues: string[] };
	soccer: { leagues: string[] };
}

export type SportsConfigUpdate = Partial<SportsConfig>;

// Full configuration
export interface Config {
	network: NetworkConfig;
	api: ApiConfig;
	display: DisplayConfig;
	colors: ColorsConfig;
	server: ServerConfig;
	watchdog: WatchdogConfig;
	log: LogConfig;
	ota: OtaConfig;
	sports: SportsConfig;
}

// Partial configuration for PUT requests
export interface ConfigUpdate {
	network?: NetworkConfigUpdate;
	api?: ApiConfigUpdate;
	display?: DisplayConfigUpdate;
	colors?: ColorsConfigUpdate;
	server?: ServerConfigUpdate;
	watchdog?: WatchdogConfigUpdate;
	log?: LogConfigUpdate;
	ota?: OtaConfigUpdate;
	sports?: SportsConfigUpdate;
}

// Network status response
export interface NetworkStatus {
	mode: 'ap' | 'station' | 'unknown';
	connected: boolean;
	setup_mode: boolean;
	setup_reason: 'no_network_configured' | 'connection_failed' | 'bad_auth' | null;
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
	// sha256 of the running app's ROMFS image; null on dev (littlefs) deploys
	app_version?: string | null;
}

// Reboot response
export interface RebootResponse {
	message: string;
}

// POST /api/check-update response. 'updating' means the device is about to
// download and restart; every other status is terminal for this check.
export interface CheckUpdateResponse {
	status: 'current' | 'updating' | 'disabled' | 'dev_deploy' | 'no_network' | 'error';
	version?: string | null;
	message?: string;
}

// GET/PUT /api/timezone — the browser-seeded UTC offset schedule.
//
// Every offset is minutes east of UTC (UTC−06:00 is -360), which is
// `-Date.prototype.getTimezoneOffset()`. The device converts to seconds once,
// on its side.
//
// The PUT REPLACES; it does not merge. Every absent or null field is an absent
// value, so a body of {} clears the timezone entirely — which is why the seed
// flow reads the document before it writes one (see stores/timezone.svelte.ts).
export interface TimezoneDocument {
	/** The offset in force now, per the last seed. */
	offset_minutes: number | null;
	/** The offset after the next DST transition; null in a zone without DST. */
	next_offset_minutes: number | null;
	/** When the offset changes, Unix seconds; null in a zone without DST. */
	transition_epoch_s: number | null;
	/** A fixed offset set by hand. Wins over the schedule whenever it is set. */
	manual_offset_minutes: number | null;
	/**
	 * GET only, derived, ignored by PUT: the offset the DEVICE would use right
	 * now, after the override precedence and the transition flip. Null when the
	 * device has never been seeded. This is what it believes, not what this
	 * browser assumes — the two can differ, and that is the point.
	 */
	effective_offset_minutes?: number | null;
}

export interface Color {
	r: number;
	g: number;
	b: number;
}

// One device log entry: [seq, unix_ts, level, message].
// level: 1 = ERROR, 2 = DEBUG (scoreboard/logger.py).
export type LogEntry = [number, number, number, string];


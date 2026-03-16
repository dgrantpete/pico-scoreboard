<script lang="ts">
	import { onMount } from 'svelte';
	import TeamLogo from '$lib/components/team-logo.svelte';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import WifiOff from '@lucide/svelte/icons/wifi-off';
	import { picoApi } from '$lib/api/pico-api';
	import type { GameResponse, NetworkStatus } from '$lib/api/types';

	interface GameData {
		id: string;
		homeTeamId: string;
		awayTeamId: string;
		homeScore: number;
		awayScore: number;
		quarter: string;
		timeRemaining: string;
		possession: 'home' | 'away' | null;
		down: number | null;
		distance: number | null;
		redZone: boolean;
		status: 'pregame' | 'active' | 'halftime' | 'final';
	}

	const downMap: Record<string, number> = { first: 1, second: 2, third: 3, fourth: 4 };
	const quarterMap: Record<string, string> = {
		first: 'Q1',
		second: 'Q2',
		third: 'Q3',
		fourth: 'Q4',
		OT: 'OT',
		OT2: '2OT'
	};

	function rgbToCss(color: { r: number; g: number; b: number }): string {
		// Handle black (0,0,0) with a fallback gray for visibility
		if (color.r === 0 && color.g === 0 && color.b === 0) {
			return '#666666';
		}
		return `rgb(${color.r}, ${color.g}, ${color.b})`;
	}

	function formatGameTime(isoString: string): string {
		const date = new Date(isoString);
		return date.toLocaleString(undefined, {
			weekday: 'short',
			month: 'short',
			day: 'numeric',
			hour: 'numeric',
			minute: '2-digit'
		});
	}

	function transformGame(game: GameResponse): GameData {
		if (game.state === 'pregame') {
			return {
				id: game.event_id,
				homeTeamId: game.home.abbreviation.toLowerCase(),
				awayTeamId: game.away.abbreviation.toLowerCase(),
				homeScore: 0,
				awayScore: 0,
				quarter: formatGameTime(game.start_time),
				timeRemaining: '',
				possession: null,
				down: null,
				distance: null,
				redZone: false,
				status: 'pregame'
			};
		} else if (game.state === 'live') {
			return {
				id: game.event_id,
				homeTeamId: game.home.abbreviation.toLowerCase(),
				awayTeamId: game.away.abbreviation.toLowerCase(),
				homeScore: game.home.score,
				awayScore: game.away.score,
				quarter: quarterMap[game.quarter] ?? game.quarter,
				timeRemaining: game.clock,
				possession: game.situation?.possession ?? null,
				down: game.situation ? downMap[game.situation.down] : null,
				distance: game.situation?.distance ?? null,
				redZone: game.situation?.red_zone ?? false,
				status: 'active'
			};
		} else {
			// final
			return {
				id: game.event_id,
				homeTeamId: game.home.abbreviation.toLowerCase(),
				awayTeamId: game.away.abbreviation.toLowerCase(),
				homeScore: game.home.score,
				awayScore: game.away.score,
				quarter: game.status === 'final/OT' ? 'FINAL/OT' : 'FINAL',
				timeRemaining: '',
				possession: null,
				down: null,
				distance: null,
				redZone: false,
				status: 'final'
			};
		}
	}

	let games = $state<GameData[]>([]);
	let rawGames = $state<GameResponse[]>([]);
	let currentGameIndex = $state(0);
	let isLoading = $state(true);
	let isRefreshing = $state(false);
	let error = $state<string | null>(null);
	let status = $state<NetworkStatus | null>(null);

	let currentGame = $derived(games[currentGameIndex]);
	let currentRawGame = $derived(rawGames[currentGameIndex]);
	let homeTeam = $derived(currentRawGame?.home ?? null);
	let awayTeam = $derived(currentRawGame?.away ?? null);

	async function fetchGames() {
		try {
			const response = await picoApi.getGames();
			rawGames = response;
			games = response.map(transformGame);
			error = null;
			// Reset index if it's out of bounds
			if (currentGameIndex >= games.length) {
				currentGameIndex = 0;
			}
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to fetch games';
			console.error('Failed to fetch games:', e);
		}
	}

	onMount(async () => {
		// Fetch status first to check if we're in setup mode
		try {
			status = await picoApi.getStatus();
		} catch (e) {
			console.error('Failed to fetch status:', e);
		}

		// Only fetch games if not in setup mode
		if (!status?.setup_mode) {
			await fetchGames();
		}
		isLoading = false;
	});

	function nextGame() {
		currentGameIndex = (currentGameIndex + 1) % games.length;
	}

	function prevGame() {
		currentGameIndex = (currentGameIndex - 1 + games.length) % games.length;
	}

	async function refreshScores() {
		isRefreshing = true;
		await fetchGames();
		isRefreshing = false;
	}

	function formatDownAndDistance(down: number | null, distance: number | null): string {
		if (down === null || distance === null) return '';
		const ordinal = ['1st', '2nd', '3rd', '4th'][down - 1] || `${down}th`;
		return `${ordinal} & ${distance}`;
	}
</script>

<div class="page">
	{#if isLoading}
		<!-- Loading State -->
		<div class="card narrow">
			<div class="card-body center-col">
				<RefreshCw class="icon-lg spinning muted" />
				<p class="text-muted">Loading games...</p>
			</div>
		</div>
	{:else if status?.setup_mode}
		<!-- Setup Mode Guidance -->
		<div class="card narrow amber-border">
			<div class="card-body center-col text-center">
				<div class="amber-icon-circle">
					<WifiOff class="icon-lg amber-icon" />
				</div>
				<div>
					<h3 class="heading">Network Setup Required</h3>
					{#if status.setup_reason === 'connection_failed'}
						<p class="subtext">
							We couldn't connect to your WiFi network. Please check your
							network settings to view live scores.
						</p>
					{:else}
						<p class="subtext">
							Your scoreboard needs to be connected to WiFi to fetch live
							game scores.
						</p>
					{/if}
				</div>
				<a href="#/setup" class="btn default">Go to Setup</a>
			</div>
		</div>
	{:else if error}
		<!-- Error State -->
		<div class="card narrow">
			<div class="card-body center-col text-center">
				<p class="text-error">Failed to load games</p>
				<p class="subtext">{error}</p>
				<button class="btn outline" onclick={refreshScores} disabled={isRefreshing}>
					<RefreshCw class="icon-sm {isRefreshing ? 'spinning' : ''}" />
					Retry
				</button>
			</div>
		</div>
	{:else if games.length === 0}
		<!-- Empty State -->
		<div class="card narrow">
			<div class="card-body center-col text-center">
				<p class="text-muted">No games available</p>
				<button class="btn outline" onclick={refreshScores} disabled={isRefreshing}>
					<RefreshCw class="icon-sm {isRefreshing ? 'spinning' : ''}" />
					Refresh
				</button>
			</div>
		</div>
	{:else if currentGame && homeTeam && awayTeam}
		<!-- Game Card -->
		<div class="card narrow">
			<div class="card-body">
				<!-- Status Bar -->
				<div class="status-bar">
					{#if currentGame.status === 'active'}
						<span class="live-dot">
							<span class="live-dot-ping"></span>
							<span class="live-dot-solid"></span>
						</span>
						<span class="badge destructive">LIVE</span>
						<span class="text-muted">&bull;</span>
					{/if}
					<span class="quarter-label">{currentGame.quarter}</span>
					{#if currentGame.timeRemaining}
						<span class="text-muted">{currentGame.timeRemaining}</span>
					{/if}
				</div>

				<!-- Scoreboard -->
				<div class="scoreboard">
					<!-- Away Team -->
					<div class="team-col">
						<TeamLogo
							teamId={awayTeam.abbreviation.toLowerCase()}
							teamName={awayTeam.abbreviation}
							abbreviation={awayTeam.abbreviation}
							primaryColor={rgbToCss(awayTeam.color)}
						/>
						<div class="team-abbr">{awayTeam.abbreviation}</div>
						<div class="score">
							{currentGame.awayScore}
						</div>
						{#if currentGame.possession === 'away'}
							<div class="possession">🏈</div>
						{:else}
							<div class="possession-spacer"></div>
						{/if}
					</div>

					<!-- Divider -->
					<div class="divider">
						<span class="at-symbol">@</span>
					</div>

					<!-- Home Team -->
					<div class="team-col">
						<TeamLogo
							teamId={homeTeam.abbreviation.toLowerCase()}
							teamName={homeTeam.abbreviation}
							abbreviation={homeTeam.abbreviation}
							primaryColor={rgbToCss(homeTeam.color)}
						/>
						<div class="team-abbr">{homeTeam.abbreviation}</div>
						<div class="score">
							{currentGame.homeScore}
						</div>
						{#if currentGame.possession === 'home'}
							<div class="possession">🏈</div>
						{:else}
							<div class="possession-spacer"></div>
						{/if}
					</div>
				</div>

				<!-- Game Details -->
				{#if currentGame.status === 'active' && currentGame.down}
					<div class="game-details">
						{#if currentGame.redZone}
							<span class="badge destructive">RED ZONE</span>
						{/if}
						<span class="down-distance">
							{formatDownAndDistance(currentGame.down, currentGame.distance)}
						</span>
					</div>
				{/if}
			</div>
		</div>

		<!-- Controls -->
		<div class="controls">
			<button class="btn outline icon" onclick={prevGame} disabled={games.length <= 1}>
				<ChevronLeft />
			</button>

			<button class="btn outline" onclick={refreshScores} disabled={isRefreshing}>
				<RefreshCw class="icon-sm {isRefreshing ? 'spinning' : ''}" />
				Refresh
			</button>

			<button class="btn outline icon" onclick={nextGame} disabled={games.length <= 1}>
				<ChevronRight />
			</button>
		</div>

		<!-- Game Counter -->
		<div class="game-dots">
			{#each games as _, i}
				<button
					class="dot {i === currentGameIndex ? 'active' : ''}"
					onclick={() => (currentGameIndex = i)}
					aria-label="Go to game {i + 1}"
				></button>
			{/each}
		</div>
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 1.5rem;
	}

	.card {
		background: var(--card);
		color: var(--card-foreground);
		border: 1px solid var(--border);
		border-radius: 0.75rem;
		box-shadow: 0 1px 2px oklch(0 0 0 / 5%);

		&.narrow {
			width: 100%;
			max-width: 28rem;
		}

		&.amber-border {
			border-color: oklch(0.769 0.188 70.08);
		}
	}

	.card-body {
		padding: 1.5rem;

		&.center-col {
			display: flex;
			flex-direction: column;
			align-items: center;
			gap: 1rem;
		}
	}

	.text-center {
		text-align: center;
	}

	.text-muted {
		color: var(--muted-foreground);
	}

	.text-error {
		color: var(--destructive);
		font-weight: 500;
	}

	.subtext {
		margin-block-start: 0.25rem;
		font-size: 0.875rem;
		color: var(--muted-foreground);
	}

	.heading {
		font-size: 1.125rem;
		font-weight: 600;
	}

	/* Amber icon circle for setup mode */
	.amber-icon-circle {
		border-radius: 9999px;
		padding: 0.75rem;
		background: oklch(0.962 0.059 95.617);
	}

	:global(.dark) .amber-icon-circle {
		background: oklch(0.356 0.09 56.09);
	}

	/* Icon sizing helpers via :global for Lucide svgs */
	:global(.icon-lg) {
		width: 2rem;
		height: 2rem;
	}

	:global(.icon-sm) {
		width: 1rem;
		height: 1rem;
	}

	:global(.spinning) {
		animation: spin 1s linear infinite;
	}

	:global(.muted) {
		color: var(--muted-foreground);
	}

	:global(.amber-icon) {
		color: oklch(0.666 0.179 58.318);
	}

	:global(.dark .amber-icon) {
		color: oklch(0.828 0.159 84.429);
	}

	/* Button styles */
	.btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		border-radius: 0.375rem;
		font-size: 0.875rem;
		font-weight: 500;
		white-space: nowrap;
		cursor: pointer;
		border: none;
		transition: background-color 0.15s, color 0.15s;
		outline: none;
		height: 2.25rem;
		padding-inline: 1rem;

		&:disabled {
			opacity: 0.5;
			pointer-events: none;
		}

		&:focus-visible {
			box-shadow: 0 0 0 2px var(--background), 0 0 0 4px var(--ring);
		}

		&.default {
			background: var(--primary);
			color: var(--primary-foreground);
		}

		&.outline {
			background: var(--card);
			border: 1px solid var(--border);

			&:hover {
				background: var(--accent);
				color: var(--accent-foreground);
			}
		}

		&.icon {
			width: 2.25rem;
			padding: 0;
		}

		& :global(svg) {
			width: 1rem;
			height: 1rem;
		}
	}

	/* Badge styles */
	.badge {
		display: inline-flex;
		align-items: center;
		border-radius: 9999px;
		padding-block: 0.125rem;
		padding-inline: 0.625rem;
		font-size: 0.75rem;
		font-weight: 500;

		&.destructive {
			background: var(--destructive);
			color: white;
		}
	}

	/* Live indicator dot */
	.live-dot {
		position: relative;
		display: flex;
		block-size: 0.5rem;
		inline-size: 0.5rem;
	}

	.live-dot-ping {
		position: absolute;
		display: inline-flex;
		block-size: 100%;
		inline-size: 100%;
		border-radius: 9999px;
		background: oklch(0.704 0.191 22.216);
		opacity: 0.75;
		animation: ping 1s cubic-bezier(0, 0, 0.2, 1) infinite;
	}

	.live-dot-solid {
		position: relative;
		display: inline-flex;
		block-size: 0.5rem;
		inline-size: 0.5rem;
		border-radius: 9999px;
		background: var(--destructive);
	}

	/* Status bar */
	.status-bar {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
		font-size: 0.875rem;
		margin-block-end: 1rem;
	}

	.quarter-label {
		font-weight: 500;
	}

	/* Scoreboard grid */
	.scoreboard {
		display: grid;
		grid-template-columns: 1fr auto 1fr;
		align-items: center;
		gap: 1rem;
	}

	.team-col {
		text-align: center;
	}

	.team-abbr {
		font-size: 1.125rem;
		font-weight: 700;
	}

	.score {
		margin-block-start: 0.5rem;
		font-size: 2.25rem;
		font-weight: 700;
		font-variant-numeric: tabular-nums;
	}

	.possession {
		margin-block-start: 0.25rem;
		font-size: 1.125rem;
	}

	.possession-spacer {
		margin-block-start: 0.25rem;
		block-size: 1.75rem;
	}

	.divider {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 0.25rem;
	}

	.at-symbol {
		font-size: 1.5rem;
		font-weight: 700;
		color: var(--muted-foreground);
	}

	/* Game details (down & distance) */
	.game-details {
		margin-block-start: 1rem;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 0.5rem;
	}

	.down-distance {
		font-size: 0.875rem;
		font-weight: 500;
	}

	/* Controls row */
	.controls {
		display: flex;
		align-items: center;
		gap: 1rem;
	}

	/* Game counter dots */
	.game-dots {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		font-size: 0.875rem;
		color: var(--muted-foreground);
	}

	.dot {
		block-size: 0.5rem;
		inline-size: 0.5rem;
		border-radius: 9999px;
		border: none;
		padding: 0;
		cursor: pointer;
		transition: background-color 0.15s;
		background: var(--muted-foreground);
		opacity: 0.3;

		&.active {
			background: var(--primary);
			opacity: 1;
		}
	}
</style>

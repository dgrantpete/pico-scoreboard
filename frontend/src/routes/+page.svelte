<script lang="ts">
	import { onMount } from 'svelte';
	import * as Card from '$lib/components/ui/card';
	import { Button } from '$lib/components/ui/button';
	import TeamLogo from '$lib/components/team-logo.svelte';
	import { Badge } from '$lib/components/ui/badge';
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

<div class="flex flex-col items-center gap-6">
	{#if isLoading}
		<!-- Loading State -->
		<Card.Root class="w-full max-w-md">
			<Card.Content class="p-6">
				<div class="flex flex-col items-center gap-4">
					<RefreshCw class="h-8 w-8 animate-spin text-muted-foreground" />
					<p class="text-muted-foreground">Loading games...</p>
				</div>
			</Card.Content>
		</Card.Root>
	{:else if status?.setup_mode}
		<!-- Setup Mode Guidance -->
		<Card.Root class="w-full max-w-md border-amber-500">
			<Card.Content class="p-6">
				<div class="flex flex-col items-center gap-4 text-center">
					<div class="rounded-full bg-amber-100 p-3 dark:bg-amber-900">
						<WifiOff class="h-8 w-8 text-amber-600 dark:text-amber-400" />
					</div>
					<div>
						<h3 class="text-lg font-semibold">Network Setup Required</h3>
						{#if status.setup_reason === 'connection_failed'}
							<p class="mt-1 text-sm text-muted-foreground">
								We couldn't connect to your WiFi network. Please check your
								network settings to view live scores.
							</p>
						{:else}
							<p class="mt-1 text-sm text-muted-foreground">
								Your scoreboard needs to be connected to WiFi to fetch live
								game scores.
							</p>
						{/if}
					</div>
					<Button href="#/setup">Go to Setup</Button>
				</div>
			</Card.Content>
		</Card.Root>
	{:else if error}
		<!-- Error State -->
		<Card.Root class="w-full max-w-md">
			<Card.Content class="p-6">
				<div class="flex flex-col items-center gap-4 text-center">
					<p class="text-destructive font-medium">Failed to load games</p>
					<p class="text-sm text-muted-foreground">{error}</p>
					<Button variant="outline" onclick={refreshScores} disabled={isRefreshing}>
						<RefreshCw class="mr-2 h-4 w-4 {isRefreshing ? 'animate-spin' : ''}" />
						Retry
					</Button>
				</div>
			</Card.Content>
		</Card.Root>
	{:else if games.length === 0}
		<!-- Empty State -->
		<Card.Root class="w-full max-w-md">
			<Card.Content class="p-6">
				<div class="flex flex-col items-center gap-4 text-center">
					<p class="text-muted-foreground">No games available</p>
					<Button variant="outline" onclick={refreshScores} disabled={isRefreshing}>
						<RefreshCw class="mr-2 h-4 w-4 {isRefreshing ? 'animate-spin' : ''}" />
						Refresh
					</Button>
				</div>
			</Card.Content>
		</Card.Root>
	{:else if currentGame && homeTeam && awayTeam}
		<!-- Game Card -->
		<Card.Root class="w-full max-w-md">
			<Card.Content class="p-6">
				<!-- Status Bar -->
				<div class="mb-4 flex items-center justify-center gap-2 text-sm">
					{#if currentGame.status === 'active'}
						<span class="relative flex h-2 w-2">
							<span
								class="absolute inline-flex h-full w-full animate-ping rounded-full bg-red-400 opacity-75"
							></span>
							<span class="relative inline-flex h-2 w-2 rounded-full bg-red-500"></span>
						</span>
						<Badge variant="destructive">LIVE</Badge>
						<span class="text-muted-foreground">•</span>
					{/if}
					<span class="font-medium">{currentGame.quarter}</span>
					{#if currentGame.timeRemaining}
						<span class="text-muted-foreground">{currentGame.timeRemaining}</span>
					{/if}
				</div>

				<!-- Scoreboard -->
				<div class="grid grid-cols-[1fr_auto_1fr] items-center gap-4">
					<!-- Away Team -->
					<div class="text-center">
						<TeamLogo
							teamId={awayTeam.abbreviation.toLowerCase()}
							teamName={awayTeam.abbreviation}
							abbreviation={awayTeam.abbreviation}
							primaryColor={rgbToCss(awayTeam.color)}
						/>
						<div class="text-lg font-bold">{awayTeam.abbreviation}</div>
						<div class="mt-2 text-4xl font-bold tabular-nums">
							{currentGame.awayScore}
						</div>
						{#if currentGame.possession === 'away'}
							<div class="mt-1 text-lg">🏈</div>
						{:else}
							<div class="mt-1 h-7"></div>
						{/if}
					</div>

					<!-- Divider -->
					<div class="flex flex-col items-center gap-1">
						<span class="text-2xl font-bold text-muted-foreground">@</span>
					</div>

					<!-- Home Team -->
					<div class="text-center">
						<TeamLogo
							teamId={homeTeam.abbreviation.toLowerCase()}
							teamName={homeTeam.abbreviation}
							abbreviation={homeTeam.abbreviation}
							primaryColor={rgbToCss(homeTeam.color)}
						/>
						<div class="text-lg font-bold">{homeTeam.abbreviation}</div>
						<div class="mt-2 text-4xl font-bold tabular-nums">
							{currentGame.homeScore}
						</div>
						{#if currentGame.possession === 'home'}
							<div class="mt-1 text-lg">🏈</div>
						{:else}
							<div class="mt-1 h-7"></div>
						{/if}
					</div>
				</div>

				<!-- Game Details -->
				{#if currentGame.status === 'active' && currentGame.down}
					<div class="mt-4 flex items-center justify-center gap-2">
						{#if currentGame.redZone}
							<Badge variant="destructive">RED ZONE</Badge>
						{/if}
						<span class="text-sm font-medium">
							{formatDownAndDistance(currentGame.down, currentGame.distance)}
						</span>
					</div>
				{/if}
			</Card.Content>
		</Card.Root>

		<!-- Controls -->
		<div class="flex items-center gap-4">
			<Button variant="outline" size="icon" onclick={prevGame} disabled={games.length <= 1}>
				<ChevronLeft class="h-4 w-4" />
			</Button>

			<Button variant="outline" onclick={refreshScores} disabled={isRefreshing}>
				<RefreshCw class="mr-2 h-4 w-4 {isRefreshing ? 'animate-spin' : ''}" />
				Refresh
			</Button>

			<Button variant="outline" size="icon" onclick={nextGame} disabled={games.length <= 1}>
				<ChevronRight class="h-4 w-4" />
			</Button>
		</div>

		<!-- Game Counter -->
		<div class="flex items-center gap-2 text-sm text-muted-foreground">
			{#each games as _, i}
				<button
					class="h-2 w-2 rounded-full transition-colors {i === currentGameIndex
						? 'bg-primary'
						: 'bg-muted-foreground/30'}"
					onclick={() => (currentGameIndex = i)}
					aria-label="Go to game {i + 1}"
				></button>
			{/each}
		</div>
	{/if}
</div>

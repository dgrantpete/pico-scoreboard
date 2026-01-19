<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import { Button } from '$lib/components/ui/button';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import { getTeamById } from '$lib/data/nfl-teams';

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

	// Demo games data
	let games = $state<GameData[]>([
		{
			id: '1',
			homeTeamId: 'buf',
			awayTeamId: 'mia',
			homeScore: 21,
			awayScore: 17,
			quarter: 'Q3',
			timeRemaining: '8:42',
			possession: 'home',
			down: 2,
			distance: 8,
			redZone: false,
			status: 'active'
		},
		{
			id: '2',
			homeTeamId: 'kc',
			awayTeamId: 'bal',
			homeScore: 14,
			awayScore: 14,
			quarter: 'Q2',
			timeRemaining: '2:15',
			possession: 'away',
			down: 3,
			distance: 4,
			redZone: true,
			status: 'active'
		},
		{
			id: '3',
			homeTeamId: 'sf',
			awayTeamId: 'dal',
			homeScore: 28,
			awayScore: 24,
			quarter: 'FINAL',
			timeRemaining: '',
			possession: null,
			down: null,
			distance: null,
			redZone: false,
			status: 'final'
		}
	]);

	let currentGameIndex = $state(0);
	let isRefreshing = $state(false);

	let currentGame = $derived(games[currentGameIndex]);
	let homeTeam = $derived(getTeamById(currentGame.homeTeamId));
	let awayTeam = $derived(getTeamById(currentGame.awayTeamId));

	function nextGame() {
		currentGameIndex = (currentGameIndex + 1) % games.length;
	}

	function prevGame() {
		currentGameIndex = (currentGameIndex - 1 + games.length) % games.length;
	}

	async function refreshScores() {
		isRefreshing = true;
		// Simulate API call
		await new Promise((resolve) => setTimeout(resolve, 500));
		// Randomly adjust scores for active games
		games = games.map((game) => {
			if (game.status === 'active') {
				return {
					...game,
					homeScore: game.homeScore + Math.floor(Math.random() * 2),
					awayScore: game.awayScore + Math.floor(Math.random() * 2)
				};
			}
			return game;
		});
		isRefreshing = false;
	}

	function formatDownAndDistance(down: number | null, distance: number | null): string {
		if (down === null || distance === null) return '';
		const ordinal = ['1st', '2nd', '3rd', '4th'][down - 1] || `${down}th`;
		return `${ordinal} & ${distance}`;
	}
</script>

<div class="flex flex-col items-center gap-6">
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
					<span class="font-medium text-red-500">LIVE</span>
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
					<div
						class="mx-auto mb-2 flex h-12 w-12 items-center justify-center rounded-lg text-lg font-bold text-white"
						style="background-color: {awayTeam?.primaryColor}"
					>
						{awayTeam?.abbreviation}
					</div>
					<div class="text-sm font-medium text-muted-foreground">
						{awayTeam?.city}
					</div>
					<div class="text-sm font-bold">{awayTeam?.name}</div>
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
					<div
						class="mx-auto mb-2 flex h-12 w-12 items-center justify-center rounded-lg text-lg font-bold text-white"
						style="background-color: {homeTeam?.primaryColor}"
					>
						{homeTeam?.abbreviation}
					</div>
					<div class="text-sm font-medium text-muted-foreground">
						{homeTeam?.city}
					</div>
					<div class="text-sm font-bold">{homeTeam?.name}</div>
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
						<span
							class="rounded bg-red-500 px-2 py-0.5 text-xs font-bold text-white"
						>
							RED ZONE
						</span>
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
</div>

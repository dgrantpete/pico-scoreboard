<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import type { Config } from "$lib/api/types";

	// Single-league sports ({enabled} config shape): one registry row per
	// sport; a 4th sport is one entry here. Multi-league sports ({leagues[]}
	// shape) get a slug list like SOCCER_LEAGUES below.
	const ENABLED_SPORTS: {
		key: keyof Config["sports"] & ("mlb" | "nba");
		label: string;
		hint: string;
	}[] = [
		{ key: "mlb", label: "MLB", hint: "Major League Baseball" },
		{ key: "nba", label: "NBA", hint: "National Basketball Association" },
	];

	// Mirrors the firmware's league registries (scoreboard/football.py +
	// scoreboard/soccer.py LEAGUE_NAMES / backend espn/league.rs). Values
	// are ESPN league slugs.
	const FOOTBALL_LEAGUES: { slug: string; label: string; hint: string }[] = [
		{ slug: "nfl", label: "NFL", hint: "National Football League" },
		{
			slug: "college-football",
			label: "College Football",
			hint: "NCAA — ESPN's Top 25 slate",
		},
	];

	const SOCCER_LEAGUES: { slug: string; label: string; hint: string }[] = [
		{ slug: "usa.1", label: "MLS", hint: "Major League Soccer" },
		{ slug: "eng.1", label: "Premier League", hint: "English top flight" },
		{ slug: "mex.1", label: "Liga MX", hint: "Mexican top flight" },
		{ slug: "fifa.world", label: "World Cup", hint: "FIFA World Cup" },
	];

	function toggleSport(key: "mlb" | "nba") {
		const current = settingsStore.config?.sports[key]?.enabled ?? false;
		settingsStore.updateSports(key, { enabled: !current });
	}

	function toggleLeague(sport: "football" | "soccer", slug: string) {
		const current = settingsStore.config?.sports[sport].leagues ?? [];
		const leagues = current.includes(slug)
			? current.filter((s) => s !== slug)
			: [...current, slug];
		settingsStore.updateSports(sport, { leagues });
	}
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Sports</h3>
			<p class="card-description">
				Which leagues rotate on the board. Takes effect after a reboot.
			</p>
		</header>
		<div class="card-content">
			{#each ENABLED_SPORTS as sport (sport.key)}
				<div class="row-between">
					<div class="label-group">
						<span class="label-text">{sport.label}</span>
						<p class="text-sm text-muted">{sport.hint}</p>
					</div>
					<label class="switch">
						<input
							type="checkbox"
							checked={config.sports[sport.key].enabled}
							onchange={() => toggleSport(sport.key)}
						/>
						<span class="switch-track"><span class="switch-thumb"></span></span>
					</label>
				</div>
			{/each}

			<hr class="separator" />

			<div class="field-group">
				<span class="label-text">Football Leagues</span>
				<p class="hint">
					Pro and college football — their games join the same rotation.
				</p>
			</div>

			{#each FOOTBALL_LEAGUES as league (league.slug)}
				<div class="row-between">
					<div class="label-group">
						<span class="label-text">{league.label}</span>
						<p class="text-sm text-muted">{league.hint}</p>
					</div>
					<label class="switch">
						<input
							type="checkbox"
							checked={config.sports.football.leagues.includes(league.slug)}
							onchange={() => toggleLeague("football", league.slug)}
						/>
						<span class="switch-track"><span class="switch-thumb"></span></span>
					</label>
				</div>
			{/each}

			<hr class="separator" />

			<div class="field-group">
				<span class="label-text">Soccer Leagues</span>
				<p class="hint">
					Pick the competitions you follow — their matches join the same
					rotation.
				</p>
			</div>

			{#each SOCCER_LEAGUES as league (league.slug)}
				<div class="row-between">
					<div class="label-group">
						<span class="label-text">{league.label}</span>
						<p class="text-sm text-muted">{league.hint}</p>
					</div>
					<label class="switch">
						<input
							type="checkbox"
							checked={config.sports.soccer.leagues.includes(league.slug)}
							onchange={() => toggleLeague("soccer", league.slug)}
						/>
						<span class="switch-track"><span class="switch-thumb"></span></span>
					</label>
				</div>
			{/each}
		</div>
	</section>
{/if}

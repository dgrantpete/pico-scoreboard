<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";

	// Mirrors the firmware's league registry (scoreboard/soccer.py
	// LEAGUE_NAMES / backend espn/league.rs). Values are ESPN league slugs.
	const SOCCER_LEAGUES: { slug: string; label: string; hint: string }[] = [
		{ slug: "usa.1", label: "MLS", hint: "Major League Soccer" },
		{ slug: "eng.1", label: "Premier League", hint: "English top flight" },
		{ slug: "mex.1", label: "Liga MX", hint: "Mexican top flight" },
		{ slug: "fifa.world", label: "World Cup", hint: "FIFA World Cup" },
	];

	function toggleLeague(slug: string) {
		const current = settingsStore.config?.sports.soccer.leagues ?? [];
		const leagues = current.includes(slug)
			? current.filter((s) => s !== slug)
			: [...current, slug];
		settingsStore.updateSports("soccer", { leagues });
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
			<div class="row-between">
				<div class="label-group">
					<span class="label-text">MLB</span>
					<p class="text-sm text-muted">Major League Baseball</p>
				</div>
				<label class="switch">
					<input
						type="checkbox"
						checked={config.sports.mlb.enabled}
						onchange={() =>
							settingsStore.updateSports("mlb", {
								enabled: !settingsStore.config?.sports.mlb.enabled,
							})}
					/>
					<span class="switch-track"><span class="switch-thumb"></span></span>
				</label>
			</div>

			<div class="row-between">
				<div class="label-group">
					<span class="label-text">NBA</span>
					<p class="text-sm text-muted">National Basketball Association</p>
				</div>
				<label class="switch">
					<input
						type="checkbox"
						checked={config.sports.nba.enabled}
						onchange={() =>
							settingsStore.updateSports("nba", {
								enabled: !settingsStore.config?.sports.nba.enabled,
							})}
					/>
					<span class="switch-track"><span class="switch-thumb"></span></span>
				</label>
			</div>

			<hr class="separator" />

			<div class="field-group">
				<span class="label-text">Soccer Leagues</span>
				<p class="hint">
					Pick the competitions you follow — their matches join the same
					rotation as MLB games.
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
							onchange={() => toggleLeague(league.slug)}
						/>
						<span class="switch-track"><span class="switch-thumb"></span></span>
					</label>
				</div>
			{/each}
		</div>
	</section>
{/if}

<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Slider } from '$lib/components/ui/slider';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import * as Collapsible from '$lib/components/ui/collapsible';
	import { Separator } from '$lib/components/ui/separator';
	import { Label } from '$lib/components/ui/label';
	import Save from '@lucide/svelte/icons/save';
	import Eye from '@lucide/svelte/icons/eye';
	import EyeOff from '@lucide/svelte/icons/eye-off';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import {
		NFL_TEAMS,
		NFL_DIVISIONS,
		getTeamsByDivision,
		createDefaultTeamSettings,
		type TeamSettings
	} from '$lib/data/nfl-teams';

	// Network settings
	let wifiNetwork = $state('');
	let wifiPassword = $state('');
	let showPassword = $state(false);

	// Display settings
	let brightness = $state([75]);
	let rotationSpeed = $state([10]); // seconds

	// Team settings
	let teamSettings = $state<Record<string, TeamSettings>>(createDefaultTeamSettings());

	// Division collapse state
	let openDivisions = $state<Set<string>>(new Set());

	function toggleDivision(division: string) {
		if (openDivisions.has(division)) {
			openDivisions.delete(division);
		} else {
			openDivisions.add(division);
		}
		openDivisions = new Set(openDivisions); // Trigger reactivity
	}

	function toggleTeam(teamId: string) {
		teamSettings[teamId].enabled = !teamSettings[teamId].enabled;
	}

	function toggleTeamOption(teamId: string, option: 'showSummary' | 'showActiveGames') {
		teamSettings[teamId][option] = !teamSettings[teamId][option];
	}

	// Count enabled teams
	let enabledTeamCount = $derived(
		Object.values(teamSettings).filter((s) => s.enabled).length
	);

	let isSaving = $state(false);

	async function saveSettings() {
		isSaving = true;
		// Simulate API call - will be replaced with actual Pico communication
		await new Promise((resolve) => setTimeout(resolve, 500));
		isSaving = false;
	}
</script>

<div class="mx-auto max-w-2xl space-y-6">
	<div>
		<h2 class="text-2xl font-bold">Settings</h2>
		<p class="text-muted-foreground">Configure your NFL scoreboard</p>
	</div>

	<!-- Network Configuration -->
	<Card.Root>
		<Card.Header>
			<Card.Title>Network</Card.Title>
			<Card.Description>WiFi configuration for the Pico</Card.Description>
		</Card.Header>
		<Card.Content class="space-y-4">
			<div class="space-y-2">
				<Label for="wifi-network">WiFi Network (SSID)</Label>
				<Input
					id="wifi-network"
					type="text"
					placeholder="Enter network name"
					bind:value={wifiNetwork}
				/>
			</div>
			<div class="space-y-2">
				<Label for="wifi-password">WiFi Password</Label>
				<div class="relative">
					<Input
						id="wifi-password"
						type={showPassword ? 'text' : 'password'}
						placeholder="Enter password"
						bind:value={wifiPassword}
						class="pr-10"
					/>
					<Button
						variant="ghost"
						size="sm"
						class="absolute right-0 top-0 h-full px-3 hover:bg-transparent"
						onclick={() => (showPassword = !showPassword)}
					>
						{#if showPassword}
							<EyeOff class="h-4 w-4 text-muted-foreground" />
						{:else}
							<Eye class="h-4 w-4 text-muted-foreground" />
						{/if}
					</Button>
				</div>
			</div>
		</Card.Content>
	</Card.Root>

	<!-- Display Settings -->
	<Card.Root>
		<Card.Header>
			<Card.Title>Display</Card.Title>
			<Card.Description>Adjust brightness and rotation settings</Card.Description>
		</Card.Header>
		<Card.Content class="space-y-6">
			<div class="space-y-2">
				<div class="flex items-center justify-between">
					<Label>Brightness</Label>
					<span class="text-sm text-muted-foreground">{brightness[0]}%</span>
				</div>
				<Slider bind:value={brightness} max={100} step={1} />
			</div>

			<Separator />

			<div class="space-y-2">
				<div class="flex items-center justify-between">
					<div>
						<Label>Rotation Speed</Label>
						<p class="text-sm text-muted-foreground">
							Time between game switches
						</p>
					</div>
					<span class="text-sm font-medium">{rotationSpeed[0]}s</span>
				</div>
				<Slider bind:value={rotationSpeed} min={3} max={30} step={1} />
			</div>
		</Card.Content>
	</Card.Root>

	<!-- Team Selection -->
	<Card.Root>
		<Card.Header>
			<Card.Title>Teams</Card.Title>
			<Card.Description>
				Select teams to follow ({enabledTeamCount} selected)
			</Card.Description>
		</Card.Header>
		<Card.Content class="space-y-2">
			{#each NFL_DIVISIONS as division}
				{@const teams = getTeamsByDivision(division)}
				{@const isOpen = openDivisions.has(division)}
				{@const enabledInDivision = teams.filter((t) => teamSettings[t.id].enabled).length}

				<Collapsible.Root open={isOpen}>
					<Collapsible.Trigger
						class="flex w-full items-center justify-between rounded-lg border p-3 hover:bg-accent"
						onclick={() => toggleDivision(division)}
					>
						<div class="flex items-center gap-2">
							<span class="font-medium">{division}</span>
							{#if enabledInDivision > 0}
								<span class="rounded-full bg-primary px-2 py-0.5 text-xs text-primary-foreground">
									{enabledInDivision}
								</span>
							{/if}
						</div>
						<ChevronDown
							class="h-4 w-4 transition-transform {isOpen ? 'rotate-180' : ''}"
						/>
					</Collapsible.Trigger>
					<Collapsible.Content>
						<div class="mt-2 space-y-1 pl-2">
							{#each teams as team}
								{@const settings = teamSettings[team.id]}
								<div
									class="rounded-lg border p-3 {settings.enabled
										? 'border-primary/50 bg-primary/5'
										: ''}"
								>
									<div class="flex items-center gap-3">
										<Checkbox
											id="team-{team.id}"
											checked={settings.enabled}
											onCheckedChange={() => toggleTeam(team.id)}
										/>
										<label
											for="team-{team.id}"
											class="flex-1 cursor-pointer font-medium"
										>
											{team.city} {team.name}
										</label>
										<span
											class="rounded px-2 py-0.5 text-xs font-bold text-white"
											style="background-color: {team.primaryColor}"
										>
											{team.abbreviation}
										</span>
									</div>

									{#if settings.enabled}
										<div class="mt-3 flex gap-4 pl-7">
											<label class="flex items-center gap-2 text-sm">
												<Checkbox
													checked={settings.showSummary}
													onCheckedChange={() =>
														toggleTeamOption(team.id, 'showSummary')}
												/>
												Team Summary
											</label>
											<label class="flex items-center gap-2 text-sm">
												<Checkbox
													checked={settings.showActiveGames}
													onCheckedChange={() =>
														toggleTeamOption(team.id, 'showActiveGames')}
												/>
												Active Games
											</label>
										</div>
									{/if}
								</div>
							{/each}
						</div>
					</Collapsible.Content>
				</Collapsible.Root>
			{/each}
		</Card.Content>
	</Card.Root>

	<!-- Save Button -->
	<div class="flex justify-end pb-8">
		<Button onclick={saveSettings} disabled={isSaving} size="lg">
			<Save class="mr-2 h-4 w-4" />
			{isSaving ? 'Saving...' : 'Save Settings'}
		</Button>
	</div>
</div>

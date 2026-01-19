<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import { browser } from '$app/environment';
	import { Button } from '$lib/components/ui/button';
	import Settings from '@lucide/svelte/icons/settings';
	import Sun from '@lucide/svelte/icons/sun';
	import Moon from '@lucide/svelte/icons/moon';
	import Monitor from '@lucide/svelte/icons/monitor';

	let { children } = $props();

	// Dark mode state: 'auto' | 'light' | 'dark'
	type ThemeMode = 'auto' | 'light' | 'dark';
	let themeMode = $state<ThemeMode>('auto');
	let systemPrefersDark = $state(false);

	// Computed actual theme based on mode and system preference
	let isDark = $derived(
		themeMode === 'dark' || (themeMode === 'auto' && systemPrefersDark)
	);

	$effect(() => {
		if (browser) {
			// Load saved preference
			const saved = localStorage.getItem('theme') as ThemeMode | null;
			if (saved && ['auto', 'light', 'dark'].includes(saved)) {
				themeMode = saved;
			}

			// Check system preference
			const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
			systemPrefersDark = mediaQuery.matches;

			// Listen for system preference changes
			const handleChange = (e: MediaQueryListEvent) => {
				systemPrefersDark = e.matches;
			};
			mediaQuery.addEventListener('change', handleChange);

			return () => mediaQuery.removeEventListener('change', handleChange);
		}
	});

	// Apply dark class to document
	$effect(() => {
		if (browser) {
			if (isDark) {
				document.documentElement.classList.add('dark');
			} else {
				document.documentElement.classList.remove('dark');
			}
		}
	});

	function cycleTheme() {
		const modes: ThemeMode[] = ['auto', 'light', 'dark'];
		const currentIndex = modes.indexOf(themeMode);
		themeMode = modes[(currentIndex + 1) % modes.length];
		if (browser) {
			localStorage.setItem('theme', themeMode);
		}
	}

	// Label based on current mode
	let themeLabel = $derived(
		themeMode === 'auto' ? 'Auto' : themeMode === 'light' ? 'Light' : 'Dark'
	);
</script>

<svelte:head>
	<title>NFL Scoreboard</title>
</svelte:head>

<div class="min-h-screen bg-background">
	<header class="border-b">
		<div class="container mx-auto flex h-14 items-center justify-between px-4">
			<div class="flex items-center gap-2">
				<span class="text-xl">🏈</span>
				<h1 class="text-lg font-semibold">NFL Scoreboard</h1>
			</div>
			<nav class="flex items-center gap-1">
				<Button
					variant="ghost"
					size="sm"
					onclick={cycleTheme}
					title="Theme: {themeLabel}"
				>
					{#if themeMode === 'auto'}
						<Monitor class="h-4 w-4" />
					{:else if themeMode === 'light'}
						<Sun class="h-4 w-4" />
					{:else}
						<Moon class="h-4 w-4" />
					{/if}
				</Button>
				<Button
					variant={page.url.pathname === '/' ? 'default' : 'ghost'}
					size="sm"
					href="#/"
				>
					Scores
				</Button>
				<Button
					variant={page.url.pathname === '/settings' ? 'default' : 'ghost'}
					size="sm"
					href="#/settings"
				>
					<Settings class="h-4 w-4" />
				</Button>
			</nav>
		</div>
	</header>

	<main class="container mx-auto px-4 py-8">
		{@render children()}
	</main>
</div>

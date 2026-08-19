<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';
	import Sun from '@lucide/svelte/icons/sun';
	import Moon from '@lucide/svelte/icons/moon';
	import Monitor from '@lucide/svelte/icons/monitor';
	import { timezoneStore } from '$lib/stores/timezone.svelte';

	let { children } = $props();

	// The device cannot know what timezone it is in and nothing upstream can
	// tell it, so this browser does — in the background, on every page load, so
	// that a scoreboard nobody opens the settings for still crosses the next
	// DST boundary correctly. It lives in the layout rather than on the
	// settings page because the logs and setup pages are visits too. Silent on
	// failure by design; see the store.
	onMount(() => {
		timezoneStore.seed();
	});

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
	<title>Pico Scoreboard</title>
</svelte:head>

<div class="layout">
	<header class="header">
		<div class="header-inner">
			<div class="logo">
				<span class="logo-emoji">&#x1F3DF;&#xFE0F;</span>
				<h1 class="logo-title">Pico Scoreboard</h1>
			</div>
			<nav class="nav">
				<a class="btn ghost sm" href="#/">Settings</a>
				<a class="btn ghost sm" href="#/logs">Logs</a>
			</nav>
			<button
				class="btn ghost sm icon-btn"
				onclick={cycleTheme}
				title="Theme: {themeLabel}"
			>
				{#if themeMode === 'auto'}
					<Monitor />
				{:else if themeMode === 'light'}
					<Sun />
				{:else}
					<Moon />
				{/if}
			</button>
		</div>
	</header>

	<main class="main">
		{@render children()}
	</main>
</div>

<style>
	.layout {
		min-block-size: 100vh;
		background-color: var(--background);
	}

	.header {
		border-block-end: 1px solid var(--border);
	}

	.header-inner {
		max-inline-size: 75rem;
		margin-inline: auto;
		display: flex;
		block-size: 3.5rem;
		align-items: center;
		justify-content: space-between;
		padding-inline: 1rem;
	}

	.logo {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.nav {
		display: flex;
		align-items: center;
		gap: 0.25rem;
		margin-inline-start: auto;
		margin-inline-end: 0.5rem;
	}

	.logo-emoji {
		font-size: 1.25rem;
	}

	.logo-title {
		font-size: 1.125rem;
		font-weight: 600;
	}

	/* Buttons come from the shared .btn system in app.css; only the
	   icon-only sizing is layout-specific. */
	.icon-btn {
		padding-inline: 0;
		inline-size: 2rem;
	}

	/* ---- Main content ---- */
	.main {
		max-inline-size: 75rem;
		margin-inline: auto;
		padding-inline: 1rem;
		padding-block: 2rem;
	}
</style>

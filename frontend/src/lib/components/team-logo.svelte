<script lang="ts">
	interface Props {
		teamId: string;
		teamName: string;
		abbreviation: string;
		primaryColor: string;
		size?: number;
	}

	let { teamId, teamName, abbreviation, primaryColor, size = 128 }: Props = $props();

	let status = $state<'loading' | 'loaded' | 'error'>('loading');

	// Reset status when teamId changes
	$effect(() => {
		teamId; // Track dependency
		status = 'loading';
	});

	function handleLoad() {
		status = 'loaded';
	}

	function handleError() {
		status = 'error';
	}
</script>

<div class="logo-wrapper">
	{#if status === 'loading'}
		<div class="skeleton"></div>
	{/if}

	{#if status === 'error'}
		<div class="fallback" style="background-color: {primaryColor}">
			{abbreviation}
		</div>
	{/if}

	<img
		src="/api/teams/{teamId}/logo?width={size}&height={size}"
		alt="{teamName} logo"
		class="logo-img {status === 'loaded' ? '' : 'hidden'}"
		onload={handleLoad}
		onerror={handleError}
	/>
</div>

<style>
	.logo-wrapper {
		position: relative;
		margin-inline: auto;
		margin-bottom: 0.5rem;
		height: 8rem;
		width: 8rem;
	}

	.skeleton {
		height: 100%;
		width: 100%;
		border-radius: 50%;
		background: var(--accent);
		animation: shimmer 2s ease-in-out infinite;
	}

	.fallback {
		display: flex;
		height: 100%;
		width: 100%;
		align-items: center;
		justify-content: center;
		border-radius: 50%;
		font-size: 1.125rem;
		font-weight: 700;
		color: white;
	}

	.logo-img {
		height: 100%;
		width: 100%;
		border-radius: 50%;
		object-fit: cover;
	}

	.hidden {
		display: none;
	}
</style>

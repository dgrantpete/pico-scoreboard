<script lang="ts">
	import { Skeleton } from '$lib/components/ui/skeleton';

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

<div class="relative mx-auto mb-2 h-32 w-32">
	{#if status === 'loading'}
		<Skeleton class="h-full w-full rounded-full" />
	{/if}

	{#if status === 'error'}
		<div
			class="flex h-full w-full items-center justify-center rounded-full text-lg font-bold text-white"
			style="background-color: {primaryColor}"
		>
			{abbreviation}
		</div>
	{/if}

	<img
		src="/api/teams/{teamId}/logo?width={size}&height={size}"
		alt="{teamName} logo"
		class="h-full w-full rounded-full object-cover {status === 'loaded' ? '' : 'hidden'}"
		onload={handleLoad}
		onerror={handleError}
	/>
</div>

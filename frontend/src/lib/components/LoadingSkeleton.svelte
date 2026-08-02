<script lang="ts">
	let { cards = 4 }: { cards?: number } = $props();
</script>

<div class="stack">
	{#each Array(cards) as _}
		<section class="card">
			<header class="card-header">
				<div class="skeleton" style="height: 1.5rem; width: 8rem;"></div>
				<div class="skeleton" style="height: 1rem; width: 12rem;"></div>
			</header>
			<div class="card-content">
				{#each Array(2) as _}
					<div class="field-group">
						<div class="skeleton" style="height: 1rem; width: 6rem;"></div>
						<div class="skeleton" style="height: 2.25rem; width: 100%;"></div>
					</div>
				{/each}
			</div>
		</section>
	{/each}
</div>

<script lang="ts">
	let {
		label,
		value,
		min = 0,
		max = 100,
		step = 1,
		hint = "",
		nested = false,
		format = (v: number) => String(v),
		oncommit,
	}: {
		label: string;
		/** Committed value, in slider space (caller maps units if needed) */
		value: number;
		min?: number;
		max?: number;
		step?: number;
		hint?: string;
		nested?: boolean;
		/** Render the readout for a slider-space value */
		format?: (value: number) => string;
		oncommit: (value: number) => void;
	} = $props();

	// Buffer during drag: the readout tracks the thumb live, but the store —
	// and therefore touched-tracking and any device side-effects — only sees
	// the final value on release.
	let dragValue = $state<number | null>(null);
	const displayValue = $derived(dragValue ?? value);
</script>

<div class="field-group" class:nested>
	<div class="row-between">
		<span class="label-text" class:text-xs={nested}>{label}</span>
		<span class="text-sm text-muted">{format(displayValue)}</span>
	</div>
	<input
		type="range"
		{min}
		{max}
		{step}
		value={displayValue}
		oninput={(e) => (dragValue = (e.currentTarget as HTMLInputElement).valueAsNumber)}
		onchange={(e) => {
			oncommit((e.currentTarget as HTMLInputElement).valueAsNumber);
			dragValue = null;
		}}
	/>
	{#if hint}
		<p class="hint">{hint}</p>
	{/if}
</div>

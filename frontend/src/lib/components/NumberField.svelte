<script lang="ts">
	let {
		id,
		label,
		hint = "",
		min,
		max,
		step = 1,
		value,
		validate,
		oncommit,
	}: {
		id: string;
		label: string;
		hint?: string;
		min?: number;
		max?: number;
		step?: number;
		value: number;
		/** Extra validation beyond min/max (e.g. cross-field constraints) */
		validate?: (value: number) => boolean;
		oncommit: (value: number) => void;
	} = $props();

	function handleChange(e: Event) {
		const input = e.target as HTMLInputElement;
		const parsed = parseInt(input.value);
		const valid =
			!isNaN(parsed) &&
			(min === undefined || parsed >= min) &&
			(max === undefined || parsed <= max) &&
			(validate === undefined || validate(parsed));

		if (valid) {
			oncommit(parsed);
		} else {
			// Revert the input to the last committed value
			input.value = String(value);
		}
	}
</script>

<div class="field-group">
	<label for={id}>{label}</label>
	<input {id} type="number" {min} {max} {step} {value} onchange={handleChange} />
	{#if hint}
		<p class="hint">{hint}</p>
	{/if}
</div>

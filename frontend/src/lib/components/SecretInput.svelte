<script lang="ts">
	import Eye from "@lucide/svelte/icons/eye";
	import EyeOff from "@lucide/svelte/icons/eye-off";

	let {
		id,
		label,
		placeholder = "",
		value,
		oninput,
	}: {
		id: string;
		label: string;
		placeholder?: string;
		value: string;
		oninput: (value: string) => void;
	} = $props();

	let show = $state(false);
</script>

<div class="field-group">
	<label for={id}>{label}</label>
	<div class="input-wrapper">
		<input
			{id}
			class="input-with-toggle"
			type={show ? "text" : "password"}
			{placeholder}
			{value}
			oninput={(e) => oninput((e.target as HTMLInputElement).value)}
		/>
		<button
			type="button"
			class="btn ghost sm toggle-btn"
			aria-label={show ? "Hide value" : "Show value"}
			onclick={() => (show = !show)}
		>
			{#if show}
				<EyeOff class="icon-muted" />
			{:else}
				<Eye class="icon-muted" />
			{/if}
		</button>
	</div>
</div>

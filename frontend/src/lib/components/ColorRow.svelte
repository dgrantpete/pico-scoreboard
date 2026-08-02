<script lang="ts">
	import type { Color } from "$lib/api/types";

	let {
		label,
		description,
		value,
		onchange,
	}: {
		label: string;
		description: string;
		value: Color;
		onchange: (color: Color) => void;
	} = $props();

	function rgbToHex(color: Color): string {
		const toHex = (n: number) => n.toString(16).padStart(2, "0");
		return `#${toHex(color.r)}${toHex(color.g)}${toHex(color.b)}`;
	}

	function hexToRgb(hex: string): Color {
		const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
		return result
			? {
					r: parseInt(result[1], 16),
					g: parseInt(result[2], 16),
					b: parseInt(result[3], 16),
				}
			: { r: 255, g: 255, b: 255 };
	}
</script>

<div class="row-between">
	<div class="label-group">
		<span class="label-text">{label}</span>
		<p class="text-sm text-muted">{description}</p>
	</div>
	<input
		type="color"
		class="color-picker"
		value={rgbToHex(value)}
		oninput={(e) => onchange(hexToRgb((e.target as HTMLInputElement).value))}
	/>
</div>

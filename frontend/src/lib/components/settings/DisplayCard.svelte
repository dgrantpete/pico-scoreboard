<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import SliderField from "$lib/components/SliderField.svelte";
	import type { GammaConfig } from "$lib/api/types";

	// Logarithmic slider mapping for data frequency (2 kHz to 50 MHz)
	const FREQ_MIN = 2; // kHz
	const FREQ_MAX = 50000; // kHz

	function freqToSlider(freqKhz: number): number {
		return (100 * Math.log(freqKhz / FREQ_MIN)) / Math.log(FREQ_MAX / FREQ_MIN);
	}

	function sliderToFreq(sliderValue: number): number {
		return Math.round(FREQ_MIN * Math.pow(FREQ_MAX / FREQ_MIN, sliderValue / 100));
	}

	function formatFrequency(freqKhz: number): string {
		if (freqKhz >= 1000) {
			return `${(freqKhz / 1000).toFixed(freqKhz % 1000 === 0 ? 0 : 1)} MHz`;
		}
		return `${freqKhz} kHz`;
	}

	const GAMMA_TYPE_OPTIONS = [
		{ value: "srgb", label: "sRGB" },
		{ value: "power", label: "Power" },
		{ value: "none", label: "None (Linear)" },
	] as const;

	function gammaTypeLabel(config: GammaConfig): string {
		return GAMMA_TYPE_OPTIONS.find((o) => o.value === config.type)?.label ?? config.type;
	}

	function handleGammaTypeChange(newType: string) {
		if (newType === "power") {
			settingsStore.updateDisplay("gamma", { type: "power", value: 2.2 });
		} else if (newType === "none") {
			settingsStore.updateDisplay("gamma", { type: "none" });
		} else {
			settingsStore.updateDisplay("gamma", { type: "srgb" });
		}
	}
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Display</h3>
			<p class="card-description">LED matrix brightness and refresh settings</p>
		</header>
		<div class="card-content gap-lg">
			<SliderField
				label="Brightness"
				value={config.display.brightness}
				max={100}
				format={(v) => `${v}%`}
				oncommit={(value) => settingsStore.updateDisplay("brightness", value)}
			/>

			<hr class="separator" />

			<SliderField
				label="Data Frequency"
				hint="LED matrix data clock speed. Very low values allow observing bitplane scanning."
				value={freqToSlider(config.display.data_frequency_khz)}
				min={0}
				max={100}
				step={0.1}
				format={(v) => formatFrequency(sliderToFreq(v))}
				oncommit={(value) => settingsStore.updateDisplay("data_frequency_khz", sliderToFreq(value))}
			/>

			<hr class="separator" />

			<SliderField
				label="Refresh Rate"
				hint="Target display refresh rate. Lower values save power but may cause flicker."
				value={config.display.target_refresh_rate}
				min={30}
				max={240}
				format={(v) => `${v} Hz`}
				oncommit={(value) => settingsStore.updateDisplay("target_refresh_rate", value)}
			/>

			<hr class="separator" />

			<div class="field-group">
				<div class="row-between">
					<label for="gamma-type">Gamma Correction</label>
					<span class="text-sm text-muted">
						{#if config.display.gamma.type === "power"}
							Power ({config.display.gamma.value.toFixed(1)})
						{:else}
							{gammaTypeLabel(config.display.gamma)}
						{/if}
					</span>
				</div>
				<select
					id="gamma-type"
					value={config.display.gamma.type}
					onchange={(e) => handleGammaTypeChange((e.currentTarget as HTMLSelectElement).value)}
				>
					{#each GAMMA_TYPE_OPTIONS as option}
						<option value={option.value}>{option.label}</option>
					{/each}
				</select>
				{#if config.display.gamma.type === "power"}
					<SliderField
						label="Power Value"
						nested
						value={config.display.gamma.value}
						min={1.0}
						max={3.0}
						step={0.1}
						format={(v) => v.toFixed(1)}
						oncommit={(value) =>
							settingsStore.updateDisplay("gamma", {
								type: "power",
								value: Math.round(value * 10) / 10,
							})}
					/>
				{/if}
				<p class="hint">
					{#if config.display.gamma.type === "srgb"}
						sRGB gamma with linear region. Best match for most content.
					{:else if config.display.gamma.type === "power"}
						Simple power function. 2.2 approximates sRGB.
					{:else}
						No gamma correction. Raw linear values sent to display.
					{/if}
				</p>
			</div>

			<hr class="separator" />

			<SliderField
				label="Dead Time"
				hint="Output-enable blanking time. Reduces ghosting but dims the display."
				value={config.display.blanking_time_ns}
				min={0}
				max={3000}
				step={10}
				format={(v) => `${v} ns`}
				oncommit={(value) => settingsStore.updateDisplay("blanking_time_ns", value)}
			/>
		</div>
	</section>
{/if}

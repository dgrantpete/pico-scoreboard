<script lang="ts">
	import { settingsStore } from "$lib/stores/settings.svelte";
	import SecretInput from "$lib/components/SecretInput.svelte";
</script>

{#if settingsStore.config}
	{@const config = settingsStore.config}
	<section class="card">
		<header class="card-header">
			<h3 class="card-title">Backend API</h3>
			<p class="card-description">Connection settings for the scores API</p>
		</header>
		<div class="card-content">
			<div class="field-group">
				<label for="api-url">API URL</label>
				<input
					id="api-url"
					type="url"
					placeholder="https://api.example.com"
					value={config.api.url}
					oninput={(e) => settingsStore.updateApi("url", (e.target as HTMLInputElement).value)}
				/>
			</div>
			<SecretInput
				id="api-key"
				label="API Key"
				placeholder="Enter API key"
				value={config.api.key}
				oninput={(value) => settingsStore.updateApi("key", value)}
			/>
		</div>
	</section>
{/if}

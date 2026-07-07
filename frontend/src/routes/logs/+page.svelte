<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import Pause from "@lucide/svelte/icons/pause";
	import Play from "@lucide/svelte/icons/play";
	import Copy from "@lucide/svelte/icons/copy";
	import Check from "@lucide/svelte/icons/check";
	import RefreshCw from "@lucide/svelte/icons/refresh-cw";
	import Loader2 from "@lucide/svelte/icons/loader-2";
	import { picoApi } from "$lib/api";
	import type { LogEntry } from "$lib/api/types";

	const POLL_MS = 3000;
	const MAX_CLIENT_ENTRIES = 500;

	// MicroPython's embedded epoch is 2000-01-01; Unix is 1970. A synced-RTC
	// timestamp in this decade is ~8.3e8 on the 2000 epoch vs ~1.7e9 on Unix,
	// so < 1e9 reliably means "2000 epoch, add the offset".
	const MICROPYTHON_EPOCH_OFFSET = 946_684_800;

	let entries = $state<LogEntry[]>([]);
	let lastSeq = $state(0);
	let paused = $state(false);
	let view = $state<"live" | "previous">("live");
	let previousLog = $state<string | null | undefined>(undefined); // undefined = not fetched
	let loadingPrevious = $state(false);
	let error = $state<string | null>(null);
	let copied = $state(false);

	// True until the first live fetch settles — drives the loading indicator
	// (an empty card before the device answers looks broken, not loading).
	let initialLoading = $state(true);

	let pollInterval: ReturnType<typeof setInterval> | null = null;
	let logEnd: HTMLDivElement | undefined = $state();

	function formatTimestamp(ts: number): string {
		if (ts < 100_000_000) {
			// RTC not yet synced when this was logged: seconds since boot-ish.
			return `+${ts}s`;
		}
		const unixMs = (ts < 1_000_000_000 ? ts + MICROPYTHON_EPOCH_OFFSET : ts) * 1000;
		return new Date(unixMs).toLocaleTimeString([], { hour12: false });
	}

	function levelName(level: number): string {
		return level === 1 ? "ERR" : level === 2 ? "DBG" : "???";
	}

	// Guards against stacking: if a poll is still in flight when the next
	// tick fires (device rebooting, WiFi drop), skip the tick instead of
	// piling a second request onto the device's tiny socket pool.
	let fetchInFlight = false;

	// A single slow poll self-heals on the next tick (the device gets busy
	// during game rotations); only surface an error once it looks persistent.
	let consecutiveFailures = 0;

	async function fetchNew() {
		if (fetchInFlight) return;
		fetchInFlight = true;
		try {
			const fresh = await picoApi.getLogs(lastSeq);
			consecutiveFailures = 0;
			error = null;
			if (fresh.length === 0) return;
			lastSeq = fresh[fresh.length - 1][0];
			entries = [...entries, ...fresh].slice(-MAX_CLIENT_ENTRIES);
			if (!paused) {
				// Follow the tail after the DOM updates
				setTimeout(() => logEnd?.scrollIntoView({ block: "end" }), 0);
			}
		} catch (e) {
			consecutiveFailures++;
			if (consecutiveFailures >= 2) {
				error = e instanceof Error ? e.message : "Failed to fetch logs";
			}
		} finally {
			initialLoading = false;
			fetchInFlight = false;
		}
	}

	async function fetchPrevious() {
		loadingPrevious = true;
		try {
			previousLog = await picoApi.getPreviousLog();
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : "Failed to fetch previous log";
		} finally {
			loadingPrevious = false;
		}
	}

	function setView(v: "live" | "previous") {
		view = v;
		if (v === "previous" && previousLog === undefined) {
			fetchPrevious();
		}
	}

	async function copyVisible() {
		const text =
			view === "live"
				? entries
						.map(([, ts, lvl, msg]) => `${formatTimestamp(ts)} ${levelName(lvl)} ${msg}`)
						.join("\n")
				: (previousLog ?? "");
		try {
			await navigator.clipboard.writeText(text);
			copied = true;
			setTimeout(() => (copied = false), 2000);
		} catch {
			// Clipboard API unavailable (e.g. plain-http captive portal)
		}
	}

	onMount(() => {
		fetchNew();
		pollInterval = setInterval(() => {
			if (!paused && view === "live" && !document.hidden) fetchNew();
		}, POLL_MS);
	});

	onDestroy(() => {
		if (pollInterval) clearInterval(pollInterval);
	});
</script>

<div class="logs-page">
	<div class="row-between">
		<div>
			<h2 class="page-title">Device Logs</h2>
			<p class="page-description">
				Live log ring from the scoreboard{view === "previous" ? " — previous boot" : ""}
			</p>
		</div>
		<div class="controls">
			<div class="view-toggle">
				<button
					class="btn sm {view === 'live' ? 'default' : 'ghost'}"
					onclick={() => setView("live")}
				>
					Live
				</button>
				<button
					class="btn sm {view === 'previous' ? 'default' : 'ghost'}"
					onclick={() => setView("previous")}
				>
					Previous boot
				</button>
			</div>
			{#if view === "live"}
				<button class="btn outline sm" onclick={() => (paused = !paused)}>
					{#if paused}
						<Play />
						Resume
					{:else}
						<Pause />
						Pause
					{/if}
				</button>
			{:else}
				<button class="btn outline sm" onclick={fetchPrevious}>
					<RefreshCw />
					Reload
				</button>
			{/if}
			<button class="btn outline sm" onclick={copyVisible}>
				{#if copied}
					<Check />
					Copied
				{:else}
					<Copy />
					Copy
				{/if}
			</button>
		</div>
	</div>

	{#if error}
		<div class="alert destructive" role="alert">{error}</div>
	{/if}

	<section class="card log-card">
		{#if view === "live"}
			{#if initialLoading && entries.length === 0}
				<div class="empty row-center">
					<Loader2 class="spinner icon-muted" />
					<span class="text-sm text-muted">Connecting to scoreboard…</span>
				</div>
			{:else if entries.length === 0}
				<p class="empty text-sm text-muted">No log entries yet.</p>
			{:else}
				<div class="log-lines">
					{#each entries as [seq, ts, lvl, msg] (seq)}
						<div class="log-line" class:is-error={lvl === 1}>
							<span class="ts">{formatTimestamp(ts)}</span>
							<span class="lvl">{levelName(lvl)}</span>
							<span class="msg">{msg}</span>
						</div>
					{/each}
					<div bind:this={logEnd}></div>
				</div>
			{/if}
		{:else if loadingPrevious || previousLog === undefined}
			<div class="empty row-center">
				<Loader2 class="spinner icon-muted" />
				<span class="text-sm text-muted">Loading previous boot log…</span>
			</div>
		{:else if previousLog === null}
			<p class="empty text-sm text-muted">
				No previous-boot log on flash. It appears after the first flush + reboot
				cycle (or read it over USB: <code>mpremote cat :/logs/previous.log</code>).
			</p>
		{:else}
			<pre class="raw-log">{previousLog}</pre>
		{/if}
	</section>
</div>

<style>
	/* Page-specific layout only — shared component styles live in app.css */
	.logs-page {
		max-width: 56rem;
		margin-inline: auto;
		display: flex;
		flex-direction: column;
		gap: 1rem;
	}

	.page-title {
		font-size: 1.5rem;
		font-weight: 700;
	}

	.page-description {
		color: var(--muted-foreground);
	}

	.controls {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		flex-wrap: wrap;
	}

	.view-toggle {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 0.375rem;
		overflow: hidden;

		& .btn {
			border-radius: 0;
		}
	}

	.log-card {
		padding: 0.75rem;
		max-height: 70vh;
		overflow-y: auto;
	}

	.empty {
		padding: 1rem;
	}

	.log-lines {
		display: flex;
		flex-direction: column;
		font-family: ui-monospace, monospace;
		font-size: 0.8125rem;
		line-height: 1.5;
	}

	.log-line {
		display: flex;
		gap: 0.625rem;
		padding-inline: 0.25rem;
		white-space: pre-wrap;
		word-break: break-word;

		&.is-error {
			color: oklch(0.637 0.237 25.331);
		}
	}

	.ts {
		color: var(--muted-foreground);
		flex-shrink: 0;
	}

	.lvl {
		flex-shrink: 0;
		font-weight: 600;
	}

	.log-line:not(.is-error) .lvl {
		color: var(--muted-foreground);
	}

	.msg {
		min-width: 0;
	}

	.raw-log {
		font-family: ui-monospace, monospace;
		font-size: 0.8125rem;
		line-height: 1.5;
		white-space: pre-wrap;
		word-break: break-word;
		margin: 0;
		padding: 0.25rem;
	}
</style>

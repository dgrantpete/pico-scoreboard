<script lang="ts">
	import { AreaChart, Tooltip, getTooltipContext } from 'layerchart';
	import { scaleTime, scaleLinear } from 'd3-scale';
	import * as Chart from '$lib/components/ui/chart';
	import { Button } from '$lib/components/ui/button';
	import { memoryTelemetryStore, type MemoryDataPoint } from '$lib/stores/memory-telemetry.svelte';

	// Extended data point type with KB values
	type MemoryDataPointWithKb = MemoryDataPoint & {
		memoryUsedKb: number;
		memoryFreeKb: number;
		totalKb: number;
	};

	// Time range options in minutes
	type TimeRange = 5 | 30 | 60 | 'all';

	let selectedRange = $state<TimeRange>('all');

	// Chart configuration for shadcn theming
	const chartConfig = {
		memoryUsedKb: {
			label: 'Memory Used',
			color: 'var(--chart-1)'
		}
	} satisfies Chart.ChartConfig;

	// Transform data to include KB values
	const filteredData = $derived(() => {
		const allData = memoryTelemetryStore.dataPoints;
		const sourceData = selectedRange === 'all' || allData.length === 0
			? allData
			: memoryTelemetryStore.getLastMinutes(selectedRange);

		// Add KB values to each data point
		return sourceData.map(point => ({
			...point,
			memoryUsedKb: Math.round(point.memoryUsed / 1024 * 10) / 10,
			memoryFreeKb: Math.round(point.memoryFree / 1024 * 10) / 10,
			totalKb: Math.round((point.memoryUsed + point.memoryFree) / 1024 * 10) / 10
		}));
	});

	// Calculate Y-axis domain based on total memory
	const yDomain = $derived(() => {
		const data = filteredData();
		if (data.length === 0) return [0, 100];
		const maxTotal = Math.max(...data.map(d => d.totalKb));
		// Round up to nearest 50 KB for cleaner axis
		return [0, Math.ceil(maxTotal / 50) * 50];
	});

	// Format time for axis based on data span
	function formatTime(date: Date): string {
		const data = filteredData();
		if (data.length < 2) return date.toLocaleTimeString();

		const span = data[data.length - 1].timestamp.getTime() - data[0].timestamp.getTime();
		const oneHour = 60 * 60 * 1000;

		if (span > oneHour) {
			return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
		}
		return date.toLocaleTimeString([], { minute: '2-digit', second: '2-digit' });
	}

	// Format time for tooltip (more detailed)
	function formatTooltipTime(date: Date): string {
		return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
	}

	function selectRange(range: TimeRange) {
		selectedRange = range;
	}

	function isSelected(range: TimeRange): boolean {
		return selectedRange === range;
	}
</script>

<!-- Custom tooltip component that reads from tooltip context -->
{#snippet CustomTooltip()}
	{@const tooltipCtx = getTooltipContext<MemoryDataPointWithKb>()}
	{#if tooltipCtx.data}
		<Tooltip.Root variant="none" class="pointer-events-none">
			<div class="rounded-lg border border-border/50 bg-background px-3 py-2 text-xs shadow-xl">
				<div class="mb-1 font-medium text-foreground">
					{formatTooltipTime(tooltipCtx.data.timestamp)}
				</div>
				<div class="space-y-0.5 text-muted-foreground">
					<div class="flex justify-between gap-4">
						<span>Used:</span>
						<span class="font-mono font-medium text-foreground">{tooltipCtx.data.memoryUsedKb} KB</span>
					</div>
					<div class="flex justify-between gap-4">
						<span>Free:</span>
						<span class="font-mono font-medium text-foreground">{tooltipCtx.data.memoryFreeKb} KB</span>
					</div>
					<div class="flex justify-between gap-4 border-t border-border/50 pt-1 mt-1">
						<span>Usage:</span>
						<span class="font-mono font-medium text-foreground">{tooltipCtx.data.memoryPercent}%</span>
					</div>
				</div>
			</div>
		</Tooltip.Root>
	{/if}
{/snippet}

<div class="space-y-2">
	<!-- Time range buttons -->
	<div class="flex gap-1">
		<Button
			variant={isSelected(5) ? 'default' : 'outline'}
			size="sm"
			class="h-6 px-2 text-xs"
			onclick={() => selectRange(5)}
		>
			5m
		</Button>
		<Button
			variant={isSelected(30) ? 'default' : 'outline'}
			size="sm"
			class="h-6 px-2 text-xs"
			onclick={() => selectRange(30)}
		>
			30m
		</Button>
		<Button
			variant={isSelected(60) ? 'default' : 'outline'}
			size="sm"
			class="h-6 px-2 text-xs"
			onclick={() => selectRange(60)}
		>
			1h
		</Button>
		<Button
			variant={isSelected('all') ? 'default' : 'outline'}
			size="sm"
			class="h-6 px-2 text-xs"
			onclick={() => selectRange('all')}
		>
			All
		</Button>
	</div>

	<!-- Chart -->
	{#if filteredData().length > 1}
		<Chart.Container config={chartConfig} class="h-[180px] w-full">
			<AreaChart
				data={filteredData()}
				x="timestamp"
				y="memoryUsedKb"
				xScale={scaleTime()}
				yScale={scaleLinear()}
				yDomain={yDomain()}
				series={[
					{
						key: 'memoryUsedKb',
						label: 'Used',
						color: 'var(--chart-1)'
					}
				]}
				tooltip={{ mode: 'bisect-x' }}
				props={{
					area: {
						fill: 'var(--chart-1)',
						fillOpacity: 0.3,
						stroke: 'var(--chart-1)',
						strokeWidth: 2
					},
					xAxis: {
						format: formatTime,
						ticks: 4
					},
					yAxis: {
						format: (v: number) => `${v} KB`
					},
					grid: {
						x: false
					}
				}}
			>
				{#snippet aboveMarks()}
					{@render CustomTooltip()}
				{/snippet}
			</AreaChart>
		</Chart.Container>
	{:else if filteredData().length === 1}
		<div class="flex h-[180px] items-center justify-center text-sm text-muted-foreground">
			Collecting data...
		</div>
	{:else}
		<div class="flex h-[180px] items-center justify-center text-sm text-muted-foreground">
			No data available
		</div>
	{/if}
</div>
